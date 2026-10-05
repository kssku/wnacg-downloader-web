# WebSocket 契约

前端与后端之间唯一的实时通道。本文档描述连接方式、消息格式、topic 常量、
心跳与重连，以及**当前存在的 topic 不匹配问题**。

**以 `src-server/src/event_bus.rs` 与 `src-server/src/api/ws.rs` 为准。**

---

## 1. 总览

| 项 | 值 |
| --- | --- |
| 端点 | `GET /api/ws` |
| 认证 | `?token=<token>` 查询串（浏览器无法给 WS 设请求头） |
| 传输 | `tokio::sync::broadcast`，容量 **1024** |
| 服务端心跳 | Ping，**30s**（首次 tick 跳过） |
| 客户端心跳 | 文本 `"ping"`，**25s** |
| 重连 | 指数退避，1000ms 起，×2，上限 15000ms |
| topic 常量数（后端） | **10** |
| 前端订阅 topic 数 | **9** |
| 前后端匹配的 topic 数 | **0** ⚠️ |

---

## 2. 连接与握手

### 2.1 升级

`src-server/src/api/ws.rs:42` 的 `handler` 接收 `WebSocketUpgrade`，
`ws.on_upgrade` 后交给 `handle_socket`（`:47`）。

### 2.2 先发快照

连上后**第一件事是补发全量任务快照**（`:50-60`），然后才进入实时事件转发：

```rust
let snapshot = SnapshotMessage {
    topic: topics::DOWNLOAD_TASK,          // ws.rs:53
    payload: app.download_manager().snapshot(),
};
```

`SnapshotMessage`（`:34-39`）形状为 `{ topic, payload }`，
`payload` 是 **`Vec<DownloadTaskEvent>`**。

> 因为 wnacg 的任务粒度是整本漫画、没有章节层，快照载荷就是
> `Vec<DownloadTaskEvent>`（`ws.rs:15-16` 注释）。这与 picacomic 的
> 章节导向载荷形状不同。

顺序很重要（`:50-51` 注释）：前端收到快照后会把任务列表整体替换，随后到达的
Update 事件才有正确的基线。

**注意**：快照是直接 `sender.send` 的，**绕过 `EventBus`**，因此它不会进入
broadcast、也不会有其他订阅者收到。

### 2.3 订阅

发完快照才 `app.events().subscribe()`（`:62`），随后进入 `select!` 循环。

这意味着**快照与订阅之间存在一个极窄的窗口**：快照发出后、订阅建立前发生的
事件会丢失。当前实现接受这一点（快照本身是对「连接之前就已发生的状态」的补偿）。

---

## 3. 消息格式

### 3.1 服务端 → 客户端

每条消息是一个 JSON 文本帧，与 `BusMessage` 一一对应：

```json
{ "topic": "<topic 字符串>", "payload": { ... } }
```

`BusMessage`（`event_bus.rs:31-35`）：

```rust
pub struct BusMessage {
    pub topic: String,
    pub payload: serde_json::Value,
}
```

### 3.2 客户端 → 服务端

| 客户端发送 | 服务端行为 | ws.rs 行 |
| --- | --- | --- |
| `Close` | 断开连接 | 93 |
| `Ping(payload)` | 回 `Pong(payload)` | 94–98 |
| 任何文本（含 `"ping"`） | **忽略** | 100 |
| 错误 | 断开 | 101 |

前端 25s 发一次文本 `"ping"` 只是**保活探测**，服务端不解析、不回应。
真正的保活是服务端 30s 的 WebSocket 协议级 Ping。

---

## 4. topic 常量（后端）

`src-server/src/event_bus.rs:15-28` 定义 `pub mod topics`，共 **10** 个常量：

| # | 常量名 | 值 | event_bus.rs 行 | 有 emit 点？ |
| --- | --- | --- | --- | --- |
| 1 | `DOWNLOAD_TASK` | `download_task` | 16 | ✅ `download_manager.rs:543`、`:550`、快照 `ws.rs:53` |
| 2 | `DOWNLOAD_TASK_DELETED` | `download_task_deleted` | 19 | ✅ `download_manager.rs:550` |
| 3 | `DOWNLOAD_SPEED` | `download_speed` | 20 | ✅ `download_manager.rs:203` |
| 4 | `DOWNLOAD_SLEEPING` | `download_sleeping` | 21 | ✅ `download_manager.rs:508` |
| 5 | `EXPORT_PDF` | `export_pdf` | 22 | ✅ `export.rs:155`、`:172` |
| 6 | `EXPORT_CBZ` | `export_cbz` | 23 | ✅ `export.rs:63`、`:143` |
| 7 | `DOWNLOAD_SHELF` | `download_shelf` | 24 | ✅ `commands.rs:384`、`:440`、`:450` |
| 8 | `LOG` | `log` | 25 | ✅ `logger.rs:36` |
| 9 | `AUTH` | `auth` | 26 | ❌ **无 emit 点** |
| 10 | `CONFIG_CHANGED` | `config_changed` | 27 | ⚠️ `commands.rs:78`（当前不可达） |

**topic 常量数核对：10。**

关于两个「异常」常量：

- **`AUTH`（`auth`）**：只有 `AuthEvent` 结构体（`events.rs:115`）存在，
  **从未被 emit**。属于声明但未使用的常量。
- **`CONFIG_CHANGED`（`config_changed`）**：emit 点位于 `commands.rs:78`，
  在配置保存成功**之后**。由于 `POST /api/config` 的请求体包装错位导致保存
  永远失败（见 `API.md` 第 4.1 节），这条事件**实际不可达**。

---

## 5. 前端订阅面

`src/bindings.ts:484-494` 定义 `TOPIC_MAP`：

| 前端 key | 映射到的 topic | bindings.ts 行 |
| --- | --- | --- |
| `downloadShelfEvent` | `download-shelf-event` | 485 |
| `downloadSleepingEvent` | `download-sleeping-event` | 486 |
| `downloadSpeedEvent` | `download-speed-event` | 487 |
| `downloadTaskEvent` | `download-task-event` | 488 |
| `downloadTaskDeletedEvent` | `download-task-deleted-event` | 489 |
| `exportCbzEvent` | `export-cbz-event` | 490 |
| `exportPdfEvent` | `export-pdf-event` | 491 |
| `logEvent` | `log-event` | 492 |
| `taskSnapshot` | `task-snapshot-event` | 493 |

**9 个 topic。** 注释（`:484`）写着「需与 src-server/src/event_bus.rs 的
 topics 一致」，但**实际上一个都不一致**。

### 5.1 分发逻辑

```ts
function dispatch(topic: string, payload: any): void {
  lastPayload.set(topic, payload);
  const set = listeners.get(topic);   // 精确字符串查找，无归一化
  if (!set) return;
  ...
}
```

`dispatch` 用**精确字符串**查 `listeners`，没有任何大小写折叠、连字符/下划线
互转或前后缀裁剪。服务端发什么 topic，就必须有什么 key。

### 5.2 订阅与重放

`subscribe(key, cb)`（`:496-517`）：

1. 经 `TOPIC_MAP` 把 key 翻成 topic。
2. 在 `listeners` 里注册 `wrapped` 监听器。
3. **重放**：若 `lastPayload` 里有该 topic 的缓存，立即用缓存调一次 `cb`
   （`:508-513`）。
4. 调 `connect()` 确保连接存在。

重放机制是为「组件挂载晚于事件到达」设计的，注释特别提到 `taskSnapshot`。

---

## 6. ⚠️ 已知问题：整个事件系统是死的

### 6.1 现象

前端的 9 个 topic 字符串（连字符 + `-event` 后缀）与后端的 10 个常量
（下划线命名）**没有任何一个字符串相等**：

| 前端 | 后端 | 相等？ |
| --- | --- | --- |
| `download-task-event` | `download_task` | ❌ |
| `download-task-deleted-event` | `download_task_deleted` | ❌ |
| `download-speed-event` | `download_speed` | ❌ |
| `download-sleeping-event` | `download_sleeping` | ❌ |
| `export-pdf-event` | `export_pdf` | ❌ |
| `export-cbz-event` | `export_cbz` | ❌ |
| `download-shelf-event` | `download_shelf` | ❌ |
| `log-event` | `log` | ❌ |
| `task-snapshot-event` | （不存在；快照走 `download_task`） | ❌ |

### 6.2 后果

`dispatch()` 做的是 `listeners.get(topic)` 精确查找。服务端转发的是
`BusMessage.topic` 原值（即后端常量），因此：

- **每一个事件都被静默丢弃** —— `listeners` 里永远查不到对应 key。
- `lastPayload` 缓存的是**后端 topic**，而 `subscribe` 查的是**前端 topic**，
  所以重放**永不触发**。
- `taskSnapshot` 映射到的 `task-snapshot-event` 在后端根本不存在；
  快照实际以 topic `download_task` 到达，前端收不到。
- `bindings.ts:508` 的注释「WebSocket 建连时后端会推一次全量任务状态」
  在当前实现下**是错的**。

结论：**WebSocket 层在功能上完全失效**。任务列表、下载速度、日志面板、
导出进度、书架进度全部收不到任何推送。

### 6.3 前端订阅者

在 `src/`（排除 `bindings.ts`）下检索 `taskSnapshot` / `downloadTaskEvent` /
`downloadShelfEvent` / `logEvent` / `exportPdfEvent`，命中数为 **0**。

也就是说：即使 topic 修好了，**当前也没有任何组件在监听这些事件**。
这让问题更难被发现 —— 没有报错，只是界面永远不更新。

### 6.4 修复方向（本轮不做）

两种改法，二选一：

1. **改前端**：把 `TOPIC_MAP` 的值改成后端常量
   （`download_task`、`log`、…），并把 `taskSnapshot` 指向 `download_task`。
2. **改后端**：把 `topics` 常量改成连字符形式，对齐 picacomic 的命名。

方案 2 与 picacomic 一致（picacomic 的 `topics` 用连字符且与前端匹配）。
方案 1 改动面更小。**无论选哪个，都必须同时补上订阅者。**

> 本轮**只记录、不修复**。

---

## 7. 事件载荷定义

全部定义在 `src-server/src/events.rs`。

### 7.1 `DownloadTaskEvent`（`:24-31`）

```rust
#[serde(rename_all = "camelCase")]
pub struct DownloadTaskEvent {
    pub state: DownloadTaskState,   // PascalCase 序列化
    pub comic: Comic,
    pub downloaded_img_count: u32,  // → downloadedImgCount
    pub total_img_count: u32,       // → totalImgCount
}
```

同一结构体既用于**单任务状态更新**，也用于**连接时的全量快照**（`:19-21` 注释）。

### 7.2 其余事件

| 结构体 | events.rs 行 | 字段 |
| --- | --- | --- |
| `DownloadTaskDeletedEvent` | 36 | `comic_id`（→ `comicId`） |
| `DownloadSpeedEvent` | 43 | `speed` |
| `DownloadSleepingEvent` | 50 | `comic_id`、`remaining_sec` |
| `LogEvent` | 101-110 | `timestamp`、`level`、`fields`、`target`、`filename`、`line_number` |
| `AuthEvent` | 115 | `logged_in`、`user_profile` |
| `ConfigChangedEvent` | 123 | `download_format` |

### 7.3 带 tag 的枚举事件

`ExportPdfEvent`（`:58`）、`ExportCbzEvent`（`:69`）、
`DownloadShelfEvent`（`:80`）用 `#[serde(tag = "event", content = "data")]`：

- `ExportPdfEvent` / `ExportCbzEvent`：`Start { uuid, title }` / `End { uuid }`
- `DownloadShelfEvent`：`GettingShelfComics` /
  `CreatingDownloadTask { current, total }` / `End`

### 7.4 `LogEvent` 的字段名陷阱

`LogEvent` 派生 `#[serde(rename_all = "camelCase")]`，但 `line_number`
单独用 `#[serde(rename = "line_number")]`（`:108`）**显式改回 snake_case**。

原因（`:95-98` 注释）：`LogEventWriter` 直接 `serde_json::from_str` 反序列化
tracing 写出的那一行 JSON，字段少一个就会解析失败，**整个日志面板收不到任何
日志**。前端 `LogDialog.tsx::formatLogEvent` 也按这个形状渲染，两边都不能改。

---

## 8. 断线、重连与背压

### 8.1 服务端

- 广播容量 **1024**（`event_bus.rs:52` 的 `broadcast::channel(1024)`）。
- 客户端消费过慢时 `broadcast` 返回 `Lagged(skipped)`，
  服务端 `tracing::warn!` 后 **continue**（`ws.rs:83-85`）——
  **跳过丢弃的消息，不掐断连接**。
- `Closed` → break（`:86`）。

### 8.2 客户端（`bindings.ts`）

| 机制 | 实现 | bindings.ts 行 |
| --- | --- | --- |
| 重连延迟 | `reconnectDelay = 1000` | ~371 |
| 退避 | `Math.min(reconnectDelay * 2, 15000)` | `scheduleReconnect` |
| 心跳定时器 | 25000ms 发文本 `"ping"` | `socket.onopen` |
| 手动关闭标记 | `manualClose` | `disconnectEvents` |
| 连接 URL | `wsUrl(token)`，`?token=` 兜底 | `wsUrl` |

`onmessage` 里先过滤 `!ev.data` 与 `"pong"`，再 `JSON.parse`，
解析失败直接 return；没有 `topic` 字段也 return。

`onerror` 不做处理 —— `onclose` 紧随其后，统一在那里重连。

`disconnectEvents()`（登出时调用）会设 `manualClose = true`、
清掉心跳定时器、关闭 socket、并 `lastPayload.clear()`。

---

## 9. 交叉核对

| 核对项 | 文档值 | 代码值 | 结果 |
| --- | --- | --- | --- |
| 后端 topic 常量数 | 10 | 10（`event_bus.rs:16-27`） | ✅ |
| 前端 `TOPIC_MAP` 项数 | 9 | 9（`bindings.ts:485-493`） | ✅ |
| 前后端匹配的 topic 数 | 0 | 0 | ✅（问题确认） |
| 广播容量 | 1024 | 1024 | ✅ |
| 服务端心跳间隔 | 30s | 30s | ✅ |
| 客户端心跳间隔 | 25s | 25s | ✅ |
| 前端订阅者数量 | 0 | 0 | ✅（问题确认） |
