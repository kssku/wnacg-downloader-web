//! 进程内事件总线。
//!
//! 替代 Tauri 的 `app.emit()`。每个 WebSocket 连接在建立时 `subscribe()` 拿到一个
//! `broadcast::Receiver`，此后所有 `emit()` 出来的事件都会推给它。
//!
//! 用 `tokio::sync::broadcast` 而不是 `mpsc`：事件是「一对多」的广播语义，
//! 而且 slow consumer 只会丢自己的消息（`RecvError::Lagged`），不会拖垮发布方。

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::broadcast;

/// 事件主题常量。前端按 `topic` 分发到不同的处理函数。
pub mod topics {
    pub const DOWNLOAD_TASK: &str = "download_task";
    /// 任务被移除（前端把卡片删掉）。与 `DOWNLOAD_TASK` 分开，
    /// 避免前端在处理状态事件时还要区分「这是删除」这种特殊载荷。
    pub const DOWNLOAD_TASK_DELETED: &str = "download_task_deleted";
    pub const DOWNLOAD_SPEED: &str = "download_speed";
    pub const DOWNLOAD_SLEEPING: &str = "download_sleeping";
    pub const EXPORT_PDF: &str = "export_pdf";
    pub const EXPORT_CBZ: &str = "export_cbz";
    pub const DOWNLOAD_SHELF: &str = "download_shelf";
    pub const LOG: &str = "log";
    pub const AUTH: &str = "auth";
    pub const CONFIG_CHANGED: &str = "config_changed";
}

/// 一条待广播的消息。`topic` 决定前端的分发分支，`payload` 是已经序列化好的 JSON。
#[derive(Debug, Clone, Serialize)]
pub struct BusMessage {
    pub topic: String,
    pub payload: serde_json::Value,
}

/// 事件总线。克隆是廉价的（内部是 `Arc`）。
#[derive(Clone)]
pub struct EventBus {
    tx: Arc<broadcast::Sender<BusMessage>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    /// 容量 1024 条。订阅者跟不上时会收到 `Lagged`，我们选择跳过丢失的消息
    /// 而不是断开连接——前端丢几条速度事件无关紧要。
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(1024);
        Self { tx: Arc::new(tx) }
    }

    /// 订阅。每个 WebSocket 连接调用一次。
    pub fn subscribe(&self) -> broadcast::Receiver<BusMessage> {
        self.tx.subscribe()
    }

    /// 广播一条事件。没有订阅者时静默丢弃（服务刚启动、前端还没连上）。
    pub fn emit<T: Serialize>(&self, topic: &str, payload: &T) {
        let payload = match serde_json::to_value(payload) {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(topic, err = %err, "事件序列化失败，已丢弃");
                return;
            }
        };
        // send 返回 Err 表示当前没有任何订阅者，属于正常情况。
        let _ = self.tx.send(BusMessage {
            topic: topic.to_string(),
            payload,
        });
    }

    /// 当前订阅者数量，仅用于日志/调试。
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}