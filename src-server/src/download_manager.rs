//! 下载调度。从原 `src-tauri/src/download_manager.rs` 移植，改动只有三类：
//!
//! 1. `AppHandle` → `AppContext`（`app.get_config()` 由 `AppContextExt` 提供）
//! 2. `Event::emit(&app)` → `app.events().emit(topics::*, &event)`
//! 3. `tauri::async_runtime::spawn` → `tokio::spawn`
//!
//! 额外新增 `snapshot()`：Web 端 WebSocket 刚连上时需要一次性拿到全部任务状态，
//! 桌面版没有这个需求（它靠事件全量重放）。

use std::{
    collections::HashMap,
    io::Cursor,
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{anyhow, Context};
use bytes::Bytes;
use image::ImageFormat;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{watch, Semaphore, SemaphorePermit},
    task::JoinSet,
    time::sleep,
};

use crate::{
    context::AppContext,
    event_bus::topics,
    events::{
        DownloadSleepingEvent, DownloadSpeedEvent, DownloadTaskDeletedEvent, DownloadTaskEvent,
    },
    extensions::{AnyhowErrorToStringChain, AppContextExt},
    store::{DbTask, DbTaskState, ImageRepo, TaskRepo},
    types::{ChapterInfo, Comic},
};

/// 用于管理下载任务
///
/// 克隆 `DownloadManager` 的开销极小，性能开销几乎可以忽略不计。
/// 可以放心地在多个线程中传递和使用它的克隆副本。
#[derive(Clone)]
pub struct DownloadManager {
    app: AppContext,
    comic_sem: Arc<Semaphore>,
    img_sem: Arc<Semaphore>,
    byte_per_sec: Arc<AtomicU64>,
    download_tasks: Arc<RwLock<HashMap<String, DownloadTask>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadTaskState {
    Pending,
    Downloading,
    Paused,
    Cancelled,
    Completed,
    Failed,
}

impl DownloadTaskState {
    /// 映射到 DB 里的任务状态。两个枚举目前一一对应，
    /// 分开定义是为了让 DB 层的状态演化不牵连运行期状态机。
    pub fn to_db(self) -> DbTaskState {
        match self {
            Self::Pending => DbTaskState::Pending,
            Self::Downloading => DbTaskState::Downloading,
            Self::Paused => DbTaskState::Paused,
            Self::Cancelled => DbTaskState::Cancelled,
            Self::Completed => DbTaskState::Completed,
            Self::Failed => DbTaskState::Failed,
        }
    }
}

impl DownloadManager {
    pub fn new(app: AppContext) -> Self {
        let (comic_concurrency, img_concurrency) = {
            let config = app.get_config();
            let config = config.read();
            (config.comic_concurrency, config.img_concurrency)
        };

        let manager = DownloadManager {
            app,
            comic_sem: Arc::new(Semaphore::new(comic_concurrency)),
            img_sem: Arc::new(Semaphore::new(img_concurrency)),
            byte_per_sec: Arc::new(AtomicU64::new(0)),
            download_tasks: Arc::new(RwLock::new(HashMap::new())),
        };

        tokio::spawn(manager.clone().emit_download_speed_loop());

        // 恢复上次进程遗留的未完成任务。
        //
        // 只捞 `pending` / `downloading` / `failed` 三种态（终态任务没有恢复价值）。
        // 恢复需要重新走 wnacg API 拿漫画信息，是异步的，所以丢进后台任务。
        // 恢复失败不阻断启动：服务能起来比任务列表完整更重要。
        let recover_manager = manager.clone();
        tokio::spawn(async move {
            if let Err(err) = recover_manager.recover_pending_tasks().await {
                tracing::error!(
                    err_title = "恢复未完成下载任务失败",
                    message = %err
                );
            }
        });

        manager
    }

    /// 从 DB 里把上次进程遗留的未完成任务重新拉起。
    ///
    /// 这些任务只存了 `comic_id`，需要重新走一次 wnacg API 拿完整漫画信息
    /// （标题、图片列表）才能重建 `DownloadTask`。
    async fn recover_pending_tasks(&self) -> anyhow::Result<()> {
        let tasks = TaskRepo::list_resumable(self.app.store())?;
        if tasks.is_empty() {
            return Ok(());
        }

        tracing::info!(count = tasks.len(), "发现未完成的下载任务，开始恢复");

        let mut restored = 0usize;
        for task in tasks {
            match self.restore_one(&task).await {
                Ok(()) => restored += 1,
                Err(err) => {
                    // 单个任务恢复失败（比如漫画已下架、站点连不上）
                    // 不应拖垮其他任务。
                    tracing::warn!(
                        comic_id = task.comic_id,
                        comic_title = task.comic_title,
                        err_title = "恢复单个下载任务失败，已跳过",
                        message = %err
                    );
                }
            }
        }

        tracing::info!(restored, "未完成下载任务恢复完成");
        Ok(())
    }

    /// 重建单个任务并重新挂进调度。
    ///
    /// 关键点：先按图片表的事实对齐进度，再走 `create_download_task`——
    /// 它会调 `upsert_new` 保留已有状态与进度，所以恢复不会把一个
    /// 下载到一半的任务打回原点。
    ///
    /// wnacg 无章节，`comic_id` 即任务主键，恢复比 jm/pica 简单。
    async fn restore_one(&self, db_task: &DbTask) -> anyhow::Result<()> {
        let comic_id = db_task.comic_id.clone();

        // 进度对齐：图片表的 state 才是真相，DB 里的计数只是缓存。
        match ImageRepo::count_done(self.app.store(), &comic_id) {
            Ok(done) => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let done = done.max(0) as u32;
                // 恢复前把计数写回与图片表一致。
                if let Err(err) =
                    TaskRepo::set_progress(self.app.store(), &comic_id, i64::from(done))
                {
                    tracing::warn!(
                        comic_id,
                        err_title = "恢复时对齐任务进度失败",
                        message = %err
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    comic_id,
                    err_title = "恢复时统计已完成图片失败",
                    message = %err
                );
            }
        }

        // 拿完整漫画信息（含图片列表）。
        let comic = crate::utils::get_comic(&self.app, comic_id.clone())
            .await
            .context(format!("获取漫画`{comic_id}`信息失败"))?;

        self.create_download_task(comic);

        tracing::info!(comic_id, "已恢复下载任务");
        Ok(())
    }

    /// 配置变更后原地调整并发度。
    ///
    /// 注意不要 shutdown 后重建：已 spawn 的任务还持有旧的 permit，
    /// 重建会让实际并发翻倍。`Semaphore::add_permits` / `forget_permits`
    /// 是安全的增量调整。
    pub fn update_concurrency(&self, comic_concurrency: usize, img_concurrency: usize) {
        let current_comic = self.comic_sem.available_permits();
        let current_img = self.img_sem.available_permits();

        if comic_concurrency > current_comic {
            self.comic_sem.add_permits(comic_concurrency - current_comic);
        } else if comic_concurrency < current_comic {
            // `forget_permits` 只回收当前空闲的 permit，不影响正在下载的任务。
            self.comic_sem.forget_permits(current_comic - comic_concurrency);
        }

        if img_concurrency > current_img {
            self.img_sem.add_permits(img_concurrency - current_img);
        } else if img_concurrency < current_img {
            self.img_sem.forget_permits(current_img - img_concurrency);
        }
    }

    pub fn create_download_task(&self, comic: Comic) {
        use DownloadTaskState::{Downloading, Paused, Pending};
        let comic_id = comic.id.clone();
        let mut tasks = self.download_tasks.write();
        if let Some(task) = tasks.get(&comic_id) {
            // 如果任务已经存在，且状态是`Pending`、`Downloading`或`Paused`，则不创建新任务
            let state = *task.state_sender.borrow();
            if matches!(state, Pending | Downloading | Paused) {
                return;
            }
        }
        let task = DownloadTask::new(self.app.clone(), comic);
        // 先落库再 spawn：任务一旦开始跑就必须在 DB 里可查，
        // 否则重启恢复会漏掉「刚创建就崩溃」的任务。
        // 落库失败不阻断下载本身，只记日志。
        if let Err(err) = task.persist_new() {
            let comic_title = &task.comic.title;
            tracing::error!(
                err_title = format!("`{comic_title}`持久化下载任务失败"),
                message = %err
            );
        }
        tokio::spawn(task.clone().process());
        tasks.insert(comic_id, task);
    }

    pub fn pause_download_task(&self, comic_id: &str) -> anyhow::Result<()> {
        let tasks = self.download_tasks.read();
        let Some(task) = tasks.get(comic_id) else {
            return Err(anyhow!("未找到漫画ID为`{comic_id}`的下载任务"));
        };
        task.set_state(DownloadTaskState::Paused);
        Ok(())
    }

    pub fn resume_download_task(&self, comic_id: &str) -> anyhow::Result<()> {
        use DownloadTaskState::{Cancelled, Completed, Failed, Pending};
        let comic = {
            let tasks = self.download_tasks.read();
            let Some(task) = tasks.get(comic_id) else {
                return Err(anyhow!("未找到漫画ID为`{comic_id}`的下载任务"));
            };
            let task_state = *task.state_sender.borrow();

            if matches!(task_state, Failed | Cancelled | Completed) {
                // 如果任务状态是`Failed`、`Cancelled`或`Completed`，则获取 comic 用于重新创建下载任务
                Some(task.comic.as_ref().clone())
            } else {
                task.set_state(Pending);
                None
            }
        };
        // 如果 comic 不为 None，则重新创建下载任务
        if let Some(comic) = comic {
            self.create_download_task(comic);
        }
        Ok(())
    }

    pub fn cancel_download_task(&self, comic_id: &str) -> anyhow::Result<()> {
        let tasks = self.download_tasks.read();
        let Some(task) = tasks.get(comic_id) else {
            return Err(anyhow!("未找到漫画ID为`{comic_id}`的下载任务"));
        };
        task.set_state(DownloadTaskState::Cancelled);
        Ok(())
    }

    /// 删除任务记录（`delete_download_task` 命令用）。
    pub fn remove_download_task(&self, comic_id: &str) -> anyhow::Result<()> {
        let mut tasks = self.download_tasks.write();
        let Some(task) = tasks.remove(comic_id) else {
            return Err(anyhow!("未找到漫画ID为`{comic_id}`的下载任务"));
        };
        // 先置为取消，让正在跑的任务自己停下来，再发 Deleted 事件让前端移除。
        task.set_state(DownloadTaskState::Cancelled);
        task.emit_deleted();
        Ok(())
    }

    /// 返回当前所有下载任务的快照，供 WebSocket 建连时做初始状态同步。
    pub fn snapshot(&self) -> Vec<DownloadTaskEvent> {
        self.download_tasks
            .read()
            .values()
            .map(DownloadTask::to_event)
            .collect()
    }

    #[allow(clippy::cast_precision_loss)]
    async fn emit_download_speed_loop(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;
            let byte_per_sec = self.byte_per_sec.swap(0, Ordering::Relaxed);
            let mega_byte_per_sec = byte_per_sec as f64 / 1024.0 / 1024.0;
            let speed = format!("{mega_byte_per_sec:.2} MB/s");
            // 发送总进度条下载速度事件
            let _ = self
                .app
                .events()
                .emit(topics::DOWNLOAD_SPEED, &DownloadSpeedEvent { speed });
        }
    }
}

#[derive(Clone)]
struct DownloadTask {
    app: AppContext,
    download_manager: DownloadManager,
    comic: Arc<Comic>,
    state_sender: watch::Sender<DownloadTaskState>,
    downloaded_img_count: Arc<AtomicU32>,
    total_img_count: Arc<AtomicU32>,
    /// 最近一次失败原因。`set_state(Failed)` 时随状态一起落库，
    /// 让恢复后的任务能看到「上次为什么挂」。
    last_error: Arc<RwLock<Option<String>>>,
    /// 本任务对应的章节信息。
    ///
    /// wnacg 站点为单本无章节结构，这里存的始终是
    /// `comic.chapter_infos[0]`（合成章节，`chapter_id == comic.id`）。
    /// 字段存在的意义是**形状对齐** jmcomic/picacomic 的 `DownloadTask`
    /// ——前端可统一按「章节」渲染。
    ///
    /// 注意：wnacg 的目录名生成**不读本字段**，仍是
    /// `config.download_dir.join(comic.title)` 直接拼出。
    ///
    /// 因此本字段目前**没有任何读取点**，`#[allow(dead_code)]` 是
    /// 有意为之——它不是遗漏，是「形状对齐但暂不驱动行为」的显式标记。
    #[allow(dead_code)]
    chapter_info: Arc<ChapterInfo>,
    /// 章节级重试次数。
    ///
    /// 诚实说明：wnacg **没有章节级重试**（图片级重试记在
    /// `download_image.retry_count`），因此本值在实际运行中**恒为 0**。
    /// 保留它纯粹是对齐 jmcomic/picacomic 的 `DownloadTask` 形状，
    /// 不代表 wnacg 有对应的重试语义。
    ///
    /// 与 `chapter_info` 同理，目前没有任何读取点，`#[allow(dead_code)]`
    /// 是显式标记而非遗漏。
    #[allow(dead_code)]
    retry_count: Arc<AtomicU32>,
}

impl DownloadTask {
    pub fn new(app: AppContext, comic: Comic) -> Self {
        let download_manager = app.get_download_manager();
        let (state_sender, _) = watch::channel(DownloadTaskState::Pending);

        // wnacg 的 `comic.chapter_infos` 由 `Comic::from_html` /
        // `Comic::from_metadata` 合成为恒定单元素列表。这里取其首项即可；
        // 若为空（理论上不该发生）则就地合成一个，避免下游 panic。
        let chapter_info = comic.chapter_infos.first().cloned().unwrap_or_else(|| ChapterInfo {
            chapter_id: comic.id.clone(),
            chapter_title: comic.title.clone(),
            order: 1,
            is_downloaded: None,
            chapter_download_dir: None,
        });

        Self {
            app,
            download_manager,
            comic: Arc::new(comic),
            chapter_info: Arc::new(chapter_info),
            state_sender,
            downloaded_img_count: Arc::new(AtomicU32::new(0)),
            total_img_count: Arc::new(AtomicU32::new(0)),
            last_error: Arc::new(RwLock::new(None)),
            // wnacg 无章节级重试，恒为 0（见字段注释）。
            retry_count: Arc::new(AtomicU32::new(0)),
        }
    }

    /// 把新任务写进 `download_task`（`state = pending`）。
    ///
    /// 对齐 jmcomic/picacomic：任务一旦开始跑就必须在 DB 里可查。
    /// wnacg 没有章节，`comic_id` 即任务主键。
    fn persist_new(&self) -> anyhow::Result<()> {
        let download_dir = self
            .app
            .get_config()
            .read()
            .download_dir
            .to_string_lossy()
            .to_string();
        TaskRepo::upsert_new(
            self.app.store(),
            &self.comic.id,
            &self.comic.title,
            &download_dir,
        )
    }

    /// 把内存里的进度计数刷进 `download_task`。
    ///
    /// `total` 只在首次调用时写入（`set_total_img_count` 会把
    /// `done_img_count` 一并归零，重复调用会冲掉已完成的计数）。
    /// 与 jmcomic/picacomic 一致：DB 里的计数只是给前端和青龙看的缓存，
    /// `download_image.state` 才是真相，所以这里失败只 warn。
    fn persist_progress(&self, done: u32, total: u32, write_total: bool) {
        let comic_id = &self.comic.id;
        if write_total {
            if let Err(err) = TaskRepo::set_total_img_count(
                self.app.store(),
                comic_id,
                i64::from(total),
            ) {
                tracing::warn!(
                    err_title = "写入任务图片总数失败（不影响下载）",
                    comic_id,
                    message = %err
                );
                return;
            }
        }
        if let Err(err) = TaskRepo::set_progress(self.app.store(), comic_id, i64::from(done)) {
            tracing::warn!(
                err_title = "写入任务进度失败（不影响下载）",
                comic_id,
                message = %err
            );
        }
    }

    async fn process(self) {
        let download_comic_task = self.download_comic();
        tokio::pin!(download_comic_task);

        let mut state_receiver = self.state_sender.subscribe();
        state_receiver.mark_changed();
        let mut permit = None;
        loop {
            let state_is_downloading = *state_receiver.borrow() == DownloadTaskState::Downloading;
            let state_is_pending = *state_receiver.borrow() == DownloadTaskState::Pending;
            tokio::select! {
                () = &mut download_comic_task, if state_is_downloading && permit.is_some() => break,
                control_flow = self.acquire_comic_permit(&mut permit), if state_is_pending => {
                    match control_flow {
                        ControlFlow::Continue(()) => continue,
                        ControlFlow::Break(()) => break,
                    }
                },
                _ = state_receiver.changed() => {
                    match self.handle_state_change(&mut permit, &mut state_receiver) {
                        ControlFlow::Continue(()) => continue,
                        ControlFlow::Break(()) => break,
                    }
                }
            }
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    async fn download_comic(&self) {
        let comic_id = self.comic.id.clone();
        let comic_title = &self.comic.title;
        // 获取此漫画每张图片的下载链接
        let img_urls = self
            .comic
            .img_list
            .iter()
            .map(|img| &img.url)
            .filter(|url| !url.ends_with("shoucang.jpg")) // 过滤掉最后一张图片
            .map(|url| format!("https:{url}"))
            .collect::<Vec<_>>();
        // 总共需要下载的图片数量
        self.total_img_count
            .store(img_urls.len() as u32, Ordering::Relaxed);

        // 创建临时下载目录
        let Some(temp_download_dir) = self.create_temp_download_dir() else {
            return;
        };
        // 清理临时下载目录中与`config.download_format`对不上的文件
        self.clean_temp_download_dir(&temp_download_dir);

        let mut join_set = JoinSet::new();
        // 开始下载之前，先保存元数据
        if let Err(err) = self.save_metadata(&temp_download_dir) {
            let err_title = format!("`{comic_title}`保存元数据失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
            return;
        }
        // 登记图片清单（断点续传的锚点：已 done 的图片不会被重置）。
        // 顺便把 total 写进任务表——`set_total_img_count` 会把 done 归零，
        // 所以只在这一处调用一次。
        if let Err(err) = ImageRepo::insert_many(self.app.store(), &comic_id, &img_urls) {
            let err_title = format!("`{comic_title}`登记图片清单失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
        }
        self.persist_progress(0, self.total_img_count.load(Ordering::Relaxed), true);

        // 逐一创建下载任务
        for (i, url) in img_urls.into_iter().enumerate() {
            let temp_download_dir = temp_download_dir.clone();
            let download_img_task = DownloadImgTask::new(self, url, temp_download_dir, i);
            // 创建下载任务
            join_set.spawn(download_img_task.process());
        }
        // 等待所有下载任务完成
        join_set.join_all().await;
        tracing::trace!(comic_id, comic_title, "所有图片下载任务完成");
        // 检查此漫画的图片是否全部下载成功
        let downloaded_img_count = self.downloaded_img_count.load(Ordering::Relaxed);
        let total_img_count = self.total_img_count.load(Ordering::Relaxed);
        // 此漫画的图片未全部下载成功
        if downloaded_img_count != total_img_count {
            let err_title = format!("`{comic_title}`下载不完整");
            let err_msg =
                format!("总共有`{total_img_count}`张图片，但只下载了`{downloaded_img_count}`张");
            tracing::error!(err_title, message = err_msg);

            self.set_state(DownloadTaskState::Failed);
            self.emit_download_task_event();

            return;
        }
        // 此漫画的图片全部下载成功
        if let Err(err) = self.rename_temp_download_dir(&temp_download_dir) {
            let err_title = format!("`{comic_title}`重命名临时下载目录失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);

            self.set_state(DownloadTaskState::Failed);
            self.emit_download_task_event();

            return;
        }
        tracing::trace!(
            comic_id,
            comic_title,
            "重命名临时下载目录`{}`成功",
            temp_download_dir.display()
        );
        tracing::info!(comic_id, comic_title, "漫画下载成功");

        self.sleep_between_comics().await;
        // 发送下载结束事件
        self.set_state(DownloadTaskState::Completed);
        self.emit_download_task_event();
    }

    fn create_temp_download_dir(&self) -> Option<PathBuf> {
        let comic_id = self.comic.id.clone();
        let comic_title = &self.comic.title;

        let temp_download_dir = self
            .app
            .get_config()
            .read()
            .download_dir
            .join(format!(".下载中-{comic_title}")); // 以 `.下载中-` 开头，表示是临时目录

        if let Err(err) = std::fs::create_dir_all(&temp_download_dir).map_err(anyhow::Error::from) {
            // 如果创建目录失败，则发送下载漫画结束事件，并返回
            let err_title = format!(
                "`{comic_title}`创建目录`{}`失败",
                temp_download_dir.display()
            );
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);

            self.set_state(DownloadTaskState::Failed);
            self.emit_download_task_event();

            return None;
        }

        tracing::trace!(
            comic_id,
            comic_title,
            "创建临时下载目录`{}`成功",
            temp_download_dir.display()
        );

        Some(temp_download_dir)
    }

    /// 删除临时下载目录中与`config.download_format`对不上的文件
    fn clean_temp_download_dir(&self, temp_download_dir: &Path) {
        let comic_id = self.comic.id.clone();
        let comic_title = &self.comic.title;

        let entries = match std::fs::read_dir(temp_download_dir).map_err(anyhow::Error::from) {
            Ok(entries) => entries,
            Err(err) => {
                let err_title = format!(
                    "`{comic_title}`读取临时下载目录`{}`失败",
                    temp_download_dir.display()
                );
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
                return;
            }
        };

        let download_format = self.app.get_config().read().download_format;
        let extension = download_format.extension();
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            // path有扩展名，且能转换为utf8，并与`config.download_format`一致或是gif，则保留
            let should_keep = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "gif" || Some(ext) == extension);
            if should_keep {
                continue;
            }
            // 否则删除文件
            if let Err(err) = std::fs::remove_file(&path).map_err(anyhow::Error::from) {
                let err_title =
                    format!("`{comic_title}`删除临时下载目录的`{}`失败", path.display());
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
            }
        }

        tracing::trace!(
            comic_id,
            comic_title,
            "清理临时下载目录`{}`成功",
            temp_download_dir.display()
        );
    }

    async fn acquire_comic_permit<'a>(
        &'a self,
        permit: &mut Option<SemaphorePermit<'a>>,
    ) -> ControlFlow<()> {
        let comic_id = self.comic.id.clone();
        let comic_title = &self.comic.title;

        tracing::debug!(comic_id, comic_title, "漫画开始排队");

        self.emit_download_task_event();

        *permit = match permit.take() {
            // 如果有permit，则直接用
            Some(permit) => Some(permit),
            // 如果没有permit，则获取permit
            None => match self
                .download_manager
                .comic_sem
                .acquire()
                .await
                .map_err(anyhow::Error::from)
            {
                Ok(permit) => Some(permit),
                Err(err) => {
                    let err_title = format!("`{comic_title}`获取下载漫画的permit失败");
                    let string_chain = err.to_string_chain();
                    tracing::error!(err_title, message = string_chain);

                    self.set_state(DownloadTaskState::Failed);
                    self.emit_download_task_event();

                    return ControlFlow::Break(());
                }
            },
        };
        // 如果当前任务状态不是`Pending`，则不将任务状态设置为`Downloading`
        if *self.state_sender.borrow() != DownloadTaskState::Pending {
            return ControlFlow::Continue(());
        }
        // 将任务状态设置为`Downloading`
        if let Err(err) = self
            .state_sender
            .send(DownloadTaskState::Downloading)
            .map_err(anyhow::Error::from)
        {
            let err_title = format!("`{comic_title}`发送状态`Downloading`失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }

    fn handle_state_change<'a>(
        &'a self,
        permit: &mut Option<SemaphorePermit<'a>>,
        state_receiver: &mut watch::Receiver<DownloadTaskState>,
    ) -> ControlFlow<()> {
        let comic_id = self.comic.id.clone();
        let comic_title = &self.comic.title;

        self.emit_download_task_event();
        let state = *state_receiver.borrow();
        match state {
            DownloadTaskState::Paused => {
                tracing::debug!(comic_id, comic_title, "漫画暂停中");
                if let Some(permit) = permit.take() {
                    drop(permit);
                }
                ControlFlow::Continue(())
            }
            DownloadTaskState::Cancelled => {
                tracing::debug!(comic_id, comic_title, "漫画取消下载");
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        }
    }

    async fn sleep_between_comics(&self) {
        let comic_id = self.comic.id.clone();
        let mut remaining_sec = self.app.get_config().read().comic_download_interval_sec;
        while remaining_sec > 0 {
            // 发送章节休眠事件
            let _ = self.app.events().emit(
                topics::DOWNLOAD_SLEEPING,
                &DownloadSleepingEvent {
                    comic_id: comic_id.clone(),
                    remaining_sec,
                },
            );
            sleep(Duration::from_secs(1)).await;
            remaining_sec -= 1;
        }
    }

    fn set_state(&self, state: DownloadTaskState) {
        let comic_title = &self.comic.title;
        // 先落库再广播：让持久状态成为真相源，内存与事件都是它的投影。
        // 落库失败只记日志不中断——下载本身比任务记录更重要。
        let last_error = self.last_error.read().clone();
        if let Err(err) = TaskRepo::set_state(
            self.app.store(),
            &self.comic.id,
            state.to_db(),
            last_error.as_deref(),
        ) {
            let err_title = format!("`{comic_title}`持久化状态`{state:?}`失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
        }
        if let Err(err) = self.state_sender.send(state).map_err(anyhow::Error::from) {
            let err_title = format!("`{comic_title}`发送状态`{state:?}`失败");
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
        }
    }

    /// 把当前任务状态打包成事件。
    fn to_event(&self) -> DownloadTaskEvent {
        DownloadTaskEvent {
            state: *self.state_sender.borrow(),
            comic: self.comic.as_ref().clone(),
            downloaded_img_count: self.downloaded_img_count.load(Ordering::Relaxed),
            total_img_count: self.total_img_count.load(Ordering::Relaxed),
        }
    }

    fn emit_download_task_event(&self) {
        let _ = self
            .app
            .events()
            .emit(topics::DOWNLOAD_TASK, &self.to_event());
    }

    /// 通知前端把这个任务卡片移除。桌面版没有删除任务的概念，
    /// 删除只发生在「已下载」列表里；Web 版加了「移除任务记录」按钮，
    /// 因此单独发一条 [`DownloadTaskDeletedEvent`]，与状态事件区分开。
    fn emit_deleted(&self) {
        let _ = self.app.events().emit(
            topics::DOWNLOAD_TASK_DELETED,
            &DownloadTaskDeletedEvent {
                comic_id: self.comic.id.clone(),
            },
        );
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn save_metadata(&self, temp_download_dir: &Path) -> anyhow::Result<()> {
        let mut comic = self.comic.as_ref().clone();
        // 将所有comic的is_downloaded字段设置为None，这样能使is_downloaded字段在序列化时被忽略
        comic.is_downloaded = None;

        let comic_title = &comic.title;
        let comic_json = serde_json::to_string_pretty(&comic).context(format!(
            "`{comic_title}`的元数据保存失败，将Comic序列化为json失败"
        ))?;

        let metadata_path = temp_download_dir.join("元数据.json");

        std::fs::write(&metadata_path, comic_json).context(format!(
            "`{comic_title}`的元数据保存失败，写入文件`{}`失败",
            metadata_path.display()
        ))?;

        Ok(())
    }

    fn rename_temp_download_dir(&self, temp_download_dir: &Path) -> anyhow::Result<()> {
        let Some(parent) = temp_download_dir.parent() else {
            return Err(anyhow!("无法获取`{}`的父目录", temp_download_dir.display()));
        };

        let download_dir = parent.join(&self.comic.title);

        if download_dir.exists() {
            std::fs::remove_dir_all(&download_dir)
                .context(format!("删除目录`{}`失败", download_dir.display()))?;
        }

        std::fs::rename(temp_download_dir, &download_dir).context(format!(
            "将`{}`重命名为`{}`失败",
            temp_download_dir.display(),
            download_dir.display()
        ))?;

        Ok(())
    }
}

#[derive(Clone)]
struct DownloadImgTask {
    app: AppContext,
    download_manager: DownloadManager,
    download_task: DownloadTask,
    url: String,
    temp_download_dir: PathBuf,
    index: usize,
}

impl DownloadImgTask {
    pub fn new(
        download_task: &DownloadTask,
        url: String,
        temp_download_dir: PathBuf,
        index: usize,
    ) -> Self {
        Self {
            app: download_task.app.clone(),
            download_manager: download_task.download_manager.clone(),
            download_task: download_task.clone(),
            url,
            temp_download_dir,
            index,
        }
    }

    async fn process(self) {
        let download_img_task = self.download_img();
        tokio::pin!(download_img_task);

        let mut state_receiver = self.download_task.state_sender.subscribe();
        state_receiver.mark_changed();
        let mut permit = None;
        loop {
            let state_is_downloading = *state_receiver.borrow() == DownloadTaskState::Downloading;
            let state_is_pending = *state_receiver.borrow() == DownloadTaskState::Pending;
            tokio::select! {
                () = &mut download_img_task, if state_is_downloading && permit.is_some() => break,
                control_flow = self.acquire_img_permit(&mut permit), if state_is_pending => {
                    match control_flow {
                        ControlFlow::Continue(()) => continue,
                        ControlFlow::Break(()) => break,
                    }
                },
                _ = state_receiver.changed() => {
                    match self.handle_state_change(&mut permit, &mut state_receiver) {
                        ControlFlow::Continue(()) => continue,
                        ControlFlow::Break(()) => break,
                    }
                }
            }
        }
    }

    async fn download_img(&self) {
        let url = &self.url;
        let comic_id = self.download_task.comic.id.clone();
        let comic_title = &self.download_task.comic.title;
        let temp_download_dir = &self.temp_download_dir;

        let (use_original_filename, download_format) = {
            let config = self.app.get_config();
            let config = config.read();
            (config.use_original_filename, config.download_format)
        };

        let index_filename = format!("{:04}", self.index + 1);
        let original_filename = self
            .url
            .rsplit('/')
            .next()
            .and_then(|s| s.split('.').next())
            .unwrap_or(&index_filename);
        let img_filename = if use_original_filename {
            original_filename
        } else {
            &index_filename
        };

        if let Some(ext) = download_format.extension() {
            let user_format_path = temp_download_dir.join(format!("{img_filename}.{ext}"));
            let gif_path = temp_download_dir.join(format!("{img_filename}.gif"));

            if user_format_path.exists() || gif_path.exists() {
                // 如果图片已存在，则跳过下载
                tracing::trace!(comic_id, comic_title, url, "图片已存在，跳过下载");
                #[allow(clippy::cast_possible_wrap)]
                let img_index = self.index as i64;
                if let Err(err) =
                    ImageRepo::mark_done(self.app.store(), &comic_id, img_index, None)
                {
                    tracing::error!(
                        err_title = format!("标记图片`{img_index}`完成失败"),
                        message = %err
                    );
                }
                let done = self
                    .download_task
                    .downloaded_img_count
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;
                let total = self.download_task.total_img_count.load(Ordering::Relaxed);
                self.download_task.persist_progress(done, total, false);
                self.download_task.emit_download_task_event();
                return;
            }
        }

        tracing::trace!(comic_id, comic_title, url, "开始下载图片");

        // 下载图片
        let (img_data, img_format) = match self
            .app
            .get_wnacg_client()
            .get_img_data_and_format(url)
            .await
        {
            Ok(data_and_format) => data_and_format,
            Err(err) => {
                let err_title = format!("下载图片`{url}`失败");
                let string_chain = err.to_string_chain();
                tracing::error!(err_title, message = string_chain);
                // 记录失败原因。**不**推进计数——它仍然是未完成的。
                #[allow(clippy::cast_possible_wrap)]
                let img_index = self.index as i64;
                if let Err(err) =
                    ImageRepo::mark_failed(self.app.store(), &comic_id, img_index, &string_chain)
                {
                    tracing::error!(
                        err_title = format!("记录图片`{img_index}`失败原因失败"),
                        message = %err
                    );
                }
                return;
            }
        };
        let img_data_len = img_data.len() as u64;

        tracing::trace!(comic_id, comic_title, url, "图片成功下载到内存");

        // 获取图片格式的扩展名
        let src_img_ext = match img_format {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::WebP => "webp",
            ImageFormat::Gif => "gif",
            _ => {
                let err_title = format!("保存图片`{url}`失败");
                let err_msg = format!("遇到了预料之外的图片格式`{img_format:?}`，请反馈给开发者");
                tracing::error!(err_title, message = err_msg);
                return;
            }
        };

        let ext = match img_format {
            ImageFormat::Gif => "gif",
            _ => download_format.extension().unwrap_or(src_img_ext),
        };
        let save_path = temp_download_dir.join(format!("{img_filename}.{ext}"));

        let target_format = match img_format {
            ImageFormat::Gif => ImageFormat::Gif,
            _ => download_format.to_image_format().unwrap_or(img_format),
        };

        // 保存图片
        if let Err(err) = save_img(&save_path, target_format, img_data, img_format).await {
            let err_title = format!("保存图片`{}`失败", save_path.display());
            let string_chain = err.to_string_chain();
            tracing::error!(err_title, message = string_chain);
            return;
        }

        tracing::trace!(
            comic_id,
            url,
            comic_title,
            "图片成功保存到`{}`",
            save_path.display()
        );

        // 记录下载字节数
        self.download_manager
            .byte_per_sec
            .fetch_add(img_data_len, Ordering::Relaxed);

        #[allow(clippy::cast_possible_wrap)]
        let img_index = self.index as i64;
        if let Err(err) = ImageRepo::mark_done(
            self.app.store(),
            &comic_id,
            img_index,
            Some(img_data_len as i64),
        ) {
            tracing::error!(
                err_title = format!("标记图片`{img_index}`完成失败"),
                message = %err
            );
        }
        let done = self
            .download_task
            .downloaded_img_count
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let total = self.download_task.total_img_count.load(Ordering::Relaxed);
        self.download_task.persist_progress(done, total, false);
        self.download_task.emit_download_task_event();

        let img_download_interval_sec = self.app.get_config().read().img_download_interval_sec;
        sleep(Duration::from_secs(img_download_interval_sec)).await;
    }

    async fn acquire_img_permit<'a>(
        &'a self,
        permit: &mut Option<SemaphorePermit<'a>>,
    ) -> ControlFlow<()> {
        let url = &self.url;
        let comic_id = self.download_task.comic.id.clone();
        let comic_title = &self.download_task.comic.title;

        tracing::trace!(comic_id, comic_title, url, "图片开始排队");

        *permit = match permit.take() {
            // 如果有permit，则直接用
            Some(permit) => Some(permit),
            // 如果没有permit，则获取permit
            None => match self
                .download_manager
                .img_sem
                .acquire()
                .await
                .map_err(anyhow::Error::from)
            {
                Ok(permit) => Some(permit),
                Err(err) => {
                    let err_title = format!("`{comic_title}`获取下载图片的permit失败");
                    let string_chain = err.to_string_chain();
                    tracing::error!(err_title, message = string_chain);
                    return ControlFlow::Break(());
                }
            },
        };
        ControlFlow::Continue(())
    }

    fn handle_state_change<'a>(
        &'a self,
        permit: &mut Option<SemaphorePermit<'a>>,
        state_receiver: &mut watch::Receiver<DownloadTaskState>,
    ) -> ControlFlow<()> {
        let url = &self.url;
        let comic_id = self.download_task.comic.id.clone();
        let comic_title = &self.download_task.comic.title;

        let state = *state_receiver.borrow();
        match state {
            DownloadTaskState::Paused => {
                tracing::trace!(comic_id, comic_title, url, "图片暂停下载");
                if let Some(permit) = permit.take() {
                    drop(permit);
                }
                ControlFlow::Continue(())
            }
            DownloadTaskState::Cancelled => {
                tracing::trace!(comic_id, comic_title, url, "图片取消下载");
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        }
    }
}

/// 保存图片。若无需转码则直接写字节，避免无谓的解码/编码开销。
async fn save_img(
    path: &Path,
    target_format: ImageFormat,
    img_data: Bytes,
    src_format: ImageFormat,
) -> anyhow::Result<()> {
    // 格式一致，直接落盘
    if target_format == src_format {
        tokio::fs::write(path, &img_data)
            .await
            .context(format!("写入文件`{}`失败", path.display()))?;
        return Ok(());
    }

    // 需要转码：解码 → 编码 → 写入。放到阻塞线程池，避免卡住 async 运行时。
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let img = image::load_from_memory_with_format(&img_data, src_format)
            .context("解码图片失败")?;

        let mut buffer = Cursor::new(Vec::new());
        img.write_to(&mut buffer, target_format)
            .context("编码图片失败")?;

        std::fs::write(&path, buffer.into_inner())
            .context(format!("写入文件`{}`失败", path.display()))?;

        Ok(())
    })
    .await
    .context("图片转码任务 panic")??;

    Ok(())
}