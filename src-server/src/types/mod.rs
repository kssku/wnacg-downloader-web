//! 与原桌面版一一对应的数据类型。
//!
//! 相对桌面版的唯一改动：`from_html` 系列不再接收 `tauri::AppHandle`，
//! 而是接收 `&AppContext`（用于读配置里的下载目录与 API 域名）。

mod comic;
mod comic_info;
mod download_format;
mod get_shelf_result;
mod img_list;
mod log_level;
mod search_result;
mod tag;
mod user_profile;

pub use comic::*;
pub use comic_info::*;
pub use download_format::*;
pub use get_shelf_result::*;
pub use img_list::*;
pub use log_level::*;
pub use search_result::*;
pub use tag::*;
pub use user_profile::*;