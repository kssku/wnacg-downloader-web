//! REST 路由。把 `api::commands` 里的业务函数接到 HTTP 端点上。
//!
//! 路径设计原则：与原前端 `bindings.ts` 里的命令名一一对应，
//! 这样前端数据层只需要把 `commands.xxx(args)` 换成 `api.post("/api/xxx", args)`，
//! 语义不用重新理解。

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::api::{commands, error::ApiError, ws};
use crate::auth::{require_auth, AuthConfig};
use crate::config::Config;
use crate::context::AppContext;
use crate::errors::CommandError;
use crate::store::repo::{ImageRepo, TaskRepo, TaskStats};
use crate::store::types::DbTaskState;
use crate::types::{Comic, GetShelfResult, SearchResult, UserProfile};

/// 路由共享状态。
#[derive(Clone)]
pub struct AppState {
    pub app: AppContext,
}

/// 把 `CommandResult<T>` 适配成 handler 的返回类型。
macro_rules! handle {
    ($title:expr, $expr:expr) => {
        $expr.map_err(ApiError::from)
    };
}

/// 组装全部路由。
pub fn router(app: AppContext, auth: AuthConfig) -> Router {
    let state = AppState { app };

    // 不需要认证的端点
    let public = Router::new()
        .route("/health", get(health))
        .route("/auth/check", get(auth_check));

    // 需要认证的端点
    let protected = Router::new()
        // ── 配置 ──────────────────────────────────────────
        .route("/config", get(get_config).post(save_config))
        .route("/server/info", get(server_info).post(post_server_info))
        // ── 登录 ──────────────────────────────────────────
        .route("/login", post(login))
        .route("/user/profile", get(user_profile).post(post_user_profile))
        // ── 搜索 ──────────────────────────────────────────
        .route("/search/keyword", get(search_by_keyword).post(post_search_by_keyword))
        .route("/search/tag", get(search_by_tag).post(post_search_by_tag))
        // ── 详情 / 书架 ───────────────────────────────────
        .route("/comic/:comic_id", get(get_comic))
        .route("/comic", post(post_comic))
        .route("/shelf", get(get_shelf).post(post_get_shelf))
        .route("/shelf/download", post(download_shelf))
        // ── 下载任务 ──────────────────────────────────────
        .route("/download/task", post(create_download_task))
        .route("/download/task/:comic_id/pause", post(pause_download_task))
        .route(
            "/download/task/:comic_id/resume",
            post(resume_download_task),
        )
        .route(
            "/download/task/:comic_id/cancel",
            post(cancel_download_task),
        )
        .route("/download/tasks", get(list_download_tasks))
        // ── 已下载 ────────────────────────────────────────
        .route("/downloaded/comics", get(get_downloaded_comics))
        // ── 封面 ──────────────────────────────────────────
        .route("/cover", get(get_cover))
        // ── 导出 ──────────────────────────────────────────
        .route("/export/pdf", post(export_pdf))
        .route("/export/cbz", post(export_cbz))
        // ── 日志 ──────────────────────────────────────────
        .route("/logs/size", get(get_logs_dir_size).post(post_logs_dir_size))
        .route("/logs", get(get_logs).post(post_logs))
        .route("/logs/clear", post(clear_logs))
        // ── 数据库任务（供青龙等外部脚本消费）──────────────
        .route("/tasks", get(query_tasks))
        .route("/tasks/stats", get(task_stats))
        .route("/tasks/purge", post(purge_tasks))
        .route("/tasks/:comic_id", get(get_task).delete(delete_task))
        .route("/tasks/:comic_id/retry", post(retry_task));

    protected
        .merge(public)
        .route("/ws", get(ws::handler))
        .layer(axum::middleware::from_fn_with_state(
            auth.clone(),
            require_auth,
        ))
        .with_state(state)
}

// ════════════════════════════════════════════════════════════════
// 公开端点
// ════════════════════════════════════════════════════════════════

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// 前端用它探测自己的凭证是否还有效。走到这里说明中间件已放行。
async fn auth_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

// ════════════════════════════════════════════════════════════════
// 配置
// ════════════════════════════════════════════════════════════════

async fn get_config(State(state): State<AppState>) -> Json<Config> {
    Json(commands::get_config(&state.app))
}

async fn save_config(
    State(state): State<AppState>,
    Json(config): Json<Config>,
) -> Result<Json<()>, ApiError> {
    handle!("保存配置失败", commands::save_config(&state.app, config))?;
    Ok(Json(()))
}

async fn server_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(commands::get_server_info(&state.app))
}

/// 前端走的是 `POST /api/server/info` + `{}`。
async fn post_server_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(commands::get_server_info(&state.app))
}

// ════════════════════════════════════════════════════════════════
// 登录
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct LoginRequest {
    #[serde(alias = "email")]
    username: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<String>, ApiError> {
    let cookie = commands::login(&state.app, req.username, req.password)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(cookie))
}

async fn user_profile(State(state): State<AppState>) -> Result<Json<UserProfile>, ApiError> {
    let profile = commands::get_user_profile(&state.app)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(profile))
}

/// 前端走的是 `POST /api/user/profile` + `{}`。
async fn post_user_profile(State(state): State<AppState>) -> Result<Json<UserProfile>, ApiError> {
    user_profile(State(state)).await
}

// ════════════════════════════════════════════════════════════════
// 搜索
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct KeywordSearchQuery {
    keyword: String,
    #[serde(default = "default_page")]
    page: i64,
}

#[derive(Deserialize)]
struct TagSearchQuery {
    #[serde(alias = "tagName", alias = "tag")]
    tag_name: String,
    #[serde(default = "default_page")]
    page: i64,
}

fn default_page() -> i64 {
    1
}

async fn search_by_keyword(
    State(state): State<AppState>,
    Query(q): Query<KeywordSearchQuery>,
) -> Result<Json<SearchResult>, ApiError> {
    let result = commands::search_by_keyword(&state.app, q.keyword, q.page)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn post_search_by_keyword(
    State(state): State<AppState>,
    Json(q): Json<KeywordSearchQuery>,
) -> Result<Json<SearchResult>, ApiError> {
    let result = commands::search_by_keyword(&state.app, q.keyword, q.page)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn search_by_tag(
    State(state): State<AppState>,
    Query(q): Query<TagSearchQuery>,
) -> Result<Json<SearchResult>, ApiError> {
    let result = commands::search_by_tag(&state.app, q.tag_name, q.page)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn post_search_by_tag(
    State(state): State<AppState>,
    Json(q): Json<TagSearchQuery>,
) -> Result<Json<SearchResult>, ApiError> {
    let result = commands::search_by_tag(&state.app, q.tag_name, q.page)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

// ════════════════════════════════════════════════════════════════
// 详情 / 书架
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComicIdRequest {
    comic_id: i64,
}

async fn get_comic(
    State(state): State<AppState>,
    Path(comic_id): Path<i64>,
) -> Result<Json<Comic>, ApiError> {
    let comic = commands::get_comic(&state.app, comic_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(comic))
}

/// 前端走的是 `POST /api/comic` + `{ comicId }`。
async fn post_comic(
    State(state): State<AppState>,
    Json(req): Json<ComicIdRequest>,
) -> Result<Json<Comic>, ApiError> {
    let comic = commands::get_comic(&state.app, req.comic_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(comic))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetShelfQuery {
    shelf_id: i64,
    #[serde(default = "default_page")]
    page: i64,
}

async fn get_shelf(
    State(state): State<AppState>,
    Query(q): Query<GetShelfQuery>,
) -> Result<Json<GetShelfResult>, ApiError> {
    let result = commands::get_shelf(&state.app, q.shelf_id, q.page)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn post_get_shelf(
    State(state): State<AppState>,
    Json(q): Json<GetShelfQuery>,
) -> Result<Json<GetShelfResult>, ApiError> {
    let result = commands::get_shelf(&state.app, q.shelf_id, q.page)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShelfIdRequest {
    shelf_id: i64,
}

async fn download_shelf(
    State(state): State<AppState>,
    Json(req): Json<ShelfIdRequest>,
) -> Result<Json<()>, ApiError> {
    commands::download_shelf(&state.app, req.shelf_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(()))
}

// ════════════════════════════════════════════════════════════════
// 下载任务
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct CreateTaskRequest {
    comic: Comic,
}

async fn create_download_task(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskRequest>,
) -> Json<()> {
    commands::create_download_task(&state.app, req.comic);
    Json(())
}

async fn pause_download_task(
    State(state): State<AppState>,
    Path(comic_id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    handle!(
        "暂停下载任务失败",
        commands::pause_download_task(&state.app, comic_id)
    )?;
    Ok(Json(()))
}

async fn resume_download_task(
    State(state): State<AppState>,
    Path(comic_id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    handle!(
        "继续下载任务失败",
        commands::resume_download_task(&state.app, comic_id)
    )?;
    Ok(Json(()))
}

async fn cancel_download_task(
    State(state): State<AppState>,
    Path(comic_id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    handle!(
        "取消下载任务失败",
        commands::cancel_download_task(&state.app, comic_id)
    )?;
    Ok(Json(()))
}

async fn list_download_tasks(
    State(state): State<AppState>,
) -> Json<Vec<crate::events::DownloadTaskEvent>> {
    Json(commands::list_download_tasks(&state.app))
}

// ════════════════════════════════════════════════════════════════
// 已下载
// ════════════════════════════════════════════════════════════════

async fn get_downloaded_comics(
    State(state): State<AppState>,
) -> Result<Json<Vec<Comic>>, ApiError> {
    let comics = handle!(
        "获取已下载的漫画失败",
        commands::get_downloaded_comics(&state.app)
    )?;
    Ok(Json(comics))
}

// ════════════════════════════════════════════════════════════════
// 封面
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoverQuery {
    cover_url: String,
}

async fn get_cover(
    State(state): State<AppState>,
    Query(q): Query<CoverQuery>,
) -> Result<axum::response::Response, ApiError> {
    let data = commands::get_cover_data(&state.app, q.cover_url)
        .await
        .map_err(ApiError::from)?;

    // 封面可能是 jpg/png/webp，这里统一按 jpeg 返回也可，但更稳妥的是嗅探。
    let content_type = sniff_image_content_type(&data);
    let mut resp = axum::response::Response::new(axum::body::Body::from(data));
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(content_type),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=86400"),
    );
    Ok(resp)
}

/// 极简图片类型嗅探，避免引入额外 crate。
fn sniff_image_content_type(data: &[u8]) -> &'static str {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else {
        "application/octet-stream"
    }
}

// ════════════════════════════════════════════════════════════════
// 导出
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct ExportRequest {
    comic: Comic,
}

async fn export_pdf(
    State(state): State<AppState>,
    Json(req): Json<ExportRequest>,
) -> Result<Json<()>, ApiError> {
    handle!(
        "导出pdf失败",
        commands::export_pdf(&state.app, req.comic)
    )?;
    Ok(Json(()))
}

async fn export_cbz(
    State(state): State<AppState>,
    Json(req): Json<ExportRequest>,
) -> Result<Json<()>, ApiError> {
    handle!(
        "导出cbz失败",
        commands::export_cbz(&state.app, req.comic)
    )?;
    Ok(Json(()))
}

// ════════════════════════════════════════════════════════════════
// 日志
// ════════════════════════════════════════════════════════════════

async fn get_logs_dir_size(State(state): State<AppState>) -> Result<Json<u64>, ApiError> {
    let size = handle!(
        "获取日志目录大小失败",
        commands::get_logs_dir_size(&state.app)
    )?;
    Ok(Json(size))
}

/// 前端走的是 `POST /api/logs/size` + `{}`。
async fn post_logs_dir_size(State(state): State<AppState>) -> Result<Json<u64>, ApiError> {
    get_logs_dir_size(State(state)).await
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default = "default_tail")]
    tail: usize,
}

fn default_tail() -> usize {
    200
}

async fn get_logs(
    State(state): State<AppState>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    let lines = handle!(
        "读取日志失败",
        commands::read_logs(&state.app, q.tail)
    )?;
    Ok(Json(lines))
}

async fn post_logs(
    State(state): State<AppState>,
    Json(q): Json<LogsQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    let lines = handle!(
        "读取日志失败",
        commands::read_logs(&state.app, q.tail)
    )?;
    Ok(Json(lines))
}

async fn clear_logs(State(state): State<AppState>) -> Result<Json<()>, ApiError> {
    handle!("清空日志失败", commands::clear_logs(&state.app))?;
    Ok(Json(()))
}

// ════════════════════════════════════════════════════════════════
// 数据库任务
// ════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct TaskQuery {
    state: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn query_tasks(
    State(state): State<AppState>,
    Query(q): Query<TaskQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.app.store();
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);

    let mut tasks = TaskRepo::list(&store)
        .map_err(|err| ApiError(CommandError::from("查询任务失败", err)))?;

    // 过滤 + 分页放在内存里做：任务表规模是「用户下载过的漫画数」量级
    // （几百到几千行），一次全量读出来比维护一堆动态 SQL 更简单可靠。
    if let Some(want) = q.state.as_deref() {
        tasks.retain(|t| t.state.as_str() == want);
    }
    let result: Vec<_> = tasks
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    Ok(Json(serde_json::json!({ "tasks": result })))
}

async fn task_stats(
    State(state): State<AppState>,
) -> Result<Json<TaskStats>, ApiError> {
    let store = state.app.store();
    let stats = TaskRepo::stats(&store)
        .map_err(|err| ApiError(CommandError::from("统计任务失败", err)))?;
    Ok(Json(stats))
}

async fn get_task(
    State(state): State<AppState>,
    Path(comic_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.app.store();
    let task = TaskRepo::get(&store, comic_id)
        .map_err(|err| ApiError(CommandError::from("查询任务失败", err)))?;

    match task {
        Some(task) => Ok(Json(serde_json::json!(task))),
        None => Err(ApiError(CommandError {
            err_title: "查询任务失败".to_string(),
            err_message: format!("未找到漫画ID为`{comic_id}`的任务"),
        })),
    }
}

async fn delete_task(
    State(state): State<AppState>,
    Path(comic_id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    let store = state.app.store();
    TaskRepo::delete(&store, comic_id)
        .map_err(|err| ApiError(CommandError::from("删除任务失败", err)))?;
    Ok(Json(()))
}

async fn retry_task(
    State(state): State<AppState>,
    Path(comic_id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    let store = state.app.store();

    // 「重试」= 把任务打回 pending、清掉错误、把失败/未完成的图片重置，
    // 然后交给下载管理器重新排队。图片级的 done 状态保留，实现断点续传。
    TaskRepo::set_state(&store, comic_id, DbTaskState::Pending, None)
        .map_err(|err| ApiError(CommandError::from("重试任务失败", err)))?;
    TaskRepo::set_retry_count(&store, comic_id, 0)
        .map_err(|err| ApiError(CommandError::from("重试任务失败", err)))?;

    let pending = ImageRepo::list_pending_indexes(&store, comic_id)
        .map_err(|err| ApiError(CommandError::from("重试任务失败", err)))?;
    tracing::info!("任务 {comic_id} 重新排队，待下载图片 {} 张", pending.len());

    state
        .app
        .download_manager()
        .resume_download_task(comic_id)
        .map_err(|err| ApiError(CommandError::from("重试任务失败", err)))?;

    Ok(Json(()))
}

#[derive(Deserialize)]
struct PurgeRequest {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    before_days: Option<i64>,
}

async fn purge_tasks(
    State(state): State<AppState>,
    Json(req): Json<PurgeRequest>,
) -> Result<Json<u64>, ApiError> {
    let store = state.app.store();

    let mut all = TaskRepo::list(&store)
        .map_err(|err| ApiError(CommandError::from("清理任务失败", err)))?;

    if let Some(want) = req.state.as_deref() {
        all.retain(|t| t.state.as_str() == want);
    }
    if let Some(days) = req.before_days {
        let cutoff = crate::store::types::now_ts() - days * 86_400;
        all.retain(|t| t.updated_at < cutoff);
    }

    let mut removed = 0_u64;
    for task in all {
        TaskRepo::delete(&store, task.comic_id)
            .map_err(|err| ApiError(CommandError::from("清理任务失败", err)))?;
        removed += 1;
    }

    Ok(Json(removed))
}
