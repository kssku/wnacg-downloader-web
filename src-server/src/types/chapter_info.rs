use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 章节信息。
///
/// 对齐 jmcomic/picacomic 的领域模型形状（`chapter_id` / `chapter_title` /
/// `order` / `is_downloaded` / `chapter_download_dir`）。wnacg 站点为
/// **单本无章节**结构，因此实际只会有一个合成章节，`chapter_id` 恒等于
/// `comic.id`。
///
/// 注意：`chapter_download_dir` 在 wnacg 中**恒为 `None`**，
/// 因为 wnacg 的下载目录名由 `download_dir.join(comic.title)` 直接拼出，
/// 不经过本字段。保留它是为了形状对齐，**不是路径计算来源**。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterInfo {
    /// 章节 id。wnacg 无章节层，此值恒等于 `comic.id`。
    pub chapter_id: String,
    /// 章节标题。wnacg 无章节层，此值恒等于 `comic.title`。
    pub chapter_title: String,
    /// 章节序号。wnacg 恒定单章节，此值为 `1`。
    pub order: i64,
    /// 是否已下载。`None` 表示「未知」，序列化时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_downloaded: Option<bool>,
    /// 章节下载目录。wnacg 恒为 `None`——目录名不走本字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter_download_dir: Option<PathBuf>,
}