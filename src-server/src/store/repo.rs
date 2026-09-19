//! 仓储层：把 SQL 关在 `TaskRepo` / `ImageRepo` 两个命名空间里。
//!
//! 上层（`download_manager`）只操作结构体，不写 SQL；
//! 这样 schema 改动的影响面就被限制在本文件 + `migrations.rs`。

use anyhow::Context;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::types::{now_ts, DbImage, DbTask, DbTaskState, Store};

/// 任务列表的聚合统计，给前端顶部卡片用。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStats {
    pub total: i64,
    pub pending: i64,
    pub downloading: i64,
    pub completed: i64,
    pub failed: i64,
    pub cancelled: i64,
    /// 所有任务的图片总数（= 用户预期的下载量）。
    pub total_img_count: i64,
    /// 所有任务已完成下载的图片数（= 实际落盘量）。
    pub done_img_count: i64,
}

/// 漫画任务表。
pub struct TaskRepo;

impl TaskRepo {
    /// 插入或更新任务元信息。
    ///
    /// **不覆盖 `state` / 进度 / 错误**：重下同一本漫画时，
    /// 那些字段由下载过程自己按步推进，这里只负责标题和目录快照。
    pub fn upsert_new(store: &Store, comic_id: i64, title: &str, dir: &str) -> anyhow::Result<()> {
        let now = now_ts();
        store.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO download_task
                    (comic_id, comic_title, state, download_dir, created_at, updated_at)
                VALUES (?1, ?2, 'pending', ?3, ?4, ?4)
                ON CONFLICT(comic_id) DO UPDATE SET
                    comic_title  = excluded.comic_title,
                    download_dir = excluded.download_dir,
                    updated_at   = excluded.updated_at
                "#,
                params![comic_id, title, dir, now],
            )
            .context("upsert download_task 失败")?;
            Ok(())
        })
    }

    /// 从列表里移除任务（连同图片明细）。
    pub fn delete(store: &Store, comic_id: i64) -> anyhow::Result<()> {
        store.with_conn(|conn| {
            conn.execute(
                "DELETE FROM download_task WHERE comic_id = ?1",
                params![comic_id],
            )
            .context("删除 download_task 失败")?;
            Ok(())
        })
    }

    /// 清空全部任务。
    pub fn clear(store: &Store) -> anyhow::Result<()> {
        store.with_conn(|conn| {
            conn.execute("DELETE FROM download_task", [])
                .context("清空 download_task 失败")?;
            Ok(())
        })
    }

    /// 设置任务状态，并顺带记错误信息。
    pub fn set_state(
        store: &Store,
        comic_id: i64,
        state: DbTaskState,
        err: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = now_ts();
        store.with_conn(|conn| {
            conn.execute(
                "UPDATE download_task
                    SET state = ?2, last_error = ?3, updated_at = ?4
                  WHERE comic_id = ?1",
                params![comic_id, state.as_str(), err, now],
            )
            .context("更新任务状态失败")?;
            Ok(())
        })
    }

    /// 更新重试次数。
    pub fn set_retry_count(store: &Store, comic_id: i64, retry_count: i64) -> anyhow::Result<()> {
        store.with_conn(|conn| {
            conn.execute(
                "UPDATE download_task SET retry_count = ?2, updated_at = ?3 WHERE comic_id = ?1",
                params![comic_id, retry_count, now_ts()],
            )
            .context("更新任务重试次数失败")?;
            Ok(())
        })
    }

    /// 记录图片总数（拿到 `ImgList` 之后调用一次）。
    pub fn set_total_img_count(
        store: &Store,
        comic_id: i64,
        total: i64,
    ) -> anyhow::Result<()> {
        store.with_conn(|conn| {
            conn.execute(
                "UPDATE download_task
                    SET total_img_count = ?2, done_img_count = 0, updated_at = ?3
                  WHERE comic_id = ?1",
                params![comic_id, total, now_ts()],
            )
            .context("更新任务图片总数失败")?;
            Ok(())
        })
    }

    /// 直接覆盖进度计数。
    ///
    /// 只有在批量重算（比如重下完成、恢复核对文件系统）时才用；
    /// 单张图片完成走 [`ImageRepo::mark_done`]，那里是原子自增。
    pub fn set_progress(store: &Store, comic_id: i64, done: i64) -> anyhow::Result<()> {
        store.with_conn(|conn| {
            conn.execute(
                "UPDATE download_task
                    SET done_img_count = ?2, updated_at = ?3
                  WHERE comic_id = ?1",
                params![comic_id, done, now_ts()],
            )
            .context("更新任务进度失败")?;
            Ok(())
        })
    }

    /// 按主键读一行。
    pub fn get(store: &Store, comic_id: i64) -> anyhow::Result<Option<DbTask>> {
        store.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT * FROM download_task WHERE comic_id = ?1")
                .context("准备查询任务失败")?;
            let row = stmt
                .query_row(params![comic_id], DbTask::from_row)
                .optional()
                .context("查询任务失败")?;
            Ok(row)
        })
    }

    /// 列出全部任务，最近更新的在前。
    pub fn list(store: &Store) -> anyhow::Result<Vec<DbTask>> {
        store.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT * FROM download_task ORDER BY updated_at DESC")
                .context("准备任务列表查询失败")?;
            let rows = stmt
                .query_map([], DbTask::from_row)
                .context("查询任务列表失败")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.context("读取任务行失败")?);
            }
            Ok(out)
        })
    }

    /// 列出可以自动恢复的任务：未到终态的。
    ///
    /// `paused` 与 `cancelled` 里只有 `cancelled` 是终态，
    /// 但暂停任务**不应该**在重启后自动跑起来——那是用户的显式意图。
    /// 所以这里只捞 `pending` / `downloading` / `failed`。
    pub fn list_resumable(store: &Store) -> anyhow::Result<Vec<DbTask>> {
        store.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM download_task
                      WHERE state IN ('pending', 'downloading', 'failed')
                      ORDER BY updated_at ASC",
                )
                .context("准备恢复任务查询失败")?;
            let rows = stmt
                .query_map([], DbTask::from_row)
                .context("查询恢复任务失败")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.context("读取恢复任务行失败")?);
            }
            Ok(out)
        })
    }

    /// 聚合统计。一条 SQL 出全部计数，避免前端拉全表自己算。
    pub fn stats(store: &Store) -> anyhow::Result<TaskStats> {
        store.with_conn(|conn| {
            let stats = conn
                .query_row(
                    r#"
                    SELECT
                        COUNT(*)                                                    AS total,
                        COALESCE(SUM(state = 'pending'),     0)                     AS pending,
                        COALESCE(SUM(state = 'downloading'), 0)                     AS downloading,
                        COALESCE(SUM(state = 'completed'),   0)                     AS completed,
                        COALESCE(SUM(state = 'failed'),      0)                     AS failed,
                        COALESCE(SUM(state = 'cancelled'),   0)                     AS cancelled,
                        COALESCE(SUM(total_img_count),       0)                     AS total_img_count,
                        COALESCE(SUM(done_img_count),        0)                     AS done_img_count
                      FROM download_task
                    "#,
                    [],
                    |row| {
                        Ok(TaskStats {
                            total: row.get("total")?,
                            pending: row.get("pending")?,
                            downloading: row.get("downloading")?,
                            completed: row.get("completed")?,
                            failed: row.get("failed")?,
                            cancelled: row.get("cancelled")?,
                            total_img_count: row.get("total_img_count")?,
                            done_img_count: row.get("done_img_count")?,
                        })
                    },
                )
                .context("统计任务失败")?;
            Ok(stats)
        })
    }
}

/// 图片明细表。
pub struct ImageRepo;

impl ImageRepo {
    /// 登记一本漫画的全部图片（拿到 `ImgList` 后调用）。
    ///
    /// 用 `ON CONFLICT DO NOTHING`：已经 `done` 的图片不会被重置成 `pending`，
    /// 这是断点续传不重下的关键。
    pub fn insert_many(
        store: &Store,
        comic_id: i64,
        urls: &[String],
    ) -> anyhow::Result<()> {
        let now = now_ts();
        store.with_tx(|tx| {
            let mut stmt = tx
                .prepare(
                    r#"
                    INSERT INTO download_image (comic_id, img_index, url, state, updated_at)
                    VALUES (?1, ?2, ?3, 'pending', ?4)
                    ON CONFLICT(comic_id, img_index) DO NOTHING
                    "#,
                )
                .context("准备插入图片失败")?;
            for (index, url) in urls.iter().enumerate() {
                stmt.execute(params![comic_id, index as i64, url, now])
                    .context("插入图片行失败")?;
            }
            Ok(())
        })
    }

    /// 标记单张图片完成，并原子地推进任务进度。
    ///
    /// 两个写在同一事务里，避免出现「图片已 done 但计数没加」的中间态——
    /// 那会让恢复逻辑把已完成的任务当成未完成。
    pub fn mark_done(
        store: &Store,
        comic_id: i64,
        img_index: i64,
        bytes: Option<i64>,
    ) -> anyhow::Result<()> {
        let now = now_ts();
        store.with_tx(|tx| {
            let changed = tx
                .execute(
                    r#"
                    UPDATE download_image
                       SET state = 'done', last_error = NULL, bytes = ?3, updated_at = ?4
                     WHERE comic_id = ?1 AND img_index = ?2 AND state != 'done'
                    "#,
                    params![comic_id, img_index, bytes, now],
                )
                .context("标记图片完成失败")?;

            // 只有真的从「未完成」翻成「完成」才加计数，
            // 否则重复调用会把 done_img_count 顶到超过总数。
            if changed > 0 {
                tx.execute(
                    "UPDATE download_task
                        SET done_img_count = done_img_count + 1, updated_at = ?2
                      WHERE comic_id = ?1",
                    params![comic_id, now],
                )
                .context("推进任务进度失败")?;
            }
            Ok(())
        })
    }

    /// 标记单张图片失败并记原因。
    pub fn mark_failed(
        store: &Store,
        comic_id: i64,
        img_index: i64,
        err: &str,
    ) -> anyhow::Result<()> {
        store.with_conn(|conn| {
            conn.execute(
                r#"
                UPDATE download_image
                   SET state = 'failed',
                       retry_count = retry_count + 1,
                       last_error = ?3,
                       updated_at = ?4
                 WHERE comic_id = ?1 AND img_index = ?2
                "#,
                params![comic_id, img_index, err, now_ts()],
            )
            .context("标记图片失败失败")?;
            Ok(())
        })
    }

    /// 列出某本漫画所有**未完成**的图片下标，按顺序返回。
    ///
    /// 恢复时用它决定要重下哪些，而不是「整本重下」。
    pub fn list_pending_indexes(store: &Store, comic_id: i64) -> anyhow::Result<Vec<i64>> {
        store.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT img_index FROM download_image
                      WHERE comic_id = ?1 AND state != 'done'
                      ORDER BY img_index ASC",
                )
                .context("准备未完成图片查询失败")?;
            let rows = stmt
                .query_map(params![comic_id], |row| row.get::<_, i64>(0))
                .context("查询未完成图片失败")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.context("读取图片下标失败")?);
            }
            Ok(out)
        })
    }

    /// 列出某本漫画的全部图片行。
    pub fn list(store: &Store, comic_id: i64) -> anyhow::Result<Vec<DbImage>> {
        store.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM download_image WHERE comic_id = ?1 ORDER BY img_index ASC",
                )
                .context("准备图片列表查询失败")?;
            let rows = stmt
                .query_map(params![comic_id], DbImage::from_row)
                .context("查询图片列表失败")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.context("读取图片行失败")?);
            }
            Ok(out)
        })
    }

    /// 数已完成张数。恢复时用来和任务里的计数对账。
    pub fn count_done(store: &Store, comic_id: i64) -> anyhow::Result<i64> {
        store.with_conn(|conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM download_image WHERE comic_id = ?1 AND state = 'done'",
                    params![comic_id],
                    |row| row.get(0),
                )
                .context("统计已完成图片失败")?;
            Ok(count)
        })
    }

    /// 清空一本漫画的图片记录（重下前调用）。
    pub fn clear(store: &Store, comic_id: i64) -> anyhow::Result<()> {
        store.with_conn(|conn| {
            conn.execute(
                "DELETE FROM download_image WHERE comic_id = ?1",
                params![comic_id],
            )
            .context("清空图片记录失败")?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let dir = std::env::temp_dir().join(format!("wnacg-repo-test-{}", now_ts()));
        std::fs::create_dir_all(&dir).unwrap();
        Store::open(&dir.join("test.db")).unwrap()
    }

    fn urls(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("//img.example/{i}.jpg")).collect()
    }

    #[test]
    fn upsert_keeps_progress_on_reinsert() {
        let s = store();
        TaskRepo::upsert_new(&s, 1, "标题", "/downloads/1").unwrap();
        TaskRepo::set_total_img_count(&s, 1, 10).unwrap();
        TaskRepo::set_progress(&s, 1, 4).unwrap();
        TaskRepo::set_state(&s, 1, DbTaskState::Downloading, None).unwrap();

        // 重下同一本：标题/目录可以更新，进度和状态不能被打回原形。
        TaskRepo::upsert_new(&s, 1, "新标题", "/downloads/2").unwrap();

        let task = TaskRepo::get(&s, 1).unwrap().unwrap();
        assert_eq!(task.comic_title, "新标题");
        assert_eq!(task.download_dir, "/downloads/2");
        assert_eq!(task.done_img_count, 4, "进度被重置了");
        assert_eq!(task.state, DbTaskState::Downloading, "状态被重置了");
    }

    #[test]
    fn mark_done_is_idempotent_for_counter() {
        let s = store();
        TaskRepo::upsert_new(&s, 1, "标题", "/d").unwrap();
        TaskRepo::set_total_img_count(&s, 1, 3).unwrap();
        ImageRepo::insert_many(&s, 1, &urls(3)).unwrap();

        ImageRepo::mark_done(&s, 1, 0, Some(100)).unwrap();
        ImageRepo::mark_done(&s, 1, 0, Some(100)).unwrap();

        let task = TaskRepo::get(&s, 1).unwrap().unwrap();
        assert_eq!(task.done_img_count, 1, "重复标记把计数顶高了");
        assert_eq!(ImageRepo::count_done(&s, 1).unwrap(), 1);
    }

    #[test]
    fn insert_many_does_not_reset_done_images() {
        let s = store();
        TaskRepo::upsert_new(&s, 1, "标题", "/d").unwrap();
        ImageRepo::insert_many(&s, 1, &urls(3)).unwrap();
        ImageRepo::mark_done(&s, 1, 1, None).unwrap();

        // 模拟恢复时重新登记：已完成的第 1 张必须保持 done。
        ImageRepo::insert_many(&s, 1, &urls(3)).unwrap();

        let pending = ImageRepo::list_pending_indexes(&s, 1).unwrap();
        assert_eq!(pending, vec![0, 2], "已完成的图片被重置成待下载了");
    }

    #[test]
    fn stats_aggregates_all_states() {
        let s = store();
        TaskRepo::upsert_new(&s, 1, "A", "/d").unwrap();
        TaskRepo::upsert_new(&s, 2, "B", "/d").unwrap();
        TaskRepo::upsert_new(&s, 3, "C", "/d").unwrap();
        TaskRepo::set_state(&s, 1, DbTaskState::Completed, None).unwrap();
        TaskRepo::set_state(&s, 2, DbTaskState::Failed, Some("超时")).unwrap();
        TaskRepo::set_state(&s, 3, DbTaskState::Downloading, None).unwrap();
        TaskRepo::set_total_img_count(&s, 1, 10).unwrap();
        TaskRepo::set_progress(&s, 1, 10).unwrap();

        let st = TaskRepo::stats(&s).unwrap();
        assert_eq!(st.total, 3);
        assert_eq!(st.completed, 1);
        assert_eq!(st.failed, 1);
        assert_eq!(st.downloading, 1);
        assert_eq!(st.pending, 0);
        assert_eq!(st.total_img_count, 10);
        assert_eq!(st.done_img_count, 10);
    }

    #[test]
    fn resumable_excludes_paused_and_cancelled() {
        let s = store();
        for id in 1..=5 {
            TaskRepo::upsert_new(&s, id, "T", "/d").unwrap();
        }
        TaskRepo::set_state(&s, 1, DbTaskState::Pending, None).unwrap();
        TaskRepo::set_state(&s, 2, DbTaskState::Downloading, None).unwrap();
        TaskRepo::set_state(&s, 3, DbTaskState::Failed, None).unwrap();
        TaskRepo::set_state(&s, 4, DbTaskState::Paused, None).unwrap();
        TaskRepo::set_state(&s, 5, DbTaskState::Cancelled, None).unwrap();

        let ids: Vec<i64> = TaskRepo::list_resumable(&s)
            .unwrap()
            .into_iter()
            .map(|t| t.comic_id)
            .collect();
        assert_eq!(ids.len(), 3, "恢复集合应只含 pending/downloading/failed");
        assert!(!ids.contains(&4), "暂停任务被自动恢复了");
        assert!(!ids.contains(&5), "已取消任务被自动恢复了");
    }

    #[test]
    fn delete_cascades_to_images() {
        let s = store();
        TaskRepo::upsert_new(&s, 7, "T", "/d").unwrap();
        ImageRepo::insert_many(&s, 7, &urls(4)).unwrap();

        TaskRepo::delete(&s, 7).unwrap();

        assert!(TaskRepo::get(&s, 7).unwrap().is_none());
        assert!(ImageRepo::list(&s, 7).unwrap().is_empty(), "图片明细未级联删除");
    }

    #[test]
    fn mark_failed_bumps_retry_count() {
        let s = store();
        TaskRepo::upsert_new(&s, 1, "T", "/d").unwrap();
        ImageRepo::insert_many(&s, 1, &urls(1)).unwrap();

        ImageRepo::mark_failed(&s, 1, 0, "连接超时").unwrap();
        ImageRepo::mark_failed(&s, 1, 0, "连接超时").unwrap();

        let img = &ImageRepo::list(&s, 1).unwrap()[0];
        assert_eq!(img.state, DbImageState::Failed);
        assert_eq!(img.retry_count, 2);
        assert_eq!(img.last_error.as_deref(), Some("连接超时"));
    }
}
