//! users / sessions 两张表。

use rusqlite::params;

use super::{Db, User};
use crate::domain::now_secs;

const UCOL: &str =
    "id,username,password_hash,is_admin,can_upload,can_download,can_delete,can_mkdir,created_at";

impl Db {
    pub async fn user_by_id(&self, id: i64) -> rusqlite::Result<Option<User>> {
        self.one(&format!("SELECT {UCOL} FROM users WHERE id=?1"), (id,), User::from_row).await
    }

    pub async fn user_by_name(&self, name: &str) -> rusqlite::Result<Option<User>> {
        self.one(&format!("SELECT {UCOL} FROM users WHERE username=?1"), (name,), User::from_row).await
    }

    pub async fn all_users(&self) -> rusqlite::Result<Vec<User>> {
        self.rows(&format!("SELECT {UCOL} FROM users ORDER BY id"), (), User::from_row).await
    }

    pub async fn count_admins(&self) -> rusqlite::Result<i64> {
        self.with(|c| c.query_row("SELECT COUNT(*) FROM users WHERE is_admin=1", [], |r| r.get(0))).await
    }

    pub async fn create_user(
        &self,
        name: &str,
        hash: &str,
        is_admin: bool,
        flags: [bool; 4],
    ) -> rusqlite::Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO users(username,password_hash,is_admin,can_upload,can_download,can_delete,can_mkdir,created_at) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![name, hash, is_admin as i64, flags[0] as i64, flags[1] as i64, flags[2] as i64, flags[3] as i64, now_secs()],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    pub async fn set_password(&self, id: i64, hash: &str) -> rusqlite::Result<()> {
        self.with(|c| c.execute("UPDATE users SET password_hash=?2 WHERE id=?1", params![id, hash]).map(|_| ()))
            .await
    }

    pub async fn update_flags(&self, id: i64, is_admin: bool, flags: [bool; 4]) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE users SET is_admin=?2,can_upload=?3,can_download=?4,can_delete=?5,can_mkdir=?6 WHERE id=?1",
                params![id, is_admin as i64, flags[0] as i64, flags[1] as i64, flags[2] as i64, flags[3] as i64],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn delete_user(&self, id: i64) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM users WHERE id=?1", (id,)).map(|_| ())).await
    }

    // ---------- sessions ----------

    pub async fn create_session(&self, user_id: i64, token: &str, ttl: i64) -> rusqlite::Result<()> {
        let now = now_secs();
        self.with(|c| {
            c.execute(
                "INSERT INTO sessions(user_id,token,created_at,expires_at) VALUES(?1,?2,?3,?4)",
                params![user_id, token, now, now + ttl],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn drop_session(&self, token: &str) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM sessions WHERE token=?1", (token,)).map(|_| ())).await
    }

    pub async fn drop_other_sessions(&self, user_id: i64, keep: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM sessions WHERE user_id=?1 AND token<>?2", params![user_id, keep]).map(|_| ())
        })
        .await
    }

    /// 按 token 取用户;顺手清理过期会话。
    pub async fn user_by_token(&self, token: &str) -> rusqlite::Result<Option<User>> {
        let now = now_secs();
        self.with(|c| c.execute("DELETE FROM sessions WHERE expires_at < ?1", (now,)).map(|_| ())).await?;
        self.one(
            "SELECT u.id,u.username,u.password_hash,u.is_admin,u.can_upload,u.can_download,u.can_delete,u.can_mkdir,u.created_at \
             FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token=?1 AND s.expires_at>=?2",
            (token, now_secs()),
            User::from_row,
        )
        .await
    }
}
