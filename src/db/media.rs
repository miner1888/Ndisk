//! transcodes / media_cache 两张表:转码任务与缩略图缓存登记。

use rusqlite::params;

use super::{CacheRow, Db, TranscodeRow};

impl Db {
    // ---------- transcodes ----------

    pub async fn transcode(&self, share_id: i64, rel: &str) -> rusqlite::Result<Option<TranscodeRow>> {
        self.with(|c| {
            let r = c.query_row(
                "SELECT status,out_path,updated_at FROM transcodes WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel],
                |r| Ok(TranscodeRow { status: r.get(0)?, out_path: r.get(1)?, updated_at: r.get(2)? }),
            );
            match r {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
    }

    pub async fn transcode_start(&self, share_id: i64, rel: &str, hash: &str, out: &str, now: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO transcodes(share_id,rel_path,hash,status,out_path,updated_at) VALUES(?1,?2,?3,'running',?4,?5) \
                 ON CONFLICT(share_id,rel_path) DO UPDATE SET hash=?3,status='running',out_path=?4,updated_at=?5",
                params![share_id, rel, hash, out, now],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn transcode_finish(&self, share_id: i64, rel: &str, ok: bool, now: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE transcodes SET status=?3,updated_at=?4 WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel, if ok { "done" } else { "failed" }, now],
            )
            .map(|_| ())
        })
        .await
    }

    /// 播放命中已完成的转码产物:刷新时间戳,sweep 的 30 天按"未访问"计。
    pub async fn transcode_touch(&self, share_id: i64, rel: &str, now: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE transcodes SET updated_at=?3 WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel, now],
            )
            .map(|_| ())
        })
        .await
    }

    /// 缩略图命中:刷新对应登记的时间戳(语义同 transcode_touch)。
    pub async fn media_cache_touch(&self, share_id: i64, rel: &str, kind: &str, now: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE media_cache SET updated_at=?4 WHERE share_id=?1 AND rel_path=?2 AND kind=?3",
                params![share_id, rel, kind, now],
            )
            .map(|_| ())
        })
        .await
    }

    /// 取出并删除转码登记(删除文件时回收产物),返回产物路径。
    pub async fn transcode_take(&self, share_id: i64, rel: &str) -> rusqlite::Result<Option<String>> {
        self.with(|c| {
            let out = c
                .query_row(
                    "SELECT out_path FROM transcodes WHERE share_id=?1 AND rel_path=?2",
                    params![share_id, rel],
                    |r| r.get::<_, String>(0),
                )
                .ok();
            c.execute("DELETE FROM transcodes WHERE share_id=?1 AND rel_path=?2", params![share_id, rel])?;
            Ok(out)
        })
        .await
    }

    pub async fn transcode_list(&self) -> rusqlite::Result<Vec<(i64, String, String, String, i64)>> {
        self.rows(
            "SELECT share_id,rel_path,status,out_path,updated_at FROM transcodes",
            (),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .await
    }

    pub async fn transcode_delete(&self, share_id: i64, rel: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM transcodes WHERE share_id=?1 AND rel_path=?2", params![share_id, rel]).map(|_| ())
        })
        .await
    }

    pub async fn transcode_clear(&self) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM transcodes", []).map(|_| ())).await
    }

    // ---------- media_cache ----------

    pub async fn media_cache_put(&self, share_id: i64, rel: &str, kind: &str, out_path: &str, now: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO media_cache(share_id,rel_path,kind,out_path,updated_at) VALUES(?1,?2,?3,?4,?5) \
                 ON CONFLICT(share_id,rel_path,kind) DO UPDATE SET out_path=?4,updated_at=?5",
                params![share_id, rel, kind, out_path, now],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn media_cache_rows(&self, share_id: i64, rel: &str) -> rusqlite::Result<Vec<CacheRow>> {
        self.rows(
            "SELECT kind,out_path,updated_at FROM media_cache WHERE share_id=?1 AND rel_path=?2",
            (share_id, rel),
            |r| Ok(CacheRow { kind: r.get(0)?, out_path: r.get(1)?, updated_at: r.get(2)? }),
        )
        .await
    }

    /// 删除某文件全部缓存登记,返回需要删除的磁盘路径。
    pub async fn media_cache_drop(&self, share_id: i64, rel: &str) -> rusqlite::Result<Vec<String>> {
        self.with(|c| {
            let mut out = Vec::new();
            {
                let mut stmt = c.prepare("SELECT out_path FROM media_cache WHERE share_id=?1 AND rel_path=?2")?;
                let rows = stmt.query_map(params![share_id, rel], |r| r.get::<_, String>(0))?;
                for r in rows {
                    out.push(r?);
                }
            }
            c.execute("DELETE FROM media_cache WHERE share_id=?1 AND rel_path=?2", params![share_id, rel])?;
            Ok(out)
        })
        .await
    }

    pub async fn media_cache_list(&self) -> rusqlite::Result<Vec<(i64, String, String, String, i64)>> {
        self.rows(
            "SELECT share_id,rel_path,kind,out_path,updated_at FROM media_cache",
            (),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .await
    }

    pub async fn media_cache_delete(&self, share_id: i64, rel: &str, kind: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM media_cache WHERE share_id=?1 AND rel_path=?2 AND kind=?3",
                params![share_id, rel, kind],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn media_cache_clear(&self) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM media_cache", []).map(|_| ())).await
    }
}
