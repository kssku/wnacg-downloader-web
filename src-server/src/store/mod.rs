//! 持久化层：把下载任务状态从进程内存搬到 SQLite。
//!
//! 这一层的存在是为了解决两个根本问题：
//!
//! 1. **任务状态纯内存** —— 容器一重启，所有 `Pending` / `Downloading`
//!    任务凭空消失，用户不知道哪些下了哪些没下。
//! 2. **漫画完整性判定过粗** —— 原先靠「已下载张数 == 总张数」判断漫画是否
//!    成功，导致单张图超时要整本重下。有了图片级的 `download_image` 表，
//!    恢复时只需要重下 `state != 'done'` 的图片。
//!
//! 设计约束：
//!
//! - **不引入外部组件**。NAS 上多一个 Redis / 消息队列就多一个故障点，
//!   而 SQLite 已经在青龙侧跑着，运维熟悉。
//! - **单写连接 + WAL**。漫画并发 2 + 图片并发 10，写操作全部集中在状态迁移点，
//!   不在图片下载热路径上逐张写盘。
//! - **数据库文件独立**（`WNACG_DATA_DIR/wnacg_server.db`），**不与青龙的
//!   `wnacg.db` 混用**，避免两套 schema 互相干扰。

pub mod migrations;
pub mod repo;
pub mod types;

pub use repo::{ImageRepo, TaskRepo, TaskStats};
pub use types::{DbImage, DbImageState, DbTask, DbTaskState, Store};
