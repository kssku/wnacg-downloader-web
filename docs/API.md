# HTTP API 契约

本文档描述 `src-server` 暴露的全部 HTTP 端点。所有路由定义集中在
`src-server/src/api/routes.rs`，处理器按功能拆分在 `src-server/src/api/` 下。

**本文档以代码为准，而非以期望为准。** 凡是代码与设计意图不一致的地方，
都在对应条目下用 `⚠️ 已知问题` 标出，并说明实际行为。

---

## 1. 总览

| 项 | 值 |
| --- | --- |
| 路由注册总数 | **30** 个 `.route(...)` 调用（含 `.route("/ws", ...)`） |
| 路由定义文件 | `src-server/src/api/routes.rs`（643 行） |
| 默认监听 | `0.0.0.0:8080` |
| 认证方式 | `Authorization: Bearer <token>`，WebSocket 另支持 `?token=` |
| 认证默认状态 | **关闭**（未配置 token 时不校验） |

路由分为两组，在 `routes()` 里 merge：

- **公开组 `public`**：`/health`、`/auth/check` —— 不需要认证。
- **受保护组 `protected`**：其余全部端点 —— 经 `require_auth` 中间件。

`/ws` 挂在整个 Router 上（`.route("/ws", get(ws::handler))`），但因为
浏览器无法给 WebSocket 设置请求头，其认证走查询串 `?token=`。

---

## 2. 端点清单

下表逐行对应 `routes.rs` 里的 `.route(...)` 调用。

### 2.1 公开端点

| 方法 | 路径 | 处理器 | routes.rs 行 | 认证 | 请求体 | 响应体 |
| --- | --- | --- | --- | --- | --- | --- |
| GET | `/health` | `health` | 42 | 否 | — | `{"status":"ok"}` |
| GET | `/auth/check` | `auth_check` | 43 | 否 | — | `{"ok":true}` |

### 2.2 配置

| 方法 | 路径 | 处理器 | routes.rs 行 | 请求体 | 响应体 |
| --- | --- | --- | --- | --- | --- | --- |
| GET | `/config` | `get_config` | 50 | — | `Config` |
| POST | `/config` | `save_config` | 50 | **`Config`（裸对象）** | `null` |
| GET | `/server/info` | `server_info` | 51 | — | 服务端信息 |
| POST | `/server/info` | `post_server_info` | 51 | 服务端信息 | — |

> **`POST /api/config` 的请求体是裸 `Config`，不是 `{ config: Config }`。**
> 见第 4 节「已知问题」。

### 2.3 登录与用户

| 方法 | 路径 | 处理器 | routes.rs 行 | 请求体 |
| --- | --- | --- | --- | --- |
| POST | `/login` | `login` | 53 | `{ username, password }` |
| GET | `/user/profile` | `user_profile` | 54 | — |
| POST | `/user/profile` | `post_user_profile` | 54 | — |

### 2.4 搜索

| 方法 | 路径 | 处理器 | routes.rs 行 | 请求体 |
| --- | --- | --- | --- | --- |
| GET | `/search/keyword` | `search_by_keyword` | 56 | query 参数 |
| POST | `/search/keyword` | `post_search_by_keyword` | 56 | `{ keyword, pageNum }` |
| GET | `/search/tag` | `search_by_tag` | 57 | query 参数 |
| POST | `/search/tag` | `post_search_by_tag` | 57 | `{ tagName, pageNum }` |

### 2.5 详情与书架

| 方法 | 路径 | 处理器 | routes.rs 行 | 请求体 |
| --- | --- | --- | --- | --- |
| GET | `/comic/:comic_id` | `get_comic` | 59 | — |
| POST | `/comic` | `post_comic` | 60 | `{ comic_id }` |
| GET | `/shelf` | `get_shelf` | 61 | query 参数 |
| POST | `/shelf` | `post_get_shelf` | 61 | `{ shelfId, pageNum }` |
| POST | `/shelf/download` | `download_shelf` | 62 | `{ shelfId }` |

### 2.6 下载任务

| 方法 | 路径 | 处理器 | routes.rs 行 | 请求体 |
| --- | --- | --- | --- | --- |
| POST | `/download/task` | `create_download_task` | 64 | `{ comic }` |
| POST | `/download/task/:comic_id/pause` | `pause_download_task` | 65 | — |
| POST | `/download/task/:comic_id/resume` | `resume_download_task` | 66–69 | — |
| POST | `/download/task/:comic_id/cancel` | `cancel_download_task` | 70–73 | — |
| GET | `/download/tasks` | `list_download_tasks` | 74 | — |

> **任务粒度是「一本漫画 = 一个任务」，没有章节层。**
> 因此路径参数一律是 `comic_id`，`CreateTaskRequest` 的字段名是 `comic`（整个 `Comic` 对象）。

### 2.7 已下载与封面

| 方法 | 路径 | 处理器 | routes.rs 行 |
| --- | --- | --- | --- |
| GET | `/downloaded/comics` | `get_downloaded_comics` | 76 |
| GET | `/cover` | `get_cover` | 78 |

`/cover` 通过 query 参数 `cover_url` 接收封面地址，内部 fetch 后以二进制返回。

### 2.8 导出

| 方法 | 路径 | 处理器 | routes.rs 行 | 请求体 |
| --- | --- | --- | --- | --- |
| POST | `/export/pdf` | `export_pdf` | 80 | `{ comic }` |
| POST | `/export/cbz` | `export_cbz` | 81 | `{ comic }` |

### 2.9 日志

| 方法 | 路径 | 处理器 | routes.rs 行 | 请求体 |
| --- | --- | --- | --- | --- |
| GET | `/logs/size` | `get_logs_dir_size` | 83 | — |
| POST | `/logs/size` | `post_logs_dir_size` | 83 | `{}` |
| GET | `/logs` | `get_logs` | 84 | query 参数 |
| POST | `/logs` | `post_logs` | 84 | query 参数封装 |
| POST | `/logs/clear` | `clear_logs` | 85 | — |

### 2.10 数据库任务端点（供外部脚本消费）

这一组直接读写 SQLite，**前端完全不调用**（见第 5 节）。
设计目标是给青龙面板之类的外部脚本一个稳定的 HTTP 契约。

| 方法 | 路径 | 处理器 | routes.rs 行 | 请求体 | 响应体 |
| --- | --- | --- | --- | --- | --- |
| GET | `/tasks` | `query_tasks` | 85 | query：`limit`、`offset`、`state` | `{ "tasks": DbTask[] }` |
| GET | `/tasks/stats` | `task_stats` | 86 | — | `TaskStats` |
| POST | `/tasks/purge` | `purge_tasks` | 87 | `{ state?, before_days? }` | 删除条数（`u64`） |
| GET | `/tasks/:comic_id` | `get_task` | 88 | — | `DbTask` |
| DELETE | `/tasks/:comic_id` | `delete_task` | 88 | — | — |
| POST | `/tasks/:comic_id/retry` | `retry_task` | 89 | — | `()` |

`/tasks` 的过滤与分页在内存里完成（`TaskRepo::list` 全量读出后 `retain` /
`skip` / `take`），`limit` 默认 `100`、上限 `1000`。理由见代码注释：
任务表规模是「用户下载过的漫画数」量级，动态 SQL 不值得。

`TaskStats` 字段（`store/repo.rs`）：
`total`、`pending`、`downloading`、`completed`、`failed`、`cancelled`、
`total_img_count`、`done_img_count`。

### 2.11 WebSocket

| 方法 | 路径 | 处理器 | routes.rs 行 | 认证 |
| --- | --- | --- | --- | --- |
| GET | `/ws` | `ws::handler` | 95 | `?token=` 查询串 |

详见 `websocket.md`。

---

## 3. 状态字段的大小写

同一个业务概念在后端有两套枚举，序列化形式不同：

| 枚举 | 定义位置 | 序列化 | 用途 |
| --- | --- | --- | --- |
| `DbTaskState` | `store/types.rs` | **小写字符串**（手工 `as_str()`） | 落库、`/tasks*` 返回 |
| `DownloadTaskState` | `download_manager.rs` | **PascalCase**（serde 默认） | WebSocket 事件载荷 |

两者的变体集合一致：`Pending` / `Downloading` / `Paused` / `Cancelled` /
`Completed` / `Failed`，各 6 个。

- `DbTaskState::as_str()` 产出 `"pending"` / `"downloading"` / …
- `DownloadTaskState` 没有 `rename_all`，serde 默认输出 `"Pending"` / `"Downloading"` / …

**两条路径不交叉。** REST 侧（`/tasks*`）只返回 `DbTaskState`，WS 侧只发
`DownloadTaskState`；前端不从 REST 读任务状态，也不从 WS 读小写状态。

---

## 4. 已知问题

### 4.1 `POST /api/config` 请求体包装错位（配置从未保存成功）

- **后端**：`routes.rs` 的 `save_config` 签名是 `Json<Config>` —— **裸对象**。
- **前端**：`src/bindings.ts:276` 发送 `post("/api/config", { config })` —— **多包了一层**。

```ts
// src/bindings.ts:275-277（现状）
async saveConfig(config: Config): Promise<Result<null, CommandError>> {
  return await callResult<null>(() => post("/api/config", { config }));
}
```

后端反序列化 `{ "config": {...} }` 到 `Config` 会失败，返回 4xx。
也就是说：**配置从未保存成功过**。

连带影响：

- `ConfigChangedEvent` 的 emit 点（`api/commands.rs:78`）位于保存成功之后，
  因此这条事件实际上**永远不会发出**。
- 用户改完配置点保存，界面上没有任何报错路径会浮出来（`callResult` 包装后
  前端拿到的是 `Err`，但当前 UI 未展示）。

> 本轮**只记录、不修复**。修复与 picacomic 的同名问题（commit `17f2c41`）
> 一致，改法是去掉 `{ }`，改为 `post("/api/config", config)`。

### 4.2 `migrate_v1` 的枚举注释（已修正）

`store/migrations.rs:49` 的建表注释曾写着 `pending / running / completed /
failed / cancelled` —— `running` 从未存在过，且漏了 `downloading` 与
`paused`。

**已于本轮修正**，现与 `DbTaskState::as_str()` 一致：
`pending` / `downloading` / `paused` / `cancelled` / `completed` / `failed`。

此前因列是 `TEXT` 且无 CHECK 约束，**该错误注释从未影响行为**。

---

## 5. 前端调用面

`src/bindings.ts` 里的命令层方法是前端的全部调用面：

| 前端方法 | 实际请求 | bindings.ts 行 |
| --- | --- | --- |
| `login` | `POST /api/login` | 268 |
| `getConfig` | `GET /api/config` | 272 |
| `saveConfig` | `POST /api/config` ⚠️ 见 4.1 | 276 |
| `getUserProfile` | `POST /api/user/profile` | 280 |
| `searchByKeyword` | `POST /api/search/keyword` | 285 |
| `searchByTag` | `POST /api/search/tag` | 290 |
| `getComic` | `GET /api/comic/{id}` | 294 |
| `getShelf` | `POST /api/shelf` | 298 |
| `downloadShelf` | `POST /api/shelf/download` | 302 |
| `createDownloadTask` | `POST /api/download/task` | 307 |
| `pauseDownloadTask` | `POST /api/download/task/{id}/pause` | 311 |
| `resumeDownloadTask` | `POST /api/download/task/{id}/resume` | 315 |
| `cancelDownloadTask` | `POST /api/download/task/{id}/cancel` | 319 |
| `getDownloadedComics` | `GET /api/downloaded/comics` | 323 |
| `exportPdf` | `POST /api/export/pdf` | 327 |
| `exportCbz` | `POST /api/export/cbz` | 331 |
| `getLogsDirSize` | `POST /api/logs/size` | 335 |
| `showPathInFileManager` | **无请求**（Web 版 no-op） | 343 |
| `getCoverData` | `GET /api/cover?cover_url=…` | 351 |

**前端不调用 `/api/tasks*`。** 在 `src/` 下检索 `/api/tasks` 命中数为 **0**。
这 6 个端点纯粹是给外部脚本的契约，与前端无关。

同理，前端不调用：`/health`、`/auth/check`、`/server/info`、`/comic`（POST）、
`/logs`、`/logs/clear`、`/ws`（由 WS 客户端在 `bindings.ts` 内部直连，
不经过命令层）。

### 请求体包装的正确性

wnacg 后端用的是**具名请求结构体**，不是 picacomic 的泛型 `ComicWrapper<T>`：

- `CreateTaskRequest { comic }` → 前端 `{ comic }` ✅
- `ExportRequest { comic }` → 前端 `{ comic }` ✅
- `ComicIdRequest { comic_id }` → 前端 `{ comicId }` ✅

因此这些 `{ }` 包装是**有意且正确**的。唯一错位的是 `/api/config`。

---

## 6. `json!` 使用统计

`serde_json::json!` 宏在 `src-server/src/` 下共 **8** 处：

| 文件:行 | 用途 |
| --- | --- |
| `wnacg_client.rs:61` | 构造上游站点表单 |
| `wnacg_client.rs:129` | 构造上游站点查询参数 |
| `auth.rs:218` | 认证相关响应 |
| `api/commands.rs:462` | 命令层返回 |
| `api/routes.rs:106` | `{"status":"ok"}` |
| `api/routes.rs:111` | `{"ok":true}` |
| `api/routes.rs:545` | `{"tasks": result}` |
| `api/routes.rs:566` | `get_task` 的 `DbTask` 直出 |

这 8 处均为**即席 JSON 拼装**，没有对应的类型化结构体。其中 `routes.rs:545`
与 `:566` 把 `DbTask` 直接塞进 `json!`，因此 `/tasks*` 的响应字段名取决于
`DbTask` 的 serde 配置（无 `rename_all`，输出 snake_case：
`comic_id`、`comic_title`、`total_img_count`、`done_img_count`、`retry_count`、
`last_error`、`download_dir`、`created_at`、`updated_at`）。

> 与 picacomic 的差异：picacomic 有 5 处 `json!`，wnacg 有 8 处。
> 多出的 3 处集中在 `/tasks` 这一组外部脚本端点上。
