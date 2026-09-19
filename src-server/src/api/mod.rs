//! HTTP API 层。
//!
//! - `commands`：从原 `src-tauri/src/commands.rs` 移植过来的业务函数。
//! - `routes`：把命令挂到 REST 端点上。
//! - `ws`：WebSocket 事件推送，替代原桌面版的 `tauri::event`。
//! - `error`：把 `CommandError` 适配成 axum 的响应。

pub mod commands;
pub mod error;
pub mod routes;
pub mod ws;
