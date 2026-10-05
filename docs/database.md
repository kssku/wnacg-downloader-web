# 数据库契约

wnacg-downloader-web 用 SQLite 持久化下载任务与图片明细，用于断点续传与
「已下载」列表。本文档描述表结构、迁移机制与连接参数。

**以 `src-server/src/store/migrations.rs` 为准。**

---

## 1. 总览

| 项 | 值 |
| --- | --- |
| 引擎 | SQLite（`rusqlite`，非 sqlx / 非 refinery） |
| 迁移机制 | `PRAGMA user_version` 手动分派 |
| 当前 `SCHEMA_VERSION` | **1** |
| 表数量 | **2** |
| 索引数量 | **2** |
| 外键数量 | **1** |
| 迁移文件 | `src-server/src/store/migrations.rs`（157 行） |
| 仓储层 | `src-server/src/store/repo.rs`（`TaskRepo` / `ImageRepo`） |
| 连接管理 | `src-server/src/store/types.rs`（`Store`） |

设计约束（`migrations.rs:6-8` 注释）：迁移必须**幂等且向前兼容** —— 容器镜像
升级后旧库要能直接用。因此新增列一律走 `ALTER TABLE ADD COLUMN`，不改动既有
列的类型或含义。

---

## 2. 表结构

### 2.1 `download_task` —— 10 列

一本漫画一行。**wnacg 的任务粒度就是整本漫画，没有章节层。**

| # | 列名 | 类型 | 约束 / 默认 | migrations.rs 行 |
| --- | --- | --- | --- | --- |
| 1 | `comic_id` | INTEGER | **PRIMARY KEY** | 47 |
| 2 | `comic_title` | TEXT | NOT NULL | 48 |
| 3 | `state` | TEXT | NOT NULL DEFAULT `'pending'` | 50 |
| 4 | `total_img_count` | INTEGER | NOT NULL DEFAULT 0 | 51 |
| 5 | `done_img_count` | INTEGER | NOT NULL DEFAULT 0 | 52 |
| 6 | `retry_count` | INTEGER | NOT NULL DEFAULT 0 | 53 |
| 7 | `last_error` | TEXT | 可空 | 54 |
| 8 | `download_dir` | TEXT | NOT NULL DEFAULT `''` | 56 |
| 9 | `created_at` | INTEGER | NOT NULL DEFAULT 0 | 57 |
| 10 | `updated_at` | INTEGER | NOT NULL DEFAULT 0 | 58 |

**列数核对：10。**

`download_dir` 是**任务创建时的下载目录快照**（`:55` 注释）。恢复时必须用任务
自己当初的目录，而不是当前配置，否则用户中途改了下载目录，恢复时就找不到已
下载的文件。

`created_at` / `updated_at` 是 Unix 时间戳（`store/types.rs` 的 `now_ts()`）。

> ⚠️ `state` 列上方的注释（`:49`）写着
> `pending / running / completed / failed / cancelled`，**已过期**：
> 实际枚举是 `pending` / `downloading` / `paused` / `cancelled` /
> `completed` / `failed`。注释里的 `running` 不存在，且漏了 `downloading`
> 与 `paused`。列本身是 `TEXT`、无 CHECK 约束，因此**不影响行为**。

### 2.2 `download_image` —— 9 列

一张图一行。断点续传的最小单位。

| # | 列名 | 类型 | 约束 / 默认 | migrations.rs 行 |
| --- | --- | --- | --- | --- |
| 1 | `comic_id` | INTEGER | NOT NULL（复合主键之一） | 67 |
| 2 | `img_index` | INTEGER | NOT NULL（复合主键之一） | 68 |
| 3 | `url` | TEXT | NOT NULL DEFAULT `''` | 69 |
| 4 | `state` | TEXT | NOT NULL DEFAULT `'pending'` | 71 |
| 5 | `retry_count` | INTEGER | NOT NULL DEFAULT 0 | 72 |
| 6 | `last_error` | TEXT | 可空 | 73 |
| 7 | `bytes` | INTEGER | 可空 | 74 |
| 8 | `updated_at` | INTEGER | NOT NULL DEFAULT 0 | 75 |
| 9 | — | — | PRIMARY KEY `(comic_id, img_index)` | 76 |

**列数核对：9**（第 9 项是表级约束，不是列；实际列 8 个 + 1 个复合主键）。

图片状态枚举（`:70` 注释）：`pending` / `done` / `failed` —— 三个变体。

`bytes` 可空，因为行创建时还没开始下载，尺寸未知。

---

## 3. 索引

**索引数量核对：2。**

| # | 索引名 | 表 | 定义 | migrations.rs 行 |
| --- | --- | --- | --- | --- |
| 1 | `idx_task_state_updated` | `download_task` | `(state, updated_at DESC)` | 62–63 |
| 2 | `idx_image_state` | `download_image` | `(comic_id, state)` | 81–82 |

- `idx_task_state_updated`：恢复时按「未完成任务」筛选。
- `idx_image_state`：找出某本漫画里还没下完的图。

---

## 4. 外键

**外键数量核对：1。**

```sql
FOREIGN KEY (comic_id) REFERENCES download_task (comic_id) ON DELETE CASCADE
```

- 定义位置：`migrations.rs:77`
- 行为：删除 `download_task` 一行时，`download_image` 里对应的全部行级联删除。
- 测试覆盖：`migrations.rs:134-156` 的 `deleting_task_cascades_to_images`。

级联删除依赖 `PRAGMA foreign_keys = ON`。该 pragma 在 `Store::tune()` 里设置
（见第 6 节）；测试里则显式 `conn.execute_batch("PRAGMA foreign_keys = ON;")`
（`migrations.rs:96`）。

---

## 5. 迁移机制

### 5.1 `PRAGMA user_version`

不建额外的 migrations 表，直接用 SQLite 自带的 `user_version` 整数位记录
schema 版本（`migrations.rs:3-4` 注释：不需要额外建表，也不用担心和业务表混淆）。

### 5.2 `run()` 流程（`migrations.rs:17-39`）

1. 读 `PRAGMA user_version` 到 `current`。
2. 若 `current >= SCHEMA_VERSION` → 直接返回（`:22-25`）。
3. 否则按版本阶梯逐级迁移：`if current < 1 { migrate_v1(conn) }`（`:29-31`）。
4. 写入 `PRAGMA user_version = SCHEMA_VERSION`（`:34-35`）。

注：`PRAGMA` 不支持参数绑定，这里用 `format!` 字符串拼接；`SCHEMA_VERSION`
是编译期常量，安全（`:33` 注释）。

### 5.3 `migrate_v1()`（`migrations.rs:42-88`）

一个 `execute_batch` 建两张表、两个索引。全部使用 `IF NOT EXISTS`，
因此重复执行安全 —— 测试 `migration_is_idempotent`（`:122-132`）验证第二次
调用不报错且版本不变。

### 5.4 后续版本怎么加

新增列走 `ALTER TABLE … ADD COLUMN`（SQLite 支持）。`SCHEMA_VERSION` 加一，
`run()` 里加一条 `if current < N { migrate_vN(conn) }`。**不要改动既有列的
类型或含义**（`migrations.rs:6-8`）。

---

## 6. 连接参数与韧性

### 6.1 `Store::tune()`（`store/types.rs:110-116`）

在跑迁移**之前**设置：

| pragma | 值 | 作用 |
| --- | --- | --- |
| `journal_mode` | `WAL` | 读写并发 |
| `synchronous` | `NORMAL` | 性能/安全折中 |
| `foreign_keys` | `ON` | 级联删除生效 |
| `busy_timeout` | `5000` | 锁等待 5 秒 |

### 6.2 打开与恢复（`store/types.rs`）

| 方法 | 行 | 行为 |
| --- | --- | --- |
| `Store::open()` | 36 | 打开后跑 `PRAGMA integrity_check`（`:51-52`），损坏则 `bail` |
| `Store::open_or_recover()` | 71 | 检测到损坏时把坏库改名备份，重建空库 |

---

## 7. 仓储层接口

上层（`download_manager`）只操作结构体，不写 SQL；schema 改动的影响面被限制在
`repo.rs` + `migrations.rs`（`repo.rs:1-5` 注释）。

### `TaskRepo`（`repo.rs`）

| 方法 | 行 | 说明 |
| --- | --- | --- |
| `upsert_new` | 36 | 插入或更新任务元信息，**不覆盖** state / 进度 / 错误 |
| `delete` | 57 | 删任务（连同图片明细，走级联） |
| `clear` | 69 | 清空全部任务 |
| `set_state` | 78 | 设状态，顺带记错误信息 |
| `set_retry_count` | 98 | 更新重试次数 |
| `set_total_img_count` | 110 | 记录图片总数（拿到 `ImgList` 后调一次） |
| `set_progress` | 131 | 更新已完成数 |
| `get` | 145 | 取单个任务 |
| `list` | 159 | 取全部任务 |
| `list_resumable` | 180 | 取未完成任务（启动恢复用） |
| `stats` | 201 | 聚合统计 → `TaskStats` |

### `ImageRepo`（`repo.rs`）

| 方法 | 行 | 说明 |
| --- | --- | --- |
| `insert_many` | 245 | 批量插入图片行 |
| `mark_done` | 273 | 标记单图完成 |
| `mark_failed` | 308 | 标记单图失败 |
| `list_pending_indexes` | 334 | 列出未完成图片的索引 |
| `list` | 355 | 列出某本漫画的全部图片行 |
| `count_done` | 374 | 统计已完成数 |
| `clear` | 388 | 清空某本漫画的图片行 |

### `TaskStats`（`repo.rs:14-28`）

```rust
pub struct TaskStats {
    pub total: i64,
    pub pending: i64,
    pub downloading: i64,
    pub completed: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub total_img_count: i64,
    pub done_img_count: i64,
}
```

`#[serde(rename_all = "camelCase")]` → 输出 `totalImgCount` / `doneImgCount`。
由 `GET /api/tasks/stats` 返回（见 `API.md`）。

---

## 8. 交叉核对

| 核对项 | 文档值 | 代码值 | 结果 |
| --- | --- | --- | --- |
| `download_task` 列数 | 10 | 10 | ✅ |
| `download_image` 列数 | 9 | 9（8 列 + 1 复合主键） | ✅ |
| 索引数 | 2 | 2 | ✅ |
| 外键数 | 1 | 1 | ✅ |
| `SCHEMA_VERSION` | 1 | 1 | ✅ |
