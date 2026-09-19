//! Schema 版本管理与迁移。
//!
//! 用 `PRAGMA user_version` 记录 schema 版本：SQLite 自带的这个整数位
//! 不需要额外建表，也不用担心和业务表混淆。
//!
//! 迁移必须**幂等且向前兼容**：容器镜像升级后旧库要能直接用。
//! 因此新增列一律走 `ALTER TABLE ADD COLUMN`（SQLite 支持），
//! 不要改动既有列的类型或含义。

use anyhow::Context;
use rusqlite::Connection;

/// 当前代码期望的 schema 版本。
pub const SCHEMA_VERSION: i64 = 1;

/// 按需把连接上的库升到 [`SCHEMA_VERSION`]。
pub fn run(conn: &Connection) -> anyhow::Result<()> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("读取 user_version 失败")?;

    if current >= SCHEMA_VERSION {
        tracing::debug!(current, "数据库 schema 已是最新");
        return Ok(());
    }

    tracing::info!(from = current, to = SCHEMA_VERSION, "开始迁移数据库 schema");

    if current < 1 {
        migrate_v1(conn).context("migrate_v1 失败")?;
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
            -- pending / running / completed / failed / cancelled
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
            "INSERT INTO download_task (comic_id, comic_title) VALUES (1, '测试')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO download_image (comic_id, img_index) VALUES (1, 0)",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM download_task WHERE comic_id = 1", [])
            .unwrap();

        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM download_image", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "级联删除未生效");
    }
}
