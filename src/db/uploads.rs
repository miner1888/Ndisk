//! uploads 表:分片上传会话。

use rusqlite::{params, Row};

use super::{Db, UploadRow};
use crate::domain::chunkup::UploadPlan;
use crate::domain::now_secs;

fn upload_row(r: &Row) -> rusqlite::Result<UploadRow> {
    Ok(UploadRow {
        id: r.get(0)?,
        token: r.get(1)?,
        user_id: r.get(2)?,
        share_id: r.get(3)?,
        rel_path: r.get(4)?,
        filename: r.get(5)?,
        total_size: r.get(6)?,
        chunk_size: r.get(7)?,
        num_parts: r.get(8)?,
    })
}

const UCOLS: &str = "id,token,user_id,share_id,rel_path,filename,total_size,chunk_size,num_parts";

impl Db {
    pub async fn create_upload(&self, u: &UploadPlan) -> rusqlite::Result<i64> {
        let now = now_secs();
        self.with(|c| {
            c.execute(
                "INSERT INTO uploads(token,user_id,share_id,rel_path,filename,total_size,chunk_size,num_parts,created_at,updated_at) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",
                params![u.token, u.user_id, u.share_id, u.rel_path, u.filename, u.total_size, u.chunk_size, u.num_parts, now],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    pub async fn upload_by_token(&self, token: &str) -> rusqlite::Result<Option<UploadRow>> {
        self.one(&format!("SELECT {UCOLS} FROM uploads WHERE token=?1"), (token,), upload_row).await
    }

    pub async fn uploads_of(&self, user_id: i64) -> rusqlite::Result<Vec<UploadRow>> {
        self.rows(&format!("SELECT {UCOLS} FROM uploads WHERE user_id=?1"), (user_id,), upload_row).await
    }

    pub async fn touch_upload(&self, id: i64) -> rusqlite::Result<()> {
        self.with(|c| c.execute("UPDATE uploads SET updated_at=?2 WHERE id=?1", params![id, now_secs()]).map(|_| ()))
            .await
    }

    pub async fn delete_upload(&self, id: i64) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM uploads WHERE id=?1", (id,)).map(|_| ())).await
    }

    pub async fn prune_uploads(&self, older_than: i64) -> rusqlite::Result<usize> {
        self.with(|c| c.execute("DELETE FROM uploads WHERE updated_at < ?1", (older_than,))).await
    }

    pub async fn live_upload_tokens(&self) -> rusqlite::Result<Vec<String>> {
        self.rows("SELECT token FROM uploads", (), |r| r.get(0)).await
    }
}
