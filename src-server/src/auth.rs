//! 单用户 Token 认证。
//!
//! 这是一个**自部署的单用户后台**，不是多租户系统。因此不做用户表、
//! 不做密码哈希、不做 session —— 只有一个访问令牌：
//!
//! - 优先从环境变量 `WNACG_AUTH_TOKEN` 读；
//! - 没有就随机生成一个 32 位令牌，在启动横幅里打印一次；
//! - 请求头支持 `Authorization: Bearer <token>`；
//! - 也支持 Basic Auth（用户名固定，密码即 Token），这样浏览器会弹原生登录框；
//! - WebSocket 因为浏览器 API 限制，额外支持 `?token=xxx`。
//!
//! 设 `WNACG_AUTH_DISABLED=1` 可以完全关掉认证（仅限已由反代做鉴权的场景）。

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine as _;

/// 认证配置。构造后不可变，可以安全共享。
#[derive(Clone)]
pub struct AuthConfig {
    token: Arc<str>,
    username: Arc<str>,
    /// 是否跳过认证（`WNACG_AUTH_DISABLED=1`）
    disabled: bool,
}

impl AuthConfig {
    /// 从环境变量构造。返回 `(config, token_is_generated)`。
    pub fn from_env() -> (Self, bool) {
        let disabled = std::env::var("WNACG_AUTH_DISABLED")
            .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);

        let (token, generated) = match std::env::var("WNACG_AUTH_TOKEN") {
            Ok(token) if !token.trim().is_empty() => (token.trim().to_string(), false),
            _ => (random_token(), true),
        };

        let username = std::env::var("WNACG_AUTH_USERNAME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "admin".to_string());

        (
            Self {
                token: Arc::from(token.as_str()),
                username: Arc::from(username.as_str()),
                disabled,
            },
            generated,
        )
    }

    /// 校验一个候选 Token 是否正确。用固定时间比较，避免时序侧信道。
    fn token_matches(&self, candidate: &str) -> bool {
        let expected = self.token.as_bytes();
        let candidate = candidate.as_bytes();
        if expected.len() != candidate.len() {
            return false;
        }
        // 长度已经相等，这里逐字节异或累加，不提前 return。
        expected
            .iter()
            .zip(candidate)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }

    /// 校验 Basic Auth 的 `user:pass`。密码即 Token。
    fn basic_matches(&self, decoded: &str) -> bool {
        let Some((user, pass)) = decoded.split_once(':') else {
            return false;
        };
        user == self.username.as_ref() && self.token_matches(pass)
    }

    /// 生成一个浏览器可直接用的 `Authorization: Basic ...` 头。
    pub fn basic_authorization(&self) -> String {
        let raw = format!("{}:{}", self.username, self.token);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }

    /// 启动横幅里要展示的 Token 原文。
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// 认证是否被显式关闭（`WNACG_AUTH_DISABLED=1`）。
    ///
    /// 启动横幅据此决定是打印访问令牌还是提示"无需认证"——关掉认证时
    /// 再打印令牌会让人以为还要登录，与实际行为不符。
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// 生成一个随机 Token。没有 `rand` 之外的依赖，够用。
fn random_token() -> String {
    use rand::Rng as _;
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| {
            let idx = rng.gen_range(0..36);
            char::from_digit(idx, 36).unwrap_or('0')
        })
        .collect()
}

/// 无需认证的端点（相对于 API 挂载点的路径）。
///
/// 注意这里是**挂载后的相对路径**：`routes::router` 会被 `main.rs`
/// 用 `.nest("/api", ..)` 挂上去，而 axum 的 `nest` 会在进入内层路由
/// 之前把前缀剥掉，中间件看到的 `uri().path()` 是 `/health` 而不是
/// `/api/health`。所以匹配必须用相对路径。
const PUBLIC_PATHS: [&str; 2] = ["/health", "/auth/check"];

/// axum 中间件：拒绝未认证请求。
pub async fn require_auth(
    State(auth): State<AuthConfig>,
    req: Request,
    next: Next,
) -> Response {
    // 认证被显式关闭时（WNACG_AUTH_DISABLED），所有请求直接放行。
    if auth.disabled {
        return next.run(req).await;
    }

    let path = req.uri().path();

    // 健康检查与登录态探测不需要认证。
    if PUBLIC_PATHS.contains(&path) {
        return next.run(req).await;
    }

    if let Some(credential) = extract_credential(&req) {
        let ok = match credential {
            Credential::Bearer(token) => auth.token_matches(token),
            Credential::Basic(decoded) => auth.basic_matches(&decoded),
        };
        if ok {
            return next.run(req).await;
        }
    }

    // 兜底：`?token=xxx`。浏览器 WebSocket 发不了请求头，只能走这里。
    if let Some(token) = extract_query_token(&req) {
        if auth.token_matches(token) {
            return next.run(req).await;
        }
    }

    unauthorized()
}

enum Credential<'a> {
    Bearer(&'a str),
    Basic(String),
}

/// 从请求头里取出凭证。Bearer 优先于 Basic。
fn extract_credential(req: &Request) -> Option<Credential<'_>> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;

    if let Some(token) = value.strip_prefix("Bearer ") {
        return Some(Credential::Bearer(token.trim()));
    }

    if let Some(encoded) = value.strip_prefix("Basic ") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()?;
        let decoded = String::from_utf8(decoded).ok()?;
        return Some(Credential::Basic(decoded));
    }

    None
}

/// 从 URL 查询串里取 Token：`?token=xxx`。
///
/// 浏览器的 `WebSocket` 构造函数**无法携带自定义请求头**，所以
/// `/api/ws` 不能走 `Authorization`。这是 WebSocket 协议本身的限制，
/// 不是偷懒——所有需要鉴权的浏览器 WS 都是这么做的。
///
/// 安全权衡：查询串可能出现在反向代理的访问日志里。对单用户自部署的
/// NAS 场景可以接受；若要进一步收紧，应在反代层关闭对 `/api/ws` 的
/// query 记录，或改用一次性票据（不在本期范围）。
fn extract_query_token(req: &Request) -> Option<&str> {
    let query = req.uri().query()?;
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("token=") {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// 统一的 401 响应。带 `WWW-Authenticate` 让浏览器弹出登录框。
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Basic realm=\"wnacg-server\"")],
        axum::Json(serde_json::json!({
            "errTitle": "未认证",
            "errMessage": "缺少或无效的访问凭证，请在请求头带上 Authorization: Bearer <token>",
        })),
    )
        .into_response()
}