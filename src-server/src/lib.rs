//! wnacg-downloader Web 服务端
//!
//! 该 crate 以 jmcomic-downloader-web / picacomic-downloader-web 的服务端为模板，
//! 把原 Tauri 桌面版的核心逻辑（客户端、下载管理器、导出）搬到一个独立的
//! HTTP + WebSocket 服务里，方便在 NAS 上以 Docker 方式部署，通过网页后台控制。

pub mod api;
pub mod auth;
pub mod config;
pub mod context;
pub mod download_manager;
pub mod errors;
pub mod event_bus;
pub mod events;
pub mod export;
pub mod extensions;
pub mod logger;
pub mod store;
pub mod types;
pub mod utils;
pub mod wnacg_client;