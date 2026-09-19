//! 用 `AppContext` 替代 Tauri 的 `AppHandle`。
//!
//! 原桌面版通过 `app.state::<T>()` / `app.path().app_data_dir()` / `event.emit(&app)`
//! 访问全局资源。Web 版把这些收敛到一个显式、可克隆的上下文对象里，
//! 从而让 `wnacg_client` / `download_manager` / `types` 等核心模块完全脱离 Tauri。
//!
//! 与原版一样采用两段式构造：`AppContext::new` 先建配置和持久化，
//! `init_runtime` 再注入 client / download_manager——因为后两者本身持有
//! `AppContext`，一次性构造会形成循环引用。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use parking_lot::RwLock;

use crate::config::Config;
use crate::download_manager::DownloadManager;
use crate::event_bus::EventBus;
use crate::store::Store;
use crate::wnacg_client::WnacgClient;

/// 运行期路径。全部来自环境变量，Docker 里挂一个卷到 `/data` 即可。
#[derive(Debug, Clone)]
pub struct Paths {
    /// 数据根目录（配置、日志、数据库都在它下面）
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> anyhow::Result<Self> {
        let data_dir = std::env::var("WNACG_DATA_DIR")
            .or_else(|_| std::env::var("JM_DATA_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./data"));
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("创建数据目录 `{}` 失败", data_dir.display()))?;
        Ok(Self { data_dir })
    }

    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("日志")
    }

    /// 下载任务持久化数据库。
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("wnacg_server.db")
    }
}

/// 全局应用上下文。廉价可克隆（内部全是 `Arc`）。
#[derive(Clone)]
pub struct AppContext {
    paths: Paths,
    config: Arc<RwLock<Config>>,
    wnacg_client: Arc<RwLock<Option<WnacgClient>>>,
    download_manager: Arc<RwLock<Option<DownloadManager>>>,
    store: Store,
    events: EventBus,
}

impl AppContext {
    /// 第一阶段构造：只加载配置与路径。
    pub fn new(paths: Paths) -> anyhow::Result<Self> {
        let config = Config::load(&paths.config_path())?;

        // 首次启动时把配置里声明的目录建出来。
        //
        // 桌面版不需要这一步：用户自己选过目录，必然已存在。Web 版是全新
        // 部署，`/data` 卷刚挂上还是空的，若这里不建，"已下载漫画"页面会
        // 直接报 `读取下载目录失败: No such file or directory`——看起来像
        // 服务坏了，其实只是目录没建。导出目录同理。
        for dir in [&config.download_dir, &config.export_dir, &paths.logs_dir()] {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建目录 `{}` 失败", dir.display()))?;
        }

        // 数据库损坏时 `open_or_recover` 会备份旧库并重建空库，
        // 而不是让整个服务起不来。
        let store = Store::open_or_recover(&paths.db_path())?;

        Ok(Self {
            paths,
            config: Arc::new(RwLock::new(config)),
            wnacg_client: Arc::new(RwLock::new(None)),
            download_manager: Arc::new(RwLock::new(None)),
            store,
            events: EventBus::new(),
        })
    }

    /// 第二阶段构造：创建 `WnacgClient` 与 `DownloadManager` 并注入。
    pub fn init_runtime(&self) -> anyhow::Result<()> {
        let client = WnacgClient::new(self.clone());
        *self.wnacg_client.write() = Some(client);

        let manager = DownloadManager::new(self.clone());
        *self.download_manager.write() = Some(manager);

        Ok(())
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn config(&self) -> &Arc<RwLock<Config>> {
        &self.config
    }

    /// 读配置快照。
    pub fn config_read(&self) -> Config {
        self.config.read().clone()
    }

    pub fn save_config(&self, config: &Config) -> anyhow::Result<()> {
        config.save(&self.paths.config_path())?;
        *self.config.write() = config.clone();
        Ok(())
    }

    pub fn events(&self) -> &EventBus {
        &self.events
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 取 `WnacgClient`。初始化后必然存在。
    pub fn wnacg_client(&self) -> WnacgClient {
        self.wnacg_client
            .read()
            .clone()
            .expect("WnacgClient 尚未初始化")
    }

    /// 取 `DownloadManager`。初始化后必然存在。
    pub fn download_manager(&self) -> DownloadManager {
        self.download_manager
            .read()
            .clone()
            .expect("DownloadManager 尚未初始化")
    }

    /// 配置变更后重建 HTTP 客户端（代理/API 地址变了需要重建）。
    pub fn reload_wnacg_client(&self) {
        self.wnacg_client().reload_client();
    }

    /// 配置变更后应用新的下载并发度。
    ///
    /// 与 jmcomic-web 同样的修复：不要 `shutdown()` 后重建 manager，
    /// 那样已 spawn 的任务仍持有旧 permit，实际并发会翻倍。
    /// 改为在同一个 manager 上原地调整 permit。
    pub fn reload_download_manager(&self) -> anyhow::Result<()> {
        let (comic_concurrency, img_concurrency) = {
            let config = self.config.read();
            (config.comic_concurrency, config.img_concurrency)
        };

        if let Some(manager) = self.download_manager.read().clone() {
            manager.update_concurrency(comic_concurrency, img_concurrency);
        }

        Ok(())
    }

    pub fn download_dir(&self) -> PathBuf {
        self.config.read().download_dir.clone()
    }

    pub fn logs_dir(&self) -> anyhow::Result<PathBuf> {
        Ok(self.paths.logs_dir())
    }
}

/// 让 `&Path` 上的 join 更顺手。
pub fn join(base: &Path, child: impl AsRef<Path>) -> PathBuf {
    base.join(child.as_ref())
}