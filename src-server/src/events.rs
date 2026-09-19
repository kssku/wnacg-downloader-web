//! 事件定义。
//!
//! 原桌面版这些结构体派生 `tauri_specta::Event`，由 Tauri 负责 emit 到前端。
//! Web 版只保留 `Serialize`/`Deserialize`，由 `EventBus` 广播到所有 WebSocket 订阅者。
//! 字段名与前端 `bindings.ts` 一一对应，因此前端逻辑可以照搬。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::download_manager::DownloadTaskState;
use crate::types::{Comic, DownloadFormat, LogLevel, UserProfile};

/// 下载任务状态快照。
///
/// 与原桌面版 `DownloadTaskEvent` 完全同构（前端 `bindings.ts` 里就是
/// `{ state; comic; downloadedImgCount; totalImgCount }`），因此前端可以直接复用。
///
/// Web 版把「快照」和「事件」合并成同一种载荷：
/// - 每个任务状态变化时，推一条当前任务的快照；
/// - WebSocket 刚连上时，一次性推一个「全部任务快照」的列表（见 `ws.rs`）。
///
/// 删除任务在桌面版不存在，这里额外用 [`DownloadTaskDeletedEvent`] 表达。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTaskEvent {
    pub state: DownloadTaskState,
    pub comic: Comic,
    pub downloaded_img_count: u32,
    pub total_img_count: u32,
}

/// 任务被删除事件。桌面版没有这个概念，Web 版需要它让前端把卡片移除。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTaskDeletedEvent {
    pub comic_id: i64,
}

/// 下载速度事件（每秒一次）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSpeedEvent {
    pub speed: String,
}

/// 「下载完成后的休息时间」倒计时事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSleepingEvent {
    pub comic_id: i64,
    pub remaining_sec: u64,
}

/// PDF 导出进度事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum ExportPdfEvent {
    #[serde(rename_all = "camelCase")]
    Start { uuid: String, title: String },

    #[serde(rename_all = "camelCase")]
    End { uuid: String },
}

/// CBZ 导出进度事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum ExportCbzEvent {
    #[serde(rename_all = "camelCase")]
    Start { uuid: String, title: String },

    #[serde(rename_all = "camelCase")]
    End { uuid: String },
}

/// 一键下载书架进度事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum DownloadShelfEvent {
    #[serde(rename_all = "camelCase")]
    GettingShelfComics,

    #[serde(rename_all = "camelCase")]
    CreatingDownloadTask { current: i64, total: i64 },

    #[serde(rename_all = "camelCase")]
    End,
}

/// 后端日志事件。
///
/// 由 `logger.rs` 里的 `LogEventWriter` 产生，前端订阅后显示在日志面板。
///
/// **字段必须与 tracing 的 JSON 输出逐字对应**：`LogEventWriter` 直接
/// `serde_json::from_str` 反序列化 tracing 写出的那一行 JSON，字段少一个
/// 就会解析失败（整个日志面板收不到任何日志）。前端
/// `LogDialog.tsx::formatLogEvent` 也按这个形状渲染，两边都不能改。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    pub timestamp: String,
    pub level: LogLevel,
    /// tracing 的结构化字段，`message`、`err_title` 等都在这里。
    pub fields: HashMap<String, serde_json::Value>,
    pub target: String,
    pub filename: String,
    #[serde(rename = "line_number")]
    pub line_number: i64,
}

/// 登录状态变化事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthEvent {
    pub logged_in: bool,
    pub user_profile: Option<UserProfile>,
}

/// 配置变更事件。前端收到后重新拉取配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChangedEvent {
    pub download_format: DownloadFormat,
}
