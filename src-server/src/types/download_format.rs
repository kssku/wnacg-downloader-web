use image::ImageFormat;
use serde::{Deserialize, Serialize};

/// 下载图片的保存格式。
///
/// `Original` 表示保留站点的原始字节流（包括 gif），不做转码。
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadFormat {
    #[default]
    Jpeg,
    Png,
    Webp,
    Original,
}

impl DownloadFormat {
    /// 转换后应使用的扩展名。`Original` 返回 `None`，表示沿用原图扩展名。
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Jpeg => Some("jpg"),
            Self::Png => Some("png"),
            Self::Webp => Some("webp"),
            Self::Original => None,
        }
    }

    /// 目标 `image` crate 格式。`Original` 返回 `None`，表示不转码。
    pub fn to_image_format(self) -> Option<ImageFormat> {
        match self {
            Self::Jpeg => Some(ImageFormat::Jpeg),
            Self::Png => Some(ImageFormat::Png),
            Self::Webp => Some(ImageFormat::WebP),
            Self::Original => None,
        }
    }
}