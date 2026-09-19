use serde::Serialize;

use crate::extensions::AnyhowErrorToStringChain;

pub type CommandResult<T> = Result<T, CommandError>;

/// 与原桌面版完全一致的错误结构体。
///
/// 前端 `utils.ts` 里的 `handleError` 读的就是 `err.errTitle` / `err.errMessage`，
/// Web 版保持同一形状，前端逻辑不用改。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub err_title: String,
    pub err_message: String,
}

impl CommandError {
    pub fn from<E>(err_title: &str, err: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        let string_chain = err.into().to_string_chain();
        tracing::error!(err_title, message = string_chain);
        Self {
            err_title: err_title.to_string(),
            err_message: string_chain,
        }
    }
}