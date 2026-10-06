//! Schema 版本管理与迁移。
//!
//! 用 `PRAGMA user_version` 记录 schema 版本：SQLite 自带的这个整数位
//! 不需要额外建表，也不用担心和业务表混淆。
//!
//! 迁移必须**幂等且向前兼容**：容器镜像升级后旧库要能直接用。
//!
//! 一般原则是「新增列一律走 `ALTER TABLE ADD COLUMN`（SQLite 支持），
//! 不改动既有列的类型或含义」——`ALTER TABLE ADD COLUMN` 成本低、
//! 无数据搬运、失败也不损库。
//!
//! **v2 是这条原则的例外**：它要把 `download_task` 的主键从
//! `comic_id INTEGER` 换成 `chapter_id TEXT`，并给 `download_image`
//! 换外键。SQLite 不支持改主键，只能「建新表 → 搬数据 → 删旧表 → 改名」
//! —— 这是破坏性操作，一旦 `COMMIT` 就无法从 SQL 层回退，因此
//! **调用方必须在迁移前整库备份**（见 `Store::open` 里的
//! `backup_before_migration`）。
//!
//! 以后若再遇到必须重建表的迁移，同样要保证「有备份才迁移」。

use anyhow::Context;
use rusqlite::Connection;

/// 当前代码期望的 schema 版本。
pub const SCHEMA_VERSION: i64 = 2;

/// schema 版本高于程序支持。
///
/// **与「数据库损坏」是不同性质**：这是逻辑错误（库被更新版本的程序建过），
/// 库本身完好、数据也完好，不该触发重建 —— 否则会静默删除用户数据。
/// [`crate::store::Store::open_or_recover`] 用 `downcast_ref` 识别它并
/// 直接向上传播，不进入损坏重建路径。
#[derive(Debug)]
pub struct SchemaTooNew {
    pub current: i64,
    pub supported: i64,
}

impl std::fmt::Display for SchemaTooNew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "数据库 schema 版本为 `{}`，高于本程序支持的 `{}`。\
             请升级 wnacg-server，或备份后删除 `wnacg_server.db` 重建。",
            self.current, self.supported
        )
    }
}

impl std::error::Error for SchemaTooNew {}

/// 按需把连接上的库升到 [`SCHEMA_VERSION`]。
pub fn run(conn: &Connection) -> anyhow::Result<()> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("读取 user_version 失败")?;

    if current > SCHEMA_VERSION {
        // 降级场景：DB 是更新版本的程序建的。库完好，只是程序太旧。
        // 返回专属错误类型而非 anyhow!，好让 open_or_recover 区分
        // 「版本过高」和「真损坏」—— 前者拒绝启动，后者才重建。
        return Err(anyhow::Error::new(SchemaTooNew {
            current,
            supported: SCHEMA_VERSION,
        }));
    }

    if current == SCHEMA_VERSION {
        tracing::debug!(current, "数据库 schema 已是最新");
        return Ok(());
    }

    tracing::info!(from = current, to = SCHEMA_VERSION, "开始迁移数据库 schema");

    if current < 1 {
        migrate_v1(conn).context("migrate_v1 失败")?;
    }

    if current < 2 {
        migrate_v2(conn).context("migrate_v2 失败")?;
    }

    // PRAGMA 不支持参数绑定，只能字符串拼接；这里 SCHEMA_VERSION 是编译期常量，安全。
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
        .context("写入 user_version 失败")?;

    tracing::info!(version = SCHEMA_VERSION, "数据库 schema 迁移完成");
    Ok(())
}

/// v1：下载任务表 + 图片明细表。
fn migrate_v1(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        r#"
        -- 一本漫画一行。wnacg 的任务粒度就是整本漫画，没有章节层。
        CREATE TABLE IF NOT EXISTS download_task (
            comic_id         INTEGER PRIMARY KEY,
            comic_title      TEXT    NOT NULL,
            -- 取值见 DbTaskState::as_str()：pending / downloading / paused /
            -- cancelled / completed / failed
            state            TEXT    NOT NULL DEFAULT 'pending',
            total_img_count  INTEGER NOT NULL DEFAULT 0,
            done_img_count   INTEGER NOT NULL DEFAULT 0,
            retry_count      INTEGER NOT NULL DEFAULT 0,
            last_error       TEXT,
            -- 任务创建时的下载目录快照，恢复时用这个而不是当前配置
            download_dir     TEXT    NOT NULL DEFAULT '',
            created_at       INTEGER NOT NULL DEFAULT 0,
            updated_at       INTEGER NOT NULL DEFAULT 0
        );

        -- 恢复时按「未完成任务」筛选，走这个索引。
        CREATE INDEX IF NOT EXISTS idx_task_state_updated
            ON download_task (state, updated_at DESC);

        -- 一张图一行。断点续传的最小单位。
        CREATE TABLE IF NOT EXISTS download_image (
            comic_id     INTEGER NOT NULL,
            img_index    INTEGER NOT NULL,
            url          TEXT    NOT NULL DEFAULT '',
            -- pending / done / failed
            state        TEXT    NOT NULL DEFAULT 'pending',
            retry_count  INTEGER NOT NULL DEFAULT 0,
            last_error   TEXT,
            bytes        INTEGER,
            updated_at   INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (comic_id, img_index),
            FOREIGN KEY (comic_id) REFERENCES download_task (comic_id) ON DELETE CASCADE
        );

        -- 找出某本漫画里还没下完的图。
        CREATE INDEX IF NOT EXISTS idx_image_state
            ON download_image (comic_id, state);
        "#,
    )
    .context("创建 v1 表结构失败")?;

    Ok(())
}

/// v2：任务粒度从「整本漫画」改为「章节」，对齐 jmcomic/picacomic 的 schema。
///
/// `download_task` 主键 `comic_id INTEGER` → `chapter_id TEXT`，并补
/// `comic_id TEXT` / `chapter_title` / `chapter_order` 三列；
/// `download_image` 的外键同步从 `comic_id` 换成 `chapter_id`。
///
/// **wnacg 没有章节层**，所以这是「合成单章节」在 DB 层的投影：
/// `chapter_id` 值恒等于 `comic_id` 的字符串形式，`chapter_title` 取
/// 漫画标题，`chapter_order` 恒为 1 —— 与 `Comic::from_html` 里合成的
/// 那个唯一 `ChapterInfo` 完全对应。
///
/// **破坏性**：SQLite 不支持改主键，只能重建表。调用方必须已做迁移前备份
/// （`Store::open` → `backup_before_migration`）。
///
/// 实现要点：
///
/// - **`PRAGMA foreign_keys = OFF` 必须在事务外**：SQLite 的外键开关
///   在事务开始后无效，放事务里等于没关，`DROP TABLE download_task`
///   会被 `download_image` 的外键挡住。
/// - **`CAST(comic_id AS TEXT)` 显式转换**：旧列是 INTEGER 亲和性，
///   直接 `SELECT comic_id` 到 TEXT 列虽然 SQLite 也会转，但显式写出来
///   才能让「这里的转换是有意为之」这件事留在代码里。
/// - **`foreign_key_check` 收尾校验**：迁移后跑一次，有任何孤儿外键
///   会返回行；这里断言它为空，否则数据已被破坏，不如直接失败。
fn migrate_v2(conn: &Connection) -> anyhow::Result<()> {
    // 必须在事务外关外键，否则 DROP 会被 download_image 的引用挡住。
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .context("关闭外键约束失败")?;

    let result = conn.execute_batch(
        r#"
        BEGIN;

        -- 1. download_task：主键换成 chapter_id TEXT
        CREATE TABLE download_task_new (
            chapter_id      TEXT PRIMARY KEY,
            comic_id        TEXT NOT NULL,
            comic_title     TEXT NOT NULL,
            chapter_title   TEXT NOT NULL,
            chapter_order   INTEGER NOT NULL DEFAULT 0,
            state           TEXT NOT NULL DEFAULT 'pending',
            total_img_count INTEGER NOT NULL DEFAULT 0,
            done_img_count  INTEGER NOT NULL DEFAULT 0,
            retry_count     INTEGER NOT NULL DEFAULT 0,
            last_error      TEXT,
            download_dir    TEXT NOT NULL DEFAULT '',
            created_at      INTEGER NOT NULL DEFAULT 0,
            updated_at      INTEGER NOT NULL DEFAULT 0
        );

        INSERT INTO download_task_new
        SELECT
            CAST(comic_id AS TEXT),   -- chapter_id = comic_id 的字符串形式
            CAST(comic_id AS TEXT),   -- comic_id 保留原值
            comic_title,
            comic_title,              -- chapter_title = 漫画标题（合成单章节）
            1,                        -- chapter_order
            state, total_img_count, done_img_count, retry_count,
            last_error, download_dir, created_at, updated_at
        FROM download_task;

        DROP TABLE download_task;
        ALTER TABLE download_task_new RENAME TO download_task;

        CREATE INDEX IF NOT EXISTS idx_task_state_updated
            ON download_task (state, updated_at DESC);

        -- 2. download_image：外键指向 chapter_id
        CREATE TABLE download_image_new (
            chapter_id   TEXT NOT NULL,
            img_index    INTEGER NOT NULL,
            url          TEXT NOT NULL DEFAULT '',
            state        TEXT NOT NULL DEFAULT 'pending',
            retry_count  INTEGER NOT NULL DEFAULT 0,
            last_error   TEXT,
            bytes        INTEGER,
            updated_at   INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (chapter_id, img_index),
            FOREIGN KEY (chapter_id) REFERENCES download_task (chapter_id) ON DELETE CASCADE
        );

        INSERT INTO download_image_new
        SELECT
            CAST(comic_id AS TEXT),
            img_index, url, state, retry_count, last_error, bytes, updated_at
        FROM download_image;

        DROP TABLE download_image;
        ALTER TABLE download_image_new RENAME TO download_image;

        CREATE INDEX IF NOT EXISTS idx_image_state
            ON download_image (chapter_id, state);

        COMMIT;
        "#,
    );

    // 无论成败都要把外键开关恢复，避免连接后续行为异常。
    let restore = conn.execute_batch("PRAGMA foreign_keys = ON;");

    result.context("迁移 v1→v2 失败")?;
    restore.context("恢复外键约束失败")?;

    // 收尾校验：迁移后不该有孤儿外键。
    let mut stmt = conn
        .prepare("PRAGMA foreign_key_check;")
        .context("准备外键校验失败")?;
    let mut rows = stmt.query([]).context("执行外键校验失败")?;
    if rows.next().context("读取外键校验结果失败")?.is_some() {
        anyhow::bail!("迁移 v1→v2 后外键校验未通过：存在孤儿 download_image 行");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    #[test]
    fn migration_creates_tables_and_bumps_version() {
        let conn = mem();
        run(&conn).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        for table in ["download_task", "download_image"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "表 {table} 未创建");
        }
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = mem();
        run(&conn).unwrap();
        // 第二次跑不应该报错，也不应该重复建表失败。
        run(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn deleting_task_cascades_to_images() {
        let conn = mem();
        run(&conn).unwrap();
        conn.execute(
            "INSERT INTO download_task
                 (chapter_id, comic_id, comic_title, chapter_title, chapter_order)
             VALUES ('1', '1', '测试', '测试', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO download_image (chapter_id, img_index) VALUES ('1', 0)",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM download_task WHERE chapter_id = '1'", [])
            .unwrap();

        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM download_image", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "级联删除未生效");
    }
}
