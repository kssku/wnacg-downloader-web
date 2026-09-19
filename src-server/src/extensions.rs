//! 原桌面版的 extensions.rs 把 `AppHandleExt` 挂到 `tauri::AppHandle` 上，
//! 通过 `app.state::<T>()` 取全局状态。Web 版改为直接挂在 `AppContext` 上，
//! 语义完全一致，只是取状态的方式从 Tauri 的 State 换成了显式字段。

use anyhow::anyhow;
use scraper::error::SelectorErrorKind;

use crate::config::ProxyMode;
use crate::context::AppContext;
use crate::types::{Comic, GetShelfResult, ImgList, SearchResult, UserProfile};

/// 给 `reqwest::ClientBuilder` 挂上代理配置。
///
/// 原桌面版从 Tauri 配置读代理，Web 版改成读服务端 `Config`：
///
/// - `ProxyMode::NoProxy` —— 显式不走代理（连环境变量都忽略）；
/// - `ProxyMode::System`  —— 交给 reqwest 自己读 `HTTP(S)_PROXY`；
/// - `ProxyMode::Custom`  —— 用配置里的 `proxy_host:proxy_port`。
///
/// `label` 只用于日志，方便区分是 API 客户端还是图片客户端没走通代理。
pub trait ClientBuilderExt {
    fn set_proxy(self, app: &AppContext, label: &str) -> Self;
}

impl ClientBuilderExt for reqwest::ClientBuilder {
    fn set_proxy(self, app: &AppContext, label: &str) -> Self {
        let (mode, host, port) = {
            let config = app.config().read();
            (config.proxy_mode, config.proxy_host.clone(), config.proxy_port)
        };

        match mode {
            ProxyMode::NoProxy => {
                tracing::debug!("[{label}] 代理模式：直连（忽略环境变量）");
                self.no_proxy()
            }
            ProxyMode::System => {
                tracing::debug!("[{label}] 代理模式：跟随系统环境变量");
                self
            }
            ProxyMode::Custom => {
                let url = format!("http://{host}:{port}");
                match reqwest::Proxy::all(&url) {
                    Ok(proxy) => {
                        tracing::debug!("[{label}] 代理模式：手动 {url}");
                        self.proxy(proxy)
                    }
                    Err(err) => {
                        // 代理串写错不该让整个客户端构建失败——退化成直连并留日志，
                        // 用户还能进网页改配置。
                        tracing::error!("[{label}] 代理地址 `{url}` 无效，改为直连: {err}");
                        self.no_proxy()
                    }
                }
            }
        }
    }
}

pub trait AnyhowErrorToStringChain {
    /// 将 `anyhow::Error` 转换为 chain 格式
    /// # Example
    /// 0: error message
    /// 1: error message
    /// 2: error message
    fn to_string_chain(&self) -> String;
}

impl AnyhowErrorToStringChain for anyhow::Error {
    fn to_string_chain(&self) -> String {
        use std::fmt::Write;
        self.chain()
            .enumerate()
            .fold(String::new(), |mut output, (i, e)| {
                let _ = writeln!(output, "{i}: {e}");
                output
            })
    }
}

pub trait ToAnyhow<T> {
    fn to_anyhow(self) -> anyhow::Result<T>;
}

impl<T> ToAnyhow<T> for Result<T, SelectorErrorKind<'_>> {
    fn to_anyhow(self) -> anyhow::Result<T> {
        self.map_err(|e| anyhow!(e.to_string()))
    }
}

/// 取全局资源的快捷方法。原桌面版是 `AppHandleExt`，这里换成 `AppContextExt`。
pub trait AppContextExt {
    fn get_config(&self) -> std::sync::Arc<parking_lot::RwLock<crate::config::Config>>;
    fn get_wnacg_client(&self) -> crate::wnacg_client::WnacgClient;
    fn get_download_manager(&self) -> crate::download_manager::DownloadManager;
}

impl AppContextExt for AppContext {
    fn get_config(&self) -> std::sync::Arc<parking_lot::RwLock<crate::config::Config>> {
        self.config().clone()
    }

    fn get_wnacg_client(&self) -> crate::wnacg_client::WnacgClient {
        self.wnacg_client()
    }

    fn get_download_manager(&self) -> crate::download_manager::DownloadManager {
        self.download_manager()
    }
}

pub trait PathIsImg {
    /// 判断路径是否为图片(jpg/png/webp/gif)
    fn is_img(&self) -> bool;

    /// 判断路径是否为普通图片(jpg/png/webp)
    fn is_common_img(&self) -> bool;
}

impl PathIsImg for std::path::Path {
    fn is_img(&self) -> bool {
        self.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_lowercase)
            .is_some_and(|ext| matches!(ext.as_str(), "jpg" | "png" | "webp" | "gif"))
    }

    fn is_common_img(&self) -> bool {
        self.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_lowercase)
            .is_some_and(|ext| matches!(ext.as_str(), "jpg" | "png" | "webp"))
    }
}

/// 判断一个 walkdir 条目是否为漫画目录下的 `元数据.json`。
pub trait WalkDirEntryExt {
    fn is_comic_metadata(&self) -> bool;
}

impl WalkDirEntryExt for walkdir::DirEntry {
    fn is_comic_metadata(&self) -> bool {
        self.file_type().is_file()
            && self.file_name().to_str() == Some("元数据.json")
    }
}

/// 把 HTML 解析入口挂到各类型上。原桌面版签名是 `(&AppHandle, &str, ...)`，
/// Web 版换成 `(&AppContext, &str, ...)`。
pub trait FromHtmlExt {
    fn parse_comic(app: &AppContext, html: &str, img_list: ImgList) -> anyhow::Result<Comic>;
    fn parse_search(
        app: &AppContext,
        html: &str,
        is_search_by_tag: bool,
    ) -> anyhow::Result<SearchResult>;
    fn parse_shelf(app: &AppContext, html: &str) -> anyhow::Result<GetShelfResult>;
    fn parse_user_profile(app: &AppContext, html: &str) -> anyhow::Result<UserProfile>;
}

impl FromHtmlExt for Comic {
    fn parse_comic(app: &AppContext, html: &str, img_list: ImgList) -> anyhow::Result<Comic> {
        Comic::from_html(app, html, img_list)
    }

    fn parse_search(
        app: &AppContext,
        html: &str,
        is_search_by_tag: bool,
    ) -> anyhow::Result<SearchResult> {
        SearchResult::from_html(app, html, is_search_by_tag)
    }

    fn parse_shelf(app: &AppContext, html: &str) -> anyhow::Result<GetShelfResult> {
        GetShelfResult::from_html(app, html)
    }

    fn parse_user_profile(app: &AppContext, html: &str) -> anyhow::Result<UserProfile> {
        UserProfile::from_html(app, html)
    }
}