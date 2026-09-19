//! 从原 `src-tauri/src/commands.rs` 移植过来的业务函数。
//!
//! 相对桌面版的改动：
//! - `AppHandle` → `&AppContext`
//! - 去掉 `#[tauri::command]` / `#[specta::specta]` 标注
//! - 去掉桌面专属能力（`show_path_in_file_manager` 用系统文件管理器打开）
//! - 导出 PDF/CBZ 改为纯服务端实现（见 `export.rs`）
//! - 新增书架下载所需的 Web 端幂等封装

use anyhow::Context;
use std::time::Duration;
use tokio::task::JoinSet;

use crate::{
    config::Config,
    context::AppContext,
    errors::{CommandError, CommandResult},
    events::{DownloadShelfEvent, DownloadTaskEvent},
    export, logger,
    types::{Comic, GetShelfResult, SearchResult, UserProfile},
    utils, wnacg_client,
};

// ════════════════════════════════════════════════════════════════
// 配置
// ════════════════════════════════════════════════════════════════

pub fn get_config(app: &AppContext) -> Config {
    let config = app.config_read().clone();
    tracing::debug!("获取配置成功");
    config
}

/// 保存配置。
///
/// 与原桌面版一致：代理配置变化时重建 HTTP client，文件日志开关变化时热切换日志层。
pub fn save_config(app: &AppContext, config: Config) -> CommandResult<()> {
    let config_state = app.config();
    let wnacg_client = app.wnacg_client();

    let (proxy_changed, file_logger_changed, enable_file_logger) = {
        let old = config_state.read();
        let proxy_changed = old.proxy_mode != config.proxy_mode
            || old.proxy_host != config.proxy_host
            || old.proxy_port != config.proxy_port;
        let file_logger_changed = old.enable_file_logger != config.enable_file_logger;
        (proxy_changed, file_logger_changed, config.enable_file_logger)
    };

    {
        // 包裹在大括号中，以便自动释放写锁
        let mut config_state = config_state.write();
        *config_state = config;
        let config_path = app.paths().config_path();
        config_state
            .save(&config_path)
            .map_err(|err| CommandError::from("保存配置失败", err))?;
        tracing::debug!("保存配置成功");
    }

    if proxy_changed {
        wnacg_client.reload_client();
    }

    if file_logger_changed {
        if enable_file_logger {
            logger::reload_file_logger()
                .map_err(|err| CommandError::from("重新加载文件日志失败", err))?;
        } else {
            logger::disable_file_logger()
                .map_err(|err| CommandError::from("禁用文件日志失败", err))?;
        }
    }

    // 广播配置变更，前端可据此刷新界面。带上 `download_format`，
    // 前端收到后无需再发一次 GET /api/config。
    let download_format = app.config().read().download_format;
    app.events().emit(
        crate::event_bus::topics::CONFIG_CHANGED,
        &crate::events::ConfigChangedEvent { download_format },
    );

    Ok(())
}

// ════════════════════════════════════════════════════════════════
// 登录 / 用户信息
// ════════════════════════════════════════════════════════════════

pub async fn login(app: &AppContext, username: String, password: String) -> CommandResult<String> {
    let wnacg_client = app.wnacg_client();

    let cookie = wnacg_client
        .login(&username, &password)
        .await
        .map_err(|err| CommandError::from("登录失败", err))?;
    tracing::debug!("登录成功");
    Ok(cookie)
}

pub async fn get_user_profile(app: &AppContext) -> CommandResult<UserProfile> {
    let wnacg_client = app.wnacg_client();

    let user_profile = wnacg_client
        .get_user_profile()
        .await
        .map_err(|err| CommandError::from("获取用户信息失败", err))?;
    tracing::debug!("获取用户信息成功");
    Ok(user_profile)
}

// ════════════════════════════════════════════════════════════════
// 搜索 / 详情 / 书架
// ════════════════════════════════════════════════════════════════

pub async fn search_by_keyword(
    app: &AppContext,
    keyword: String,
    page_num: i64,
) -> CommandResult<SearchResult> {
    let wnacg_client = app.wnacg_client();

    let search_result = wnacg_client
        .search_by_keyword(&keyword, page_num)
        .await
        .map_err(|err| CommandError::from("关键词搜索失败", err))?;
    tracing::debug!("关键词搜索成功");
    Ok(search_result)
}

pub async fn search_by_tag(
    app: &AppContext,
    tag_name: String,
    page_num: i64,
) -> CommandResult<SearchResult> {
    let wnacg_client = app.wnacg_client();

    let search_result = wnacg_client
        .search_by_tag(&tag_name, page_num)
        .await
        .map_err(|err| CommandError::from("按标签搜索失败", err))?;
    tracing::debug!("标签搜索成功");
    Ok(search_result)
}

pub async fn get_comic(app: &AppContext, id: i64) -> CommandResult<Comic> {
    let wnacg_client = app.wnacg_client();

    let comic = wnacg_client
        .get_comic(id)
        .await
        .map_err(|err| CommandError::from("获取漫画失败", err))?;
    tracing::debug!("获取漫画成功");
    Ok(comic)
}

pub async fn get_shelf(
    app: &AppContext,
    shelf_id: i64,
    page_num: i64,
) -> CommandResult<GetShelfResult> {
    let wnacg_client = app.wnacg_client();

    let get_shelf_result = wnacg_client
        .get_shelf(shelf_id, page_num)
        .await
        .map_err(|err| CommandError::from("获取书架失败", err))?;
    tracing::debug!("获取书架成功");
    Ok(get_shelf_result)
}

// ════════════════════════════════════════════════════════════════
// 下载任务
// ════════════════════════════════════════════════════════════════

pub fn create_download_task(app: &AppContext, comic: Comic) {
    let download_manager = app.download_manager();
    download_manager.create_download_task(comic);
}

pub fn pause_download_task(app: &AppContext, comic_id: i64) -> CommandResult<()> {
    let download_manager = app.download_manager();
    download_manager
        .pause_download_task(comic_id)
        .map_err(|err| CommandError::from("暂停下载任务失败", err))?;
    tracing::debug!("暂停下载任务成功");
    Ok(())
}

pub fn resume_download_task(app: &AppContext, comic_id: i64) -> CommandResult<()> {
    let download_manager = app.download_manager();
    download_manager
        .resume_download_task(comic_id)
        .map_err(|err| CommandError::from("继续下载任务失败", err))?;
    tracing::debug!("继续下载任务成功");
    Ok(())
}

pub fn cancel_download_task(app: &AppContext, comic_id: i64) -> CommandResult<()> {
    let download_manager = app.download_manager();
    download_manager
        .cancel_download_task(comic_id)
        .map_err(|err| CommandError::from("取消下载任务失败", err))?;
    tracing::debug!("取消下载任务成功");
    Ok(())
}

/// 列出内存中当前正在跟踪的下载任务（不查数据库）。
pub fn list_download_tasks(app: &AppContext) -> Vec<DownloadTaskEvent> {
    app.download_manager().snapshot()
}

// ════════════════════════════════════════════════════════════════
// 已下载的漫画
// ════════════════════════════════════════════════════════════════

pub fn get_downloaded_comics(app: &AppContext) -> CommandResult<Vec<Comic>> {
    let download_dir = app.config_read().download_dir.clone();
    // 遍历下载目录，获取所有元数据文件的路径和修改时间
    let mut metadata_path_with_modify_time = std::fs::read_dir(&download_dir)
        .map_err(|err| {
            let err_title = format!(
                "获取已下载的漫画失败，读取下载目录`{}`失败",
                download_dir.display()
            );
            CommandError::from(&err_title, err)
        })?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if entry.file_name().to_string_lossy().starts_with(".下载中-") {
                return None;
            }
            let metadata_path = entry.path().join("元数据.json");
            if !metadata_path.exists() {
                return None;
            }
            let modify_time = metadata_path.metadata().ok()?.modified().ok()?;
            Some((metadata_path, modify_time))
        })
        .collect::<Vec<_>>();
    // 按照文件修改时间排序，最新的排在最前面
    metadata_path_with_modify_time.sort_by(|(_, a), (_, b)| b.cmp(a));
    // 从元数据文件中读取Comic
    let downloaded_comics = metadata_path_with_modify_time
        .iter()
        .filter_map(
            |(metadata_path, _)| match Comic::from_metadata(app, metadata_path) {
                Ok(comic) => Some(comic),
                Err(err) => {
                    let err_title = format!("读取元数据文件`{}`失败", metadata_path.display());
                    let string_chain = crate::extensions::AnyhowErrorToStringChain::to_string_chain(&err);
                    tracing::error!(err_title, message = string_chain);
                    None
                }
            },
        )
        .collect::<Vec<_>>();

    tracing::debug!("获取已下载的漫画成功");
    Ok(downloaded_comics)
}

// ════════════════════════════════════════════════════════════════
// 导出
// ════════════════════════════════════════════════════════════════

pub fn export_pdf(app: &AppContext, comic: Comic) -> CommandResult<()> {
    let title = comic.title.clone();
    export::pdf(app, &comic)
        .map_err(|err| CommandError::from(&format!("漫画`{title}`导出pdf失败"), err))?;
    tracing::debug!("漫画`{title}`导出pdf成功");
    Ok(())
}

pub fn export_cbz(app: &AppContext, comic: Comic) -> CommandResult<()> {
    let title = comic.title.clone();
    export::cbz(app, comic)
        .map_err(|err| CommandError::from(&format!("漫画`{title}`导出cbz失败"), err))?;
    tracing::debug!("漫画`{title}`导出cbz成功");
    Ok(())
}

// ════════════════════════════════════════════════════════════════
// 日志
// ════════════════════════════════════════════════════════════════

pub fn get_logs_dir_size(app: &AppContext) -> CommandResult<u64> {
    let logs_dir = app
        .logs_dir()
        .context("获取日志目录失败")
        .map_err(|err| CommandError::from("获取日志目录大小失败", err))?;
    let logs_dir_size = std::fs::read_dir(&logs_dir)
        .context(format!("读取日志目录`{}`失败", logs_dir.display()))
        .map_err(|err| CommandError::from("获取日志目录大小失败", err))?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum::<u64>();
    tracing::debug!("获取日志目录大小成功");
    Ok(logs_dir_size)
}

/// 清空日志目录（Web 版替代桌面版的“打开文件管理器”）。
pub fn clear_logs(app: &AppContext) -> CommandResult<()> {
    let logs_dir = app
        .logs_dir()
        .context("获取日志目录失败")
        .map_err(|err| CommandError::from("清空日志失败", err))?;
    for entry in std::fs::read_dir(&logs_dir)
        .context(format!("读取日志目录`{}`失败", logs_dir.display()))
        .map_err(|err| CommandError::from("清空日志失败", err))?
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.is_file() {
            let _ = std::fs::remove_file(&path);
        }
    }
    tracing::debug!("清空日志成功");
    Ok(())
}

/// 读取日志文件尾部若干行，供 Web 端「查看日志」使用。
pub fn read_logs(app: &AppContext, tail: usize) -> CommandResult<Vec<String>> {
    let logs_dir = app
        .logs_dir()
        .context("获取日志目录失败")
        .map_err(|err| CommandError::from("读取日志失败", err))?;

    // 找出最新的一个日志文件
    let newest = std::fs::read_dir(&logs_dir)
        .context(format!("读取日志目录`{}`失败", logs_dir.display()))
        .map_err(|err| CommandError::from("读取日志失败", err))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .max_by_key(|(_, modified)| *modified);

    let Some((path, _)) = newest else {
        return Ok(Vec::new());
    };

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读取`{}`失败", path.display()))
        .map_err(|err| CommandError::from("读取日志失败", err))?;
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(tail);
    Ok(all[start..].iter().map(|s| (*s).to_string()).collect())
}

// ════════════════════════════════════════════════════════════════
// 封面
// ════════════════════════════════════════════════════════════════

pub async fn get_cover_data(app: &AppContext, cover_url: String) -> CommandResult<Vec<u8>> {
    let wnacg_client = app.wnacg_client();

    let cover_data = wnacg_client
        .get_cover_data(&cover_url)
        .await
        .map_err(|err| CommandError::from("获取封面失败", err))?;
    Ok(cover_data.to_vec())
}

// ════════════════════════════════════════════════════════════════
// 下载整个书架
// ════════════════════════════════════════════════════════════════

/// 下载整个书架：并发拉取所有页，过滤掉已下载的，再逐个创建下载任务。
#[allow(clippy::cast_possible_wrap)]
pub async fn download_shelf(app: &AppContext, shelf_id: i64) -> CommandResult<()> {
    let config = app.config();
    let wnacg_client = app.wnacg_client();
    let download_manager = app.download_manager();

    let mut shelf_comics = Vec::new();
    app.events()
        .emit(crate::event_bus::topics::DOWNLOAD_SHELF, &DownloadShelfEvent::GettingShelfComics);

    // 获取书架第一页
    let first_page = wnacg_client
        .get_shelf(shelf_id, 1)
        .await
        .context("获取书架的第`1`页失败")
        .map_err(|err| CommandError::from("下载书架失败", err))?;
    // 先把书架的第一页放进去
    shelf_comics.extend(first_page.comics);
    let page_count = first_page.total_page;
    // 获取书架剩余页
    let mut join_set = JoinSet::new();
    for page in 2..=page_count {
        let wnacg_client = wnacg_client.clone();
        join_set.spawn(async move {
            let page = wnacg_client
                .get_shelf(shelf_id, page)
                .await
                .context(format!("获取书架的第`{page}`页失败"))?;
            Ok::<_, anyhow::Error>(page)
        });
    }
    // 等待所有请求完成
    while let Some(Ok(get_shelf_result)) = join_set.join_next().await {
        // 如果有请求失败，直接返回错误
        let page = get_shelf_result.map_err(|err| CommandError::from("下载书架失败", err))?;
        shelf_comics.extend(page.comics);
    }
    // 至此，书架的漫画已经全部获取完毕
    // 去掉已下载的漫画
    shelf_comics.retain(|comic| !comic.is_downloaded);
    let total = shelf_comics.len() as i64;

    let interval_ms = config.read().download_shelf_interval_ms;
    for (i, shelf_comic) in shelf_comics.into_iter().enumerate() {
        let comic_title = &shelf_comic.title;
        let comic_id = shelf_comic.id;

        let comic = match wnacg_client
            .get_comic(comic_id)
            .await
            .context(format!("获取ID为`{comic_id}`的漫画失败"))
        {
            Ok(comic) => comic,
            Err(err) => {
                let err_title = format!("下载书架过程中，获取漫画`{comic_title}`失败，已跳过");
                let err = err.context("可能是频率太高，请手动去`配置`里调整`下载书架时，每为一本漫画创建下载任务后休息`");
                let string_chain = crate::extensions::AnyhowErrorToStringChain::to_string_chain(&err);
                tracing::error!(err_title, message = string_chain);
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                continue;
            }
        };

        let current = (i + 1) as i64;
        app.events().emit(
            crate::event_bus::topics::DOWNLOAD_SHELF,
            &DownloadShelfEvent::CreatingDownloadTask { current, total },
        );

        download_manager.create_download_task(comic);
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }

    app.events()
        .emit(crate::event_bus::topics::DOWNLOAD_SHELF, &DownloadShelfEvent::End);

    Ok(())
}

// ════════════════════════════════════════════════════════════════
// 服务器信息
// ════════════════════════════════════════════════════════════════

pub fn get_server_info(app: &AppContext) -> serde_json::Value {
    let version = env!("CARGO_PKG_VERSION");
    let paths = app.paths();
    serde_json::json!({
        "version": version,
        "dataDir": paths.data_dir.display().to_string(),
        "downloadDir": app.download_dir().display().to_string(),
        "logsDir": app.logs_dir().map(|p| p.display().to_string()).unwrap_or_default(),
    })
}

// 让 `utils` 模块被引用（`create_id_to_dir_map` 供同步逻辑使用）
#[allow(dead_code)]
fn _keep_utils_imported(app: &AppContext) {
    let _ = utils::create_id_to_dir_map(app);
    let _ = wnacg_client::WnacgClient::new;
}
