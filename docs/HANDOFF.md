# HANDOFF - 项目交接文档

> 最后更新：2026-10-06
> 当前提交：`2c12074`
> 本文档由 WS topic 修复收尾时创建，记录当前状态与已知问题。

---

## 一、项目简介

绅士漫画（wnacg）下载器 Web 版。Rust 后端（axum）+ Vue3 + TSX 前端 SPA，
用 Docker 部署，浏览器打开就能用。

本仓库是 [wnacg-downloader](https://github.com/lanyeeee/wnacg-downloader) 的 Web 化改造版：
把原来的 Tauri 桌面壳换成 **Rust axum 服务端 + Vue3 前端**。

改造范式与这两个项目保持一致：

- [jmcomic-downloader-web](https://github.com/lanyeeee/jmcomic-downloader-web)
- [picacomic-downloader-web](https://github.com/lanyeeee/picacomic-downloader-web)

### 仓库结构

```
src-server/            Rust 后端
  src/
    api/               HTTP 路由与命令层（routes.rs / commands.rs / ws.rs）
    store/             SQLite 持久化
    types/             领域类型
    download_manager.rs  下载调度核心
    event_bus.rs       进程内事件总线 + topics 常量
    events.rs          事件载荷结构体
    wnacg_client.rs    wnacg API 客户端
    config.rs          配置结构
    auth.rs            访问令牌 / Basic Auth
src/                   前端（Vue3 + TSX + Vite）
docs/                  设计/交接文档
  API.md               HTTP API 契约
  websocket.md         WebSocket 契约
  database.md          数据库契约
Dockerfile             三阶段构建（web / server / runtime）
docker-compose.yml     部署编排
```

---

## 二、环境变量

| 变量 | 兼容旧名 | 默认 | 说明 |
|---|---|---|---|
| `WNACG_DATA_DIR` | `JM_DATA_DIR` | `./data` | 数据根目录，Docker 里挂到 `/data` |
| `WNACG_PORT` | `JM_PORT` | `8080` | 监听端口 |
| `WNACG_BIND` | `JM_BIND` | `0.0.0.0` | 监听地址 |
| `WNACG_STATIC_DIR` | `JM_STATIC_DIR` | `./dist` | 前端静态资源目录 |
| `WNACG_AUTH_TOKEN` | `JM_AUTH_TOKEN` | 随机生成并打印 | 访问令牌 |
| `WNACG_AUTH_USER` | `JM_AUTH_USER` | `admin` | Basic Auth 用户名 |
| `WNACG_AUTH_DISABLED` | `JM_AUTH_DISABLED` | `false` | 本地测试可关认证 |
| `JM_LOG_LEVEL` | — | — | 日志级别 |

---

## 三、事件系统（WS）要点

- 前端 topic 分发是**纯本地查表**：`bindings.ts` 的 `TOPIC_MAP`（kebab-case + `-event`），
  `dispatch()` 做精确 `listeners.get(topic)`，**无归一化**——topic 名一字不差才能收到。
- 后端 `event_bus.rs` 的 `topics` 常量必须与 `TOPIC_MAP` 的值完全一致（当前 11/11 匹配）。
- 建连时后端先直发一条快照（topic `task-snapshot-event`，payload 是数组
  `DownloadTaskEvent[]`），随后转发总线消息（增量 `download-task-event` 是单对象）。
  **两条契约不同，所以快照用独立 topic。**
- `/api/ws` 接受 `?token=`（浏览器无法给 WS 设 header）。

---

## 四、修复历史

| 提交 | 内容 |
|---|---|
| `bfa6c32` | fix: 修 WS topic 前后端命名错位（事件系统整体失效） |
| `2c12074` | fix: 修 saveConfig 请求包装错位（配置从未保存成功） |
| `2e17a51` | docs: 补全 API / WebSocket / 数据库契约文档 |

### 缺陷 1：WS topic 前后端命名错位（已修）

- 根因：后端 10 个 snake_case 常量 vs 前端 9 个 kebab-case `-event` 键，
  **交集为 0**；`dispatch()` 精确匹配，于是所有事件静默丢弃。
- 修复：改后端字符串值对齐前端 + jmcomic/picacomic 的命名（常量名不变），
  新增 `TASK_SNAPSHOT = "task-snapshot-event"`，`ws.rs` 快照改用该 topic。
- 选择改后端而非前端：前端 `TOPIC_MAP` 形态与 jmcomic/picacomic 一致，
  改后端可保持三个前端完全一致，且只动 10 个字符串、无逻辑变更。

### 缺陷 2：saveConfig 请求包装错位（已修）

- 根因：前端 `post("/api/config", { config })` 多包一层，后端
  `Json<Config>` 期望裸 `Config` → 恒定 400/422。
- 修复：改为 `post("/api/config", config)`。与 picacomic 同一 bug、同一修法。
- 连带影响：`CONFIG_CHANGED` 事件（`commands.rs:78`）只在保存成功后发出，
  此前不可达；本次修复后随之可达。

---

## 五、已知问题

### 1. `cargo test` 编译失败（既有，未处理）

`src-server` 的测试代码存在编译错误，与业务逻辑无关。CI 走 `docker build`
不跑测试，当前不影响部署。修复前请勿依赖 `cargo test` 的结果。

### 2. WS 修复未完整端到端验证

`bfa6c32` 修了 WS topic 前后端命名错位。已验证到达的 topic：
`task-snapshot-event` / `download-speed-event` / `log-event`（真实浏览器 CDP 抓帧）。

**未端到端验证**（常量已同批对齐，但未在真实运行中观察到）：

- `download-task-event`
- `export-pdf-event` / `export-cbz-event`
- `download-shelf-event`
- `download-sleeping-event`

原因：目标站 `www.wn07.ru` 从开发机不可达（`Connection reset by peer (os error 104)`），
无法创建下载任务。真实环境产生这些事件时需留意。

### 3. `topics::AUTH` 无 emit 点（死代码 / 未完成功能）

`event_bus.rs` 定义了 `AUTH = "auth-event"` 常量，`events.rs` 有 `AuthEvent`
结构体，但**没有任何地方 emit 这个 topic**。不影响现有功能，标注供接手者判断。

---

## 六、运维常用命令

```bash
# 健康检查
curl -sS http://127.0.0.1:8080/api/health

# 读配置（含 token，慎用）
curl -sS http://127.0.0.1:8080/api/config

# 改配置（整体读-改-写，注意是裸 Config，不要包 { config }）
# GET /api/config -> 改字段 -> POST /api/config
curl -sS -X POST http://127.0.0.1:8080/api/config \
  -H 'Content-Type: application/json' \
  --data-binary @config.json

# 容器状态 / 日志
docker compose logs -f
docker compose up -d --build
```

---

## 七、注意事项 / 待办

- **`src-server/Cargo.lock` 不提交**：它是构建产物，保持 untracked。
- **`cargo test` 不可用**：见第五节第 1 条。
- **四类事件待真实环境验证**：见第五节第 2 条。
- **CI**：`.github/workflows/` 下当前只有 `close-default-title-issue.yml`；
  镜像构建工作流待补（Dockerfile 是 3 阶段自带前端构建，CI 直接 `docker build` 即可，
  不要照抄 jmcomic 那份含独立 `pnpm build` 的 2 阶段工作流）。