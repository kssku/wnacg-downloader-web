//! Store 的核心类型：连接句柄 + 行映射结构。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::migrations;

/// 当前 Unix 时间戳（秒）。所有 `updated_at` 都走它，方便跨表比较。
pub fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 数据库句柄。
///
/// 内部用 `Mutex<Connection>` 而不是连接池：写操作全部集中在状态迁移点
/// （创建任务、图片完成、状态变更），这些点本身就不高频，单连接足够；
/// 而 SQLite 的并发写本来就要串行化，池化只会把等待点从应用层挪到锁层，
/// 并不能提高写入吞吐。
///
/// WAL 模式下读不阻塞写，所以即便是长列表查询也不会卡住下载。
#[derive(Clone)]
pub struct Store {
    conn: Arc<parking_lot::Mutex<Connection>>,
    path: Arc<PathBuf>,
}

/// schema 迁移前的整库备份。
///
/// **为什么必须备份**：v2 是破坏性迁移（`DROP TABLE` + 重建），一旦
/// `COMMIT` 成功就无法从 SQL 层回退。备份文件是用户最后的退路。
///
/// 几个刻意的选择：
///
/// - **覆盖所有版本升级**：条件用 `current < SCHEMA_VERSION`，而不是
///   `current == 1`。将来加 v3/v4 时，这条路径自动生效，不需要改。
/// - **在 `BEGIN` 之前复制**：事务回滚不该影响备份 —— 迁移失败时，
///   备份仍然在，用户可以手工恢复。
/// - **已存在则不覆盖，只记日志**：重复启动（比如上次迁移失败后重启）
///   时，第一次的备份才是「迁移前」的真实快照；覆盖它反而丢掉原始状态。
/// - **复制失败即中止**：返回 `Err` 让调用方跳过迁移，宁可不迁移，
///   也不能无备份改表。
///
/// `existed_before_open` 必须由调用方在 `Connection::open` **之前**采样
/// —— open 会创建文件，之后再判断就分不清全新库和已有库了。
fn backup_before_migration(
    db_path: &Path,
    existed_before_open: bool,
    current: i64,
) -> anyhow::Result<()> {
    // 已经是最新（或更新，虽然后者会在 migrations::run 里被拒）——无需备份。
    if current >= migrations::SCHEMA_VERSION {
        return Ok(());
    }

    // 全新库：没有旧数据要保护。直接让 migrations 建 v1→v2 表结构。
    if !existed_before_open {
        tracing::info!("数据库不存在，无需迁移前备份（将直接建最新表结构）");
        return Ok(());
    }

    let backup = PathBuf::from(format!("{}.pre-v2", db_path.display()));
    if backup.exists() {
        tracing::warn!(
            err_title = "迁移前备份文件已存在，不覆盖",
            backup = %backup.display(),
            "保留首次备份作为迁移前快照；如需重新迁移请先手工处理该文件"
        );
        return Ok(());
    }

    std::fs::copy(db_path, &backup).with_context(|| {
        format!(
            "迁移前备份数据库失败：`{}` → `{}`。已中止迁移，数据库未被修改。",
            db_path.display(),
            backup.display()
        )
    })?;

    let bytes = std::fs::metadata(&backup).map(|m| m.len()).unwrap_or(0);
    tracing::info!(
        backup = %backup.display(),
        bytes,
        from_version = current,
        to_version = migrations::SCHEMA_VERSION,
        "已在迁移前备份数据库"
    );
    Ok(())
}

impl Store {
    /// 打开（或创建）数据库，并跑完迁移。
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建数据库目录 `{}` 失败", parent.display()))?;
        }

        // **必须在 `Connection::open` 之前判断**：open 会创建文件，
        // 之后再判断 `exists()` 就分不清「全新库」和「已有库」了。
        // 全新库没有旧数据要保护，跳过备份；已有库若落后于当前
        // schema 版本，则先备份再迁移。
        let existed_before_open = db_path.exists();

        let conn = Connection::open(db_path)
            .with_context(|| format!("打开数据库 `{}` 失败", db_path.display()))?;

        Self::tune(&conn)?;

        // 启动时做一次完整性检查。损坏时**不阻塞启动**——下载服务本身的
        // 可用性比历史任务记录更重要。调用方拿到 `Err` 后应该重命名旧库、
        // 重建一个空库，并把状态降级为「无历史」。
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .context("执行 `integrity_check` 失败")?;
        if integrity != "ok" {
            anyhow::bail!(
                "数据库 `{}` 完整性检查未通过：{integrity}",
                db_path.display()
            );
        }

        // 迁移前备份。判断条件是「库已存在且落后于当前 schema 版本」——
        // 这样将来 v2→v3 等所有版本升级都自动走这条路径，不需要每加一版
        // 就改一次这里。
        //
        // 注意时机：备份必须在 `migrations::run` 的 `BEGIN` 之前完成，
        // 事务回滚不该影响备份；备份失败则**中止**，不调 migrations::run
        // ——「不能无备份改表」的落点就在这里。
        let current: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .context("读取 user_version 失败")?;
        backup_before_migration(db_path, existed_before_open, current)?;

        migrations::run(&conn).context("执行数据库迁移失败")?;

        // 迁移会临时关掉外键（`DROP TABLE` 需要），并在结束时恢复。
        // **恢复万一没生效，`download_image` 的级联删除会静默失效**
        // ——删任务留下孤儿图片行，而且不报任何错。所以在打开时
        // 硬校验一次，不依赖 `tune()` 那行 pragma 的"应该生效"。
        //
        // 这里不用 `tracing::warn!`：日志在 `main.rs` 里比 store 晚初始化，
        // 此刻打日志会丢。直接返回错误，让调用方看到。
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .context("读取 foreign_keys 失败")?;
        if fk != 1 {
            anyhow::bail!(
                "数据库 `{}` 打开后 `foreign_keys` 为 {fk}（应为 1）。\
                 级联删除会失效，拒绝继续以免产生孤儿图片行。",
                db_path.display()
            );
        }

        Ok(Self {
            conn: Arc::new(parking_lot::Mutex::new(conn)),
            path: Arc::new(db_path.to_path_buf()),
        })
    }

    /// 打开数据库；**真损坏**时把旧库改名备份后重建空库。
    ///
    /// 这是启动路径应该用的入口：宁可丢历史任务记录，也不能让服务起不来。
    ///
    /// **例外**：schema 版本高于程序支持（[`migrations::SchemaTooNew`]）
    /// 不是损坏 —— 库和数据都完好，只是程序太旧。这种情况直接向上传播，
    /// 拒绝启动，否则会把用户的数据当成损坏库删掉重建。
    pub fn open_or_recover(db_path: &Path) -> anyhow::Result<Self> {
        match Self::open(db_path) {
            Ok(store) => Ok(store),
            Err(err) => {
                // schema 版本过高是逻辑错误，不该触发重建 —— 否则会静默删除用户数据。
                if err.downcast_ref::<migrations::SchemaTooNew>().is_some() {
                    return Err(err);
                }

                tracing::error!(
                    err_title = "数据库损坏，将备份旧库并重建",
                    db_path = %db_path.display(),
                    message = %err
                );

                if db_path.exists() {
                    let backup = db_path.with_extension(format!("db.corrupt-{}", now_ts()));
                    std::fs::rename(db_path, &backup).with_context(|| {
                        format!(
                            "备份损坏的数据库 `{}` 到 `{}` 失败",
                            db_path.display(),
                            backup.display()
                        )
                    })?;
                    tracing::warn!(
                        err_title = "已备份损坏的数据库",
                        backup = %backup.display()
                    );
                }

                // WAL 的两个附属文件也要清掉，否则重建的库会继承旧 WAL。
                for suffix in ["-wal", "-shm"] {
                    let side = PathBuf::from(format!("{}{suffix}", db_path.display()));
                    if side.exists() {
                        let _ = std::fs::remove_file(&side);
                    }
                }

                Self::open(db_path)
            }
        }
    }

    /// WAL + 外键 + 忙等待。三件事必须一起设。
    fn tune(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;      -- 读不阻塞写
            PRAGMA synchronous = NORMAL;    -- WAL 下的安全/性能平衡点
            PRAGMA foreign_keys = ON;       -- download_image 的级联删除依赖它
            PRAGMA busy_timeout = 5000;     -- 写冲突时等 5s 而不是立刻报错
            "#,
        )
        .context("设置数据库 pragma 失败")?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 借出连接。调用方持锁期间不要做网络 IO。
    pub fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let conn = self.conn.lock();
        f(&conn)
    }

    /// 在事务里执行。闭包返回 `Err` 时自动回滚。
    pub fn with_tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().context("开启事务失败")?;
        let value = f(&tx)?;
        tx.commit().context("提交事务失败")?;
        Ok(value)
    }
}

/// 漫画级任务状态。与原桌面版 `DownloadTaskState` 一一对应。
///
/// 序列化成小写字符串（`"pending"` / `"downloading"` …），与 `as_str()`
/// 以及数据库里存的字面量保持一致，前端可以不加转换直接判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbTaskState {
    Pending,
    Downloading,
    Paused,
    Cancelled,
    Completed,
    Failed,
}

impl DbTaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "pending" => Self::Pending,
            "downloading" => Self::Downloading,
            "paused" => Self::Paused,
            "cancelled" => Self::Cancelled,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            other => anyhow::bail!("未知的漫画任务状态 `{other}`"),
        })
    }

    /// 终态：不会再自动发生变化。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

/// 图片级状态。刻意只有三态：断点续传只关心「要不要重下」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbImageState {
    Pending,
    Done,
    Failed,
}

impl DbImageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "pending" => Self::Pending,
            "done" => Self::Done,
            "failed" => Self::Failed,
            other => anyhow::bail!("未知的图片状态 `{other}`"),
        })
    }
}

/// `download_task` 的一行。
///
/// 主键是 `chapter_id`（TEXT），与 jmcomic/picacomic 的「漫画 → 章节 →
/// 图片」三级模型对齐。**wnacg 站点没有章节层**，所以它是合成单章节
/// 在 DB 层的投影：`chapter_id` 恒等于 `comic_id`、`chapter_title`
/// 恒等于 `comic_title`、`chapter_order` 恒为 1 —— 与
/// `Comic::from_html` 里合成的那个唯一 `ChapterInfo` 一一对应。
///
/// v1 的主键是 `comic_id INTEGER`；v2 迁移（见 `migrations::migrate_v2`）
/// 重建了这张表，把主键换成 `chapter_id TEXT` 并保留 `comic_id` 为普通列。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbTask {
    pub chapter_id: String,
    pub comic_id: String,
    pub comic_title: String,
    pub chapter_title: String,
    pub chapter_order: i64,
    pub state: DbTaskState,
    pub total_img_count: i64,
    pub done_img_count: i64,
    pub retry_count: i64,
    pub last_error: Option<String>,
    /// 下载目录快照。恢复时必须用任务自己当初的目录，而不是当前配置，
    /// 否则用户中途改了下载目录，恢复时就会找不到已下载的文件。
    pub download_dir: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// `download_image` 的一行。
///
/// `chapter_id` 是外键，指向 `download_task.chapter_id`。
/// 对 wnacg 而言它恒等于所属漫画的 `comic_id`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbImage {
    pub chapter_id: String,
    pub img_index: i64,
    pub url: String,
    pub state: DbImageState,
    pub retry_count: i64,
    pub last_error: Option<String>,
    pub bytes: Option<i64>,
    pub updated_at: i64,
}

impl DbTask {
    /// 从查询结果里读一行。
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let state_str: String = row.get("state")?;
        let state = DbTaskState::parse(&state_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;
        Ok(Self {
            chapter_id: row.get("chapter_id")?,
            comic_id: row.get("comic_id")?,
            comic_title: row.get("comic_title")?,
            chapter_title: row.get("chapter_title")?,
            chapter_order: row.get("chapter_order")?,
            state,
            total_img_count: row.get("total_img_count")?,
            done_img_count: row.get("done_img_count")?,
            retry_count: row.get("retry_count")?,
            last_error: row.get("last_error")?,
            download_dir: row.get("download_dir")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

impl DbImage {
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let state_str: String = row.get("state")?;
        let state = DbImageState::parse(&state_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;
        Ok(Self {
            chapter_id: row.get("chapter_id")?,
            img_index: row.get("img_index")?,
            url: row.get("url")?,
            state,
            retry_count: row.get("retry_count")?,
            last_error: row.get("last_error")?,
            bytes: row.get("bytes")?,
            updated_at: row.get("updated_at")?,
        })
    }
}
