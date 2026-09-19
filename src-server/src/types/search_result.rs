use anyhow::Context;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};

use crate::{
    context::AppContext,
    extensions::{AppContextExt, ToAnyhow},
    utils::filename_filter,
};

/// 搜索结果页。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    comics: Vec<ComicInSearch>,
    current_page: i64,
    total_page: i64,
    is_search_by_tag: bool,
}

impl SearchResult {
    /// `is_search_by_tag` 影响总页数的算法：
    /// 标签搜索页有分页器可直接读末页；关键词搜索页只有「共 N 条结果」，得自己按每页 24 条算。
    pub fn from_html(
        ctx: &AppContext,
        html: &str,
        is_search_by_tag: bool,
    ) -> anyhow::Result<SearchResult> {
        let document = Html::parse_document(html);
        let comic_li_selector = Selector::parse(".li.gallary_item").to_anyhow()?;

        let mut comics = Vec::new();
        for comic_li in document.select(&comic_li_selector) {
            let comic = ComicInSearch::from_li(ctx, &comic_li)?;
            comics.push(comic);
        }

        let current_page = match document
            .select(&Selector::parse(".thispage").to_anyhow()?)
            .next()
        {
            Some(span) => {
                let span_html = span.html();
                span.text()
                    .next()
                    .context(format!("没有在当前页码的<span>中找到文本: {span_html}"))?
                    .parse::<i64>()
                    .context(format!("当前页码不是整数: {span_html}"))?
            }
            None => 1,
        };

        let total_page = if is_search_by_tag {
            match document
                .select(&Selector::parse(".f_left.paginator > a").to_anyhow()?)
                .next_back()
            {
                Some(a) => {
                    let a_html = a.html();
                    a.text()
                        .next()
                        .context(format!("没有在最后一页的<a>中找到文本: {a_html}"))?
                        .parse::<i64>()
                        .context(format!("最后一页不是整数: {a_html}"))?
                        .max(current_page) // 如果是最后一页，那么当前页码就是最后一页
                }
                None => 1,
            }
        } else {
            const PAGE_SIZE: i64 = 24;
            let document_html = document.html();

            let b = document
                .select(&Selector::parse("#bodywrap .result > b").to_anyhow()?)
                .next()
                .context(format!("没有找到总结果数的<b>: {document_html}"))?;
            let b_html = b.html();

            let total = b
                .text()
                .next()
                .context(format!("没有在总结果数的<b>中找到文本: {b_html}"))?
                .replace(',', "")
                .parse::<i64>()
                .context(format!("总结果数不是整数: {b_html}"))?;
            (total + PAGE_SIZE - 1) / PAGE_SIZE
        };

        Ok(SearchResult {
            comics,
            current_page,
            total_page,
            is_search_by_tag,
        })
    }
}

/// 搜索结果里的一条漫画。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComicInSearch {
    /// 漫画 id
    id: i64,
    /// 漫画标题（带 html 标签，用于高亮匹配到的关键词）
    title_html: String,
    /// 漫画标题（纯文本，已过滤非法字符）
    title: String,
    /// 封面链接
    cover: String,
    /// 额外信息（`209張圖片， 創建於2025-01-05 18:33:19`）
    additional_info: String,
    /// 是否已下载
    is_downloaded: bool,
}

impl ComicInSearch {
    pub fn from_li(ctx: &AppContext, li: &ElementRef) -> anyhow::Result<ComicInSearch> {
        let li_html = li.html();

        let title_a = li
            .select(&Selector::parse(".title > a").to_anyhow()?)
            .next()
            .context(format!("没有在<li>中找到标题的<a>: {li_html}"))?;
        let title_a_html = title_a.html();

        let id = title_a
            .attr("href")
            .context(format!("没有在标题的<a>中找到href属性: {title_a_html}"))?
            .strip_prefix("/photos-index-aid-")
            .context(format!(
                "href不是以`/photos-index-aid-`开头: {title_a_html}"
            ))?
            .strip_suffix(".html")
            .context(format!("href不是以`.html`结尾: {title_a_html}"))?
            .parse::<i64>()
            .context(format!("id不是整数: {title_a_html}"))?;

        let title_html = title_a
            .attr("title")
            .context(format!("没有在标题的<a>中找到title属性: {title_a_html}"))?
            .trim()
            .to_string();

        let title = title_a.text().collect::<String>();
        let title = filename_filter(&title);

        let img = li
            .select(&Selector::parse("img").to_anyhow()?)
            .next()
            .context(format!("没有在<li>中找到<img>: {li_html}"))?;
        let img_html = img.html();

        let cover_src = img
            .attr("src")
            .context(format!("没有在<img>中找到src属性: {img_html}"))?;
        let cover = format!("https:{cover_src}");

        let div = li
            .select(&Selector::parse(".info_col").to_anyhow()?)
            .next()
            .context(format!("没有在<li>中找到额外信息的<div>: {li_html}"))?;
        let div_html = div.html();

        let additional_info = div
            .text()
            .next()
            .context(format!("没有在额外信息的<div>中找到文本: {div_html}"))?
            .trim()
            .to_string();

        let is_downloaded = ctx.get_config().read().download_dir.join(&title).exists();

        Ok(ComicInSearch {
            id,
            title_html,
            title,
            cover,
            additional_info,
            is_downloaded,
        })
    }
}