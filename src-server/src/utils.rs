//! 与业务无关的小工具函数。

use anyhow::Context;
use walkdir::WalkDir;

use crate::{
    context::AppContext,
    extensions::{AppContextExt, WalkDirEntryExt},
    types::Comic,
};

/// 过滤文件/目录名里的非法字符（与原桌面版完全一致）。
pub fn filename_filter(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' | '/' | '\n' => ' ',
            ':' => '：',
            '*' => '⭐',
            '?' => '？',
            '"' => '\'',
            '<' => '《',
            '>' => '》',
            '|' => '丨',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .trim()
        .to_string()
}

/// 按 id 取漫画详情（含图片列表）。
pub async fn get_comic(app: &AppContext, comic_id: i64) -> anyhow::Result<Comic> {
    let img_list = app
        .get_wnacg_client()
        .get_img_list(comic_id)
        .await
        .context("获取图片列表失败")?;
    // `WnacgClient::get_comic` 内部已经完成 HTML 解析并返回 `Comic`，
    // 这里只需把单独取回的 `img_list` 合进去，不能再走一次 `from_html`。
    let mut comic = app
        .get_wnacg_client()
        .get_comic(comic_id)
        .await
        .context("获取漫画详情失败")?;
    comic.img_list = img_list;
    Ok(comic)
}

/// 扫描下载目录，建立 `漫画id -> 目录` 的映射。
///
/// 走的是每本漫画目录下的 `元数据.json`，所以即使用户改了目录命名格式，
/// 也能正确定位已下载的漫画。
pub fn create_id_to_dir_map(app: &AppContext) -> anyhow::Result<std::collections::HashMap<i64, std::path::PathBuf>> {
    let mut id_to_dir_map = std::collections::HashMap::new();
    let download_dir = app.get_config().read().download_dir.clone();
    if !download_dir.exists() {
        return Ok(id_to_dir_map);
    }

    for entry in WalkDir::new(&download_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !entry.is_comic_metadata() {
            continue;
        }

        let metadata_str =
            std::fs::read_to_string(path).context(format!("读取`{}`失败", path.display()))?;
        let comic_json: serde_json::Value = serde_json::from_str(&metadata_str)
            .context(format!("将`{}`反序列化为serde_json::Value失败", path.display()))?;
        let id = comic_json
            .get("id")
            .and_then(|id| id.as_i64())
            .context(format!("`{path:?}`没有`id`字段"))?;

        let parent = path
            .parent()
            .context(format!("`{}`没有父目录", path.display()))?;

        id_to_dir_map.entry(id).or_insert(parent.to_path_buf());
    }

    Ok(id_to_dir_map)
}