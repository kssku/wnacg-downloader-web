use std::path::Path;

use anyhow::Context;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

use crate::{
    context::AppContext,
    extensions::{AppContextExt, ToAnyhow},
    utils::filename_filter,
};

use super::{ChapterInfo, ImgList, Tag};

/// 漫画详情。也是写出 `元数据.json` 的结构，所以字段必须保持可反序列化。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
pub struct Comic {
    /// 漫画 id。
    ///
    /// 对齐 jmcomic/picacomic 的领域模型：它们的 id 都是 `String`。
    /// wnacg 的 id 取自详情页 `<link href="/feed-index-aid-<id>.html">`，
    /// 本身就是数字字符串，不再 parse 成 `i64`。
    pub id: String,
    /// 漫画标题
    pub title: String,
    /// 封面链接
    pub cover: String,
    /// 分类
    pub category: String,
    /// 漫画有多少张图片
    pub image_count: i64,
    /// 标签
    pub tags: Vec<Tag>,
    /// 简介
    pub intro: String,
    /// 是否已下载。`None` 表示「未知」，序列化时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_downloaded: Option<bool>,
    /// wnacg 站点为单本无章节结构，此处合成恒定单元素列表以对齐
    /// jmcomic/picacomic 的领域模型形状（前端可统一按「章节」渲染）。
    ///
    /// 注意：本字段**不参与目录生成**——wnacg 的下载目录名由
    /// `download_dir.join(comic.title)` 直接拼出，不走 `chapter_download_dir`。
    /// 保留它是为了形状对齐，不是路径计算来源。
    ///
    /// `#[serde(default)]` 保证旧 `元数据.json`（不含本字段）仍可反序列化。
    #[serde(default)]
    pub chapter_infos: Vec<ChapterInfo>,
    /// 图片列表
    pub img_list: ImgList,
}

impl Comic {
    /// 从漫画详情页 HTML 解析。
    ///
    /// `img_list` 由 `WnacgClient::get_img_list` 单独取回后传入，因为图片列表
    /// 藏在页面里的一段 JS 变量中，与详情字段的解析路径不同。
    #[allow(clippy::too_many_lines)]
    pub fn from_html(ctx: &AppContext, html: &str, img_list: ImgList) -> anyhow::Result<Comic> {
        let document = Html::parse_document(html);
        let document_html = document.html();

        let link = document
            .select(&Selector::parse("head > link").to_anyhow()?)
            .next()
            .context(format!("没有找到漫画id的<link>: {document_html}"))?;
        let link_html = link.html();

        let id = link
            .attr("href")
            .context(format!("漫画id的<link>没有href属性: {link_html}"))?
            .strip_prefix("/feed-index-aid-")
            .context(format!(
                "漫画id的<link>不是以`/feed-index-aid-`开头: {link_html}"
            ))?
            .strip_suffix(".html")
            .context(format!("漫画id的<link>不是以`.html`结尾: {link_html}"))?
            .to_string();

        let h2 = document
            .select(&Selector::parse("#bodywrap > h2").to_anyhow()?)
            .next()
            .context(format!("没有找到漫画标题的<h2>: {document_html}"))?;
        let h2_html = h2.html();

        let title = h2
            .text()
            .next()
            .context(format!("漫画标题的<h2>没有文本: {h2_html}"))?;
        let title = filename_filter(title);

        let img = document
            .select(&Selector::parse(".asTBcell.uwthumb > img").to_anyhow()?)
            .next()
            .context(format!("没有找到封面的<img>: {document_html}"))?;
        let img_html = img.html();

        let cover_src = img
            .attr("src")
            .context(format!("封面的<img>没有src属性: {img_html}"))?
            .trim_start_matches('/')
            .to_string();
        let cover = format!("https://{cover_src}");

        let label = document
            .select(&Selector::parse(".asTBcell.uwconn > label").to_anyhow()?)
            .next()
            .context(format!("没有找到分类的<label>: {document_html}"))?;
        let label_html = label.html();

        let category = label
            .text()
            .next()
            .context(format!("分类的<label>没有文本: {label_html}"))?
            .strip_prefix("分類：")
            .context(format!("分类<label>的文本不是以`分類：`开头: {label_html}"))?
            .to_string();

        let label = document
            .select(&Selector::parse(".asTBcell.uwconn > label").to_anyhow()?)
            .nth(1)
            .context(format!("没有找到图片数量的<label>: {document_html}"))?;
        let label_html = label.html();

        let image_count = label
            .text()
            .next()
            .context(format!("图片数量的<label>没有文本: {label_html}"))?
            .strip_prefix("頁數：")
            .context(format!("图片数量的文本不是以`頁數：`开头: {label_html}"))?
            .strip_suffix("P")
            .context(format!("图片数量的文本不是以`P`结尾: {label_html}"))?
            .parse::<i64>()
            .context(format!("图片数量不是整数: {label_html}"))?;

        let api_domain = ctx.get_config().read().get_api_domain();
        let mut tags = vec![];
        let tag_selector = Selector::parse(".tagshow").to_anyhow()?;
        for a in document.select(&tag_selector) {
            let Some(text) = a.text().next() else {
                // 有些标签的 <a> 没有文本，跳过这些标签
                continue;
            };
            let name = text.trim().to_string();

            let a_html = a.html();
            let href = a
                .attr("href")
                .context(format!("标签的<a>没有href属性: {a_html}"))?
                .to_string();
            let url = format!("https://{api_domain}{href}");
            tags.push(Tag { name, url });
        }

        let intro = document
            .select(&Selector::parse(".asTBcell.uwconn > p").to_anyhow()?)
            .next()
            .context(format!("没有找到简介的<p>: {document_html}"))?
            .html();

        let is_downloaded = ctx.get_config().read().download_dir.join(&title).exists();
        let is_downloaded = Some(is_downloaded);

        // wnacg 站点为单本无章节结构，此处合成恒定单元素列表以对齐
        // jmcomic/picacomic 的领域模型形状。`chapter_id` 恒等于 `id`。
        // 该列表**不参与目录生成**（见 `Comic::chapter_infos` 的文档注释）。
        let chapter_infos = vec![ChapterInfo {
            chapter_id: id.clone(),
            chapter_title: title.clone(),
            order: 1,
            is_downloaded: None,
            chapter_download_dir: None,
        }];

        Ok(Comic {
            id,
            title,
            cover,
            category,
            image_count,
            tags,
            intro,
            is_downloaded,
            chapter_infos,
            img_list,
        })
    }

    /// 从已下载目录里的 `元数据.json` 还原。
    ///
    /// `is_downloaded` 在文件里存的是 `None`（写元数据时还没落盘完），
    /// 所以这里要按当前配置的下载目录重新算一遍。
    pub fn from_metadata(ctx: &AppContext, metadata_path: &Path) -> anyhow::Result<Comic> {
        let comic_json = std::fs::read_to_string(metadata_path).context(format!(
            "从元数据转为Comic失败，读取元数据文件`{}`失败",
            metadata_path.display()
        ))?;
        let mut comic = serde_json::from_str::<Comic>(&comic_json).context(format!(
            "从元数据转为Comic失败，将`{}`反序列化为Comic失败",
            metadata_path.display()
        ))?;

        let is_downloaded = ctx
            .get_config()
            .read()
            .download_dir
            .join(&comic.title)
            .exists();
        comic.is_downloaded = Some(is_downloaded);

        // 旧版 `元数据.json` 不含 `chapterInfos`（`#[serde(default)]` 让它反序列化成
        // 空列表）。这里补上合成章节，保证内存中的 `Comic` 形状始终一致——
        // 下游（如 `DownloadTask::new` 取 `chapter_infos[0]`）不必再判空。
        if comic.chapter_infos.is_empty() {
            comic.chapter_infos = vec![ChapterInfo {
                chapter_id: comic.id.clone(),
                chapter_title: comic.title.clone(),
                order: 1,
                is_downloaded: None,
                chapter_download_dir: None,
            }];
        }

        Ok(comic)
    }
}

#[cfg(test)]
mod tests {
    use super::Comic;

    /// 旧版 `元数据.json`（0c 之前写出）不含 `chapterInfos` 字段。
    /// `#[serde(default)]` 必须让它反序列化成功，且得到空列表。
    #[test]
    fn legacy_metadata_without_chapter_infos_deserializes() {
        let legacy = r#"{"id":"888001","title":"旧格式","cover":"","category":"",
            "imageCount":1,"tags":[],"intro":"",
            "imgList":[{"caption":"[01]","url":"//x/1.jpg"}]}"#;
        let comic: Comic = serde_json::from_str(legacy).expect("旧元数据应能反序列化");
        assert_eq!(comic.id, "888001");
        assert!(
            comic.chapter_infos.is_empty(),
            "缺省时应为空列表（由 Comic::from_metadata 补合成章节）"
        );
    }

    /// 新格式（含 `chapterInfos`）应原样反序列化。
    #[test]
    fn new_metadata_with_chapter_infos_roundtrips() {
        let json = r#"{"id":"888002","title":"新格式","cover":"","category":"",
            "imageCount":1,"tags":[],"intro":"",
            "chapterInfos":[{"chapterId":"888002","chapterTitle":"新格式","order":1}],
            "imgList":[{"caption":"[01]","url":"//x/1.jpg"}]}"#;
        let comic: Comic = serde_json::from_str(json).expect("新元数据应能反序列化");
        assert_eq!(comic.chapter_infos.len(), 1);
        assert_eq!(comic.chapter_infos[0].chapter_id, "888002");
        assert_eq!(comic.chapter_infos[0].order, 1);
    }
}
