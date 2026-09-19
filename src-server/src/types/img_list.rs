use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

/// 漫画的图片列表。包装 `Vec<ImgInImgList>`，保留 `Deref` / `IntoIterator`，
/// 这样上层既可以当切片用，也可以直接 `for img in img_list`。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImgList(pub Vec<ImgInImgList>);

impl Deref for ImgList {
    type Target = Vec<ImgInImgList>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ImgList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl IntoIterator for ImgList {
    type Item = ImgInImgList;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// 单张图片。序列化名与原桌面版的 `元数据.json` 保持一致。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::module_name_repetitions)]
pub struct ImgInImgList {
    /// 图片标题（`[01]`、`[001]`，根据漫画总页数确定）
    pub caption: String,
    /// 图片 url（`//img5.wnimg.ru/data/2826/33/01.jpg`，缺 `https:` 前缀）。
    /// 最后一张图片为 `/themes/weitu/images/bg/shoucang.jpg`，记得过滤。
    pub url: String,
}