use std::path::Path;
use std::sync::Arc;

use rusqlite::{params, Connection, Row};
use tokio::sync::Mutex;

use crate::domain::perms::Level;

/// SQLite 访问层:单连接 + 互斥;所有 SQL 集中在此,业务层不接触 SQL。
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

// ---------------- 模型 ----------------

#[derive(Clone, Debug, serde::Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub is_admin: bool,
    pub can_upload: bool,
    pub can_download: bool,
    pub can_delete: bool,
    pub can_mkdir: bool,
    pub created_at: i64,
}

impl User {
    fn from_row(r: &Row) -> rusqlite::Result<User> {
        Ok(User {
            id: r.get(0)?,
            username: r.get(1)?,
            password_hash: r.get(2)?,
            is_admin: r.get::<_, i64>(3)? != 0,
            can_upload: r.get::<_, i64>(4)? != 0,
            can_download: r.get::<_, i64>(5)? != 0,
            can_delete: r.get::<_, i64>(6)? != 0,
            can_mkdir: r.get::<_, i64>(7)? != 0,
            created_at: r.get(8)?,
        })
    }
    pub fn flags(&self) -> [bool; 4] {
        [
            self.can_upload,
            self.can_download,
            self.can_delete,
            self.can_mkdir,
        ]
    }
}

#[derive(Clone, Debug)]
pub struct Share {
    pub id: i64,
    pub name: String,
    pub host_path: String,
    pub owner_id: Option<i64>,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct AccessRule {
    pub id: i64,
    pub rel_path: String,
    pub level: Level,
}

#[derive(Clone, Debug)]
pub struct UploadRow {
    pub id: i64,
    pub token: String,
    pub user_id: i64,
    pub share_id: i64,
    pub rel_path: String,
    pub filename: String,
    pub total_size: i64,
    pub chunk_size: i64,
    pub num_parts: i64,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub id: i64,
    pub token: String,
    pub share_id: i64,
    pub rel_path: String,
    pub created_by: Option<i64>,
    pub created_at: i64,
    pub hits: i64,
    pub expires_at: i64,
}

impl Link {
    pub fn expired(&self) -> bool {
        self.expires_at > 0 && crate::state::now_secs() >= self.expires_at
    }
}

#[derive(Clone, Debug)]
pub struct TlsCert {
    pub domain: String,
    pub cert_pem: String,
    pub key_pem: String,
}

// ---------------- 实现 ----------------

impl Db {
    pub async fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_schema().await?;
        Ok(db)
    }

    async fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send,
    ) -> rusqlite::Result<T> {
        let conn = self.conn.lock().await;
        f(&conn)
    }

    async fn rows<T>(
        &self,
        sql: &str,
        p: impl rusqlite::Params + Send,
        map: fn(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Vec<T>> {
        self.with(|c| {
            let mut stmt = c.prepare(sql)?;
            let rd = stmt.query_map(p, map)?;
            rd.collect()
        })
        .await
    }

    async fn one<T>(
        &self,
        sql: &str,
        p: impl rusqlite::Params + Send,
        map: fn(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Option<T>> {
        Ok(self.rows(sql, p, map).await?.into_iter().next())
    }

    async fn init_schema(&self) -> Result<(), String> {
        self.with(|c| {
            c.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS users (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    username TEXT NOT NULL UNIQUE,
                    password_hash TEXT NOT NULL,
                    is_admin INTEGER NOT NULL DEFAULT 0,
                    can_upload INTEGER NOT NULL DEFAULT 1,
                    can_download INTEGER NOT NULL DEFAULT 1,
                    can_delete INTEGER NOT NULL DEFAULT 0,
                    can_mkdir INTEGER NOT NULL DEFAULT 1,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS sessions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                    token TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    expires_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS shares (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    host_path TEXT NOT NULL,
                    owner_id INTEGER,
                    kind TEXT NOT NULL DEFAULT 'shared',
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS rules (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    share_id INTEGER NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
                    rel_path TEXT NOT NULL DEFAULT '',
                    level TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    UNIQUE(share_id, rel_path)
                );
                CREATE TABLE IF NOT EXISTS file_meta (
                    share_id INTEGER NOT NULL,
                    rel_path TEXT NOT NULL,
                    owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                    created_at INTEGER NOT NULL,
                    PRIMARY KEY (share_id, rel_path)
                );
                CREATE TABLE IF NOT EXISTS uploads (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    token TEXT NOT NULL,
                    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                    share_id INTEGER NOT NULL,
                    rel_path TEXT NOT NULL,
                    filename TEXT NOT NULL,
                    total_size INTEGER NOT NULL,
                    chunk_size INTEGER NOT NULL,
                    num_parts INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS transcodes (
                    share_id INTEGER NOT NULL,
                    rel_path TEXT NOT NULL,
                    hash TEXT NOT NULL,
                    status TEXT NOT NULL,
                    out_path TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (share_id, rel_path)
                );
                CREATE TABLE IF NOT EXISTS media_cache (
                    share_id INTEGER NOT NULL,
                    rel_path TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    out_path TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (share_id, rel_path, kind)
                );
                CREATE TABLE IF NOT EXISTS links (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    token TEXT NOT NULL UNIQUE,
                    share_id INTEGER NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
                    rel_path TEXT NOT NULL,
                    created_by INTEGER,
                    created_at INTEGER NOT NULL,
                    hits INTEGER NOT NULL DEFAULT 0,
                    expires_at INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(share_id, rel_path)
                );
                CREATE TABLE IF NOT EXISTS tls_certs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    domain TEXT NOT NULL,
                    cert_pem TEXT NOT NULL,
                    key_pem TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                "#,
            )
        })
        .await
        .map_err(|e| e.to_string())
    }

    // ---- users / sessions ----

    const UCOL: &str = "id,username,password_hash,is_admin,can_upload,can_download,can_delete,can_mkdir,created_at";

    pub async fn user_by_id(&self, id: i64) -> rusqlite::Result<Option<User>> {
        self.one(
            &format!("SELECT {} FROM users WHERE id=?1", Self::UCOL),
            (id,),
            User::from_row,
        )
        .await
    }
    pub async fn user_by_name(&self, name: &str) -> rusqlite::Result<Option<User>> {
        self.one(
            &format!("SELECT {} FROM users WHERE username=?1", Self::UCOL),
            (name,),
            User::from_row,
        )
        .await
    }
    pub async fn all_users(&self) -> rusqlite::Result<Vec<User>> {
        self.rows(
            &format!("SELECT {} FROM users ORDER BY id", Self::UCOL),
            (),
            User::from_row,
        )
        .await
    }
    pub async fn count_admins(&self) -> rusqlite::Result<i64> {
        self.with(|c| {
            c.query_row("SELECT COUNT(*) FROM users WHERE is_admin=1", [], |r| {
                r.get(0)
            })
        })
        .await
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
                "INSERT INTO users(username,password_hash,is_admin,can_upload,can_download,can_delete,can_mkdir,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![name, hash, is_admin as i64, flags[0] as i64, flags[1] as i64, flags[2] as i64, flags[3] as i64, crate::state::now_secs()],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }
    pub async fn set_password(&self, id: i64, hash: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE users SET password_hash=?2 WHERE id=?1",
                params![id, hash],
            )
            .map(|_| ())
        })
        .await
    }
    pub async fn update_flags(
        &self,
        id: i64,
        is_admin: bool,
        flags: [bool; 4],
    ) -> rusqlite::Result<()> {
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
        self.with(|c| {
            c.execute("DELETE FROM users WHERE id=?1", (id,))
                .map(|_| ())
        })
        .await
    }

    pub async fn create_session(
        &self,
        user_id: i64,
        token: &str,
        ttl: i64,
    ) -> rusqlite::Result<()> {
        let now = crate::state::now_secs();
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
        self.with(|c| {
            c.execute("DELETE FROM sessions WHERE token=?1", (token,))
                .map(|_| ())
        })
        .await
    }
    pub async fn drop_other_sessions(&self, user_id: i64, keep: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM sessions WHERE user_id=?1 AND token<>?2",
                params![user_id, keep],
            )
            .map(|_| ())
        })
        .await
    }
    pub async fn user_by_token(&self, token: &str) -> rusqlite::Result<Option<User>> {
        let now = crate::state::now_secs();
        self.with(|c| {
            c.execute("DELETE FROM sessions WHERE expires_at < ?1", (now,))
                .map(|_| ())
        })
        .await?;
        self.one(
            "SELECT u.id,u.username,u.password_hash,u.is_admin,u.can_upload,u.can_download,u.can_delete,u.can_mkdir,u.created_at \
             FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token=?1 AND s.expires_at>=?2",
            (token, crate::state::now_secs()),
            User::from_row,
        )
        .await
    }

    // ---- shares / rules ----

    const SCOL: &str = "id,name,host_path,owner_id,kind";

    fn share_row(r: &Row) -> rusqlite::Result<Share> {
        Ok(Share {
            id: r.get(0)?,
            name: r.get(1)?,
            host_path: r.get(2)?,
            owner_id: r.get(3)?,
            kind: r.get(4)?,
        })
    }
    fn rule_row(r: &Row) -> rusqlite::Result<AccessRule> {
        Ok(AccessRule {
            id: r.get(0)?,
            rel_path: r.get(1)?,
            level: Level::parse(&r.get::<_, String>(2)?).unwrap_or(Level::Admin),
        })
    }

    pub async fn create_share(
        &self,
        name: &str,
        host_path: &str,
        owner_id: Option<i64>,
        kind: &str,
    ) -> rusqlite::Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO shares(name,host_path,owner_id,kind,created_at) VALUES(?1,?2,?3,?4,?5)",
                params![name, host_path, owner_id, kind, crate::state::now_secs()],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }
    pub async fn share(&self, id: i64) -> rusqlite::Result<Option<Share>> {
        self.one(
            &format!("SELECT {} FROM shares WHERE id=?1", Self::SCOL),
            (id,),
            Self::share_row,
        )
        .await
    }
    pub async fn all_shares(&self) -> rusqlite::Result<Vec<Share>> {
        self.rows(
            &format!("SELECT {} FROM shares ORDER BY id", Self::SCOL),
            (),
            Self::share_row,
        )
        .await
    }
    pub async fn update_share(&self, id: i64, name: &str, host_path: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE shares SET name=?2,host_path=?3 WHERE id=?1",
                params![id, name, host_path],
            )
            .map(|_| ())
        })
        .await
    }
    pub async fn delete_share(&self, id: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM shares WHERE id=?1", (id,))
                .map(|_| ())
        })
        .await
    }

    pub async fn rules_of(&self, share_id: i64) -> rusqlite::Result<Vec<AccessRule>> {
        self.rows(
            "SELECT id,rel_path,level FROM rules WHERE share_id=?1 ORDER BY length(rel_path) DESC, rel_path",
            (share_id,),
            Self::rule_row,
        )
        .await
    }
    pub async fn upsert_rule(
        &self,
        share_id: i64,
        rel_path: &str,
        level: Level,
    ) -> rusqlite::Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO rules(share_id,rel_path,level,created_at) VALUES(?1,?2,?3,?4) \
                 ON CONFLICT(share_id,rel_path) DO UPDATE SET level=?3",
                params![share_id, rel_path, level.as_str(), crate::state::now_secs()],
            )?;
            c.query_row(
                "SELECT id FROM rules WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel_path],
                |r| r.get(0),
            )
        })
        .await
    }
    /// 删除指定共享下的规则;返回是否真的删除了行(防跨共享误删)。
    pub async fn delete_rule(&self, share_id: i64, rule_id: i64) -> rusqlite::Result<bool> {
        self.with(|c| {
            let n = c.execute(
                "DELETE FROM rules WHERE id=?1 AND share_id=?2",
                params![rule_id, share_id],
            )?;
            Ok(n > 0)
        })
        .await
    }

    // ---- file_meta(共享目录内的条目归属;个人空间不依赖此表) ----

    /// 记录条目创建者;已存在则保留原创建者(覆盖写入不改变归属)。
    pub async fn set_meta_owner(
        &self,
        share_id: i64,
        rel_path: &str,
        owner_id: i64,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO file_meta(share_id,rel_path,owner_id,created_at) VALUES(?1,?2,?3,?4) \
                 ON CONFLICT(share_id,rel_path) DO NOTHING",
                params![share_id, rel_path, owner_id, crate::state::now_secs()],
            )?;
            Ok(())
        })
        .await
    }

    /// 条目及其子树的 (相对路径, 创建者) 列表;无记录表示非本盘创建(外部放入)。
    pub async fn meta_owners_under(
        &self,
        share_id: i64,
        rel_path: &str,
    ) -> rusqlite::Result<Vec<(String, i64)>> {
        let prefix = format!("{rel_path}/");
        self.rows(
            "SELECT rel_path,owner_id FROM file_meta WHERE share_id=?1 AND (rel_path=?2 OR substr(rel_path,1,?3)=?4)",
            (share_id, rel_path, prefix.len() as i64, prefix.as_str()),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
    }

    /// 删除条目及其子树的归属记录(跟随删除/覆盖)。
    pub async fn drop_meta(&self, share_id: i64, rel_path: &str) -> rusqlite::Result<()> {
        let prefix = format!("{rel_path}/");
        self.with(|c| {
            c.execute(
                "DELETE FROM file_meta WHERE share_id=?1 AND (rel_path=?2 OR substr(rel_path,1,?3)=?4)",
                params![share_id, rel_path, prefix.len() as i64, prefix],
            )?;
            Ok(())
        })
        .await
    }

    /// 重命名/移动:from 及其子树的归属记录整体改写到 to 下。
    pub async fn rename_meta(&self, share_id: i64, from: &str, to: &str) -> rusqlite::Result<()> {
        let from_slash = format!("{from}/");
        let plen = from_slash.len() as i64;
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            // 改名目标若残留记录(如覆盖同名)先清理,避免旧归属串到新名字上
            tx.execute("DELETE FROM file_meta WHERE share_id=?1 AND rel_path=?2", params![share_id, to])?;
            // 子树:把 "from/xxx" 改写为 "to/xxx"
            tx.execute(
                "UPDATE file_meta SET rel_path=?3 || substr(rel_path, ?4) WHERE share_id=?1 AND substr(rel_path,1,?4)=?2",
                params![share_id, from_slash, to, plen],
            )?;
            // 条目自身
            tx.execute(
                "UPDATE file_meta SET rel_path=?3 WHERE share_id=?1 AND rel_path=?2",
                params![share_id, from, to],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    // ---- uploads ----

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

    pub async fn create_upload(&self, u: &crate::domain::uplink::Upload) -> rusqlite::Result<i64> {
        let now = crate::state::now_secs();
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
        self.one(
            "SELECT id,token,user_id,share_id,rel_path,filename,total_size,chunk_size,num_parts FROM uploads WHERE token=?1",
            (token,),
            Self::upload_row,
        )
        .await
    }
    pub async fn uploads_of(&self, user_id: i64) -> rusqlite::Result<Vec<UploadRow>> {
        self.rows(
            "SELECT id,token,user_id,share_id,rel_path,filename,total_size,chunk_size,num_parts FROM uploads WHERE user_id=?1",
            (user_id,),
            Self::upload_row,
        )
        .await
    }
    pub async fn touch_upload(&self, id: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE uploads SET updated_at=?2 WHERE id=?1",
                params![id, crate::state::now_secs()],
            )
            .map(|_| ())
        })
        .await
    }
    pub async fn delete_upload(&self, id: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM uploads WHERE id=?1", (id,))
                .map(|_| ())
        })
        .await
    }
    /// 清理过期上传记录(启动时调用);返回清理条数。
    pub async fn prune_uploads(&self, older_than: i64) -> rusqlite::Result<usize> {
        self.with(|c| c.execute("DELETE FROM uploads WHERE updated_at < ?1", (older_than,)))
            .await
    }

    /// 清理已过期的分享链接行(expires_at=0 表示永久,保留)。
    pub async fn prune_expired_links(&self, now: i64) -> rusqlite::Result<usize> {
        self.with(|c| {
            c.execute(
                "DELETE FROM links WHERE expires_at > 0 AND expires_at < ?1",
                (now,),
            )
        })
        .await
    }
    pub async fn live_upload_tokens(&self) -> rusqlite::Result<Vec<String>> {
        self.rows("SELECT token FROM uploads", (), |r| r.get(0))
            .await
    }

    // ---- settings(运行期开关,键值存储) ----

    pub async fn setting(&self, key: &str) -> Option<String> {
        let key = key.to_string();
        self.rows("SELECT value FROM settings WHERE key = ?1", (&key,), |r| {
            r.get::<_, String>(0)
        })
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        let key = key.to_string();
        let value = value.to_string();
        self.with(|c| {
            c.execute(
                "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
                (&key, &value),
            )
            .map(|_| ())
        })
        .await
    }

    // ---- transcodes ----

    pub async fn transcode(
        &self,
        share_id: i64,
        rel: &str,
    ) -> rusqlite::Result<Option<(String, String, i64)>> {
        self.with(|c| {
            let r = c.query_row(
                "SELECT status,out_path,updated_at FROM transcodes WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)),
            );
            match r {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
    }
    pub async fn transcode_start(
        &self,
        share_id: i64,
        rel: &str,
        hash: &str,
        out: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
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
    pub async fn transcode_finish(
        &self,
        share_id: i64,
        rel: &str,
        ok: bool,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE transcodes SET status=?3,updated_at=?4 WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel, if ok { "done" } else { "failed" }, now],
            )
            .map(|_| ())
        })
        .await
    }
    pub async fn transcode_take(
        &self,
        share_id: i64,
        rel: &str,
    ) -> rusqlite::Result<Option<String>> {
        self.with(|c| {
            let out = c
                .query_row(
                    "SELECT out_path FROM transcodes WHERE share_id=?1 AND rel_path=?2",
                    params![share_id, rel],
                    |r| r.get::<_, String>(0),
                )
                .ok();
            c.execute(
                "DELETE FROM transcodes WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel],
            )?;
            Ok(out)
        })
        .await
    }

    /// 列出全部转码记录:(share_id, rel_path, status, out_path, updated_at)
    pub async fn transcode_list(
        &self,
    ) -> rusqlite::Result<Vec<(i64, String, String, String, i64)>> {
        self.rows(
            "SELECT share_id,rel_path,status,out_path,updated_at FROM transcodes",
            (),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .await
    }
    pub async fn transcode_delete(&self, share_id: i64, rel: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM transcodes WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel],
            )
            .map(|_| ())
        })
        .await
    }

    // ---- media_cache(缩略图等派生文件索引;便于按源文件是否存在做清理) ----

    pub async fn media_cache_put(
        &self,
        share_id: i64,
        rel: &str,
        kind: &str,
        out_path: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
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
    /// 删除某条目的派生记录,返回其落盘路径(供调用方删文件)
    pub async fn media_cache_drop(
        &self,
        share_id: i64,
        rel: &str,
    ) -> rusqlite::Result<Vec<String>> {
        self.with(|c| {
            let mut out = Vec::new();
            {
                let mut stmt = c.prepare(
                    "SELECT out_path FROM media_cache WHERE share_id=?1 AND rel_path=?2",
                )?;
                let rows = stmt.query_map(params![share_id, rel], |r| r.get::<_, String>(0))?;
                for r in rows {
                    out.push(r?);
                }
            }
            c.execute(
                "DELETE FROM media_cache WHERE share_id=?1 AND rel_path=?2",
                params![share_id, rel],
            )?;
            Ok(out)
        })
        .await
    }
    /// 全部派生记录:(share_id, rel_path, kind, out_path, updated_at)
    pub async fn media_cache_list(
        &self,
    ) -> rusqlite::Result<Vec<(i64, String, String, String, i64)>> {
        self.rows(
            "SELECT share_id,rel_path,kind,out_path,updated_at FROM media_cache",
            (),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .await
    }
    pub async fn media_cache_delete(
        &self,
        share_id: i64,
        rel: &str,
        kind: &str,
    ) -> rusqlite::Result<()> {
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
        self.with(|c| c.execute("DELETE FROM media_cache", []).map(|_| ()))
            .await
    }
    pub async fn transcode_clear(&self) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM transcodes", []).map(|_| ()))
            .await
    }

    // ---- share links ----

    fn link_row(r: &Row) -> rusqlite::Result<Link> {
        Ok(Link {
            id: r.get(0)?,
            token: r.get(1)?,
            share_id: r.get(2)?,
            rel_path: r.get(3)?,
            created_by: r.get(4)?,
            created_at: r.get(5)?,
            hits: r.get(6)?,
            expires_at: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
        })
    }

    pub async fn link_by_token(&self, token: &str) -> rusqlite::Result<Option<Link>> {
        self.one(
            "SELECT id,token,share_id,rel_path,created_by,created_at,hits,expires_at FROM links WHERE token=?1",
            (token,),
            Self::link_row,
        )
        .await
    }
    pub async fn links_of(&self, share_id: i64) -> rusqlite::Result<Vec<Link>> {
        self.rows(
            "SELECT id,token,share_id,rel_path,created_by,created_at,hits,expires_at FROM links WHERE share_id=?1 ORDER BY id",
            (share_id,),
            Self::link_row,
        )
        .await
    }
    /// 确保存在一条链接:已有则刷新有效期返回,否则新建(最多重试 6 次撞唯一键)。
    pub async fn ensure_link(
        &self,
        share_id: i64,
        rel: &str,
        created_by: Option<i64>,
        ttl: i64,
    ) -> Result<Link, String> {
        let rel = crate::domain::paths::normalize(rel)?;
        let expires = if ttl > 0 {
            crate::state::now_secs() + ttl
        } else {
            0
        };
        if let Some(mut existing) = self
            .one::<Link>(
                "SELECT id,token,share_id,rel_path,created_by,created_at,hits,expires_at FROM links WHERE share_id=?1 AND rel_path=?2",
                (share_id, &rel),
                Self::link_row,
            )
            .await
            .ok()
            .flatten()
        {
            self.with(|c| c.execute("UPDATE links SET expires_at=?2 WHERE id=?1", params![existing.id, expires]).map(|_| ()))
                .await
                .map_err(|e| e.to_string())?;
            existing.expires_at = expires;
            return Ok(existing);
        }
        for _ in 0..6 {
            let token = crate::domain::auth::new_token();
            let r = self
                .with(|c| {
                    c.execute(
                        "INSERT OR IGNORE INTO links(token,share_id,rel_path,created_by,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6)",
                        params![token, share_id, rel, created_by, crate::state::now_secs(), expires],
                    )
                })
                .await
                .map_err(|e| e.to_string())?;
            if r == 1 {
                if let Some(l) = self.link_by_token(&token).await.ok().flatten() {
                    return Ok(l);
                }
            }
        }
        Err("创建分享链接失败".into())
    }
    pub async fn delete_link(&self, token: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM links WHERE token=?1", (token,))
                .map(|_| ())
        })
        .await
    }
    pub async fn bump_link(&self, token: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("UPDATE links SET hits=hits+1 WHERE token=?1", (token,))
                .map(|_| ())
        })
        .await
    }

    // ---- tls ----

    pub async fn tls_cert(&self) -> rusqlite::Result<Option<TlsCert>> {
        self.with(|c| {
            match c.query_row(
                "SELECT domain,cert_pem,key_pem FROM tls_certs ORDER BY id LIMIT 1",
                [],
                |r| {
                    Ok(TlsCert {
                        domain: r.get(0)?,
                        cert_pem: r.get(1)?,
                        key_pem: r.get(2)?,
                    })
                },
            ) {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
    }
    pub async fn tls_upsert(
        &self,
        domain: &str,
        cert_pem: &str,
        key_pem: &str,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            let now = crate::state::now_secs();
            let id: Option<i64> = c.query_row("SELECT id FROM tls_certs ORDER BY id LIMIT 1", [], |r| r.get(0)).ok();
            if let Some(id) = id {
                c.execute("UPDATE tls_certs SET domain=?2,cert_pem=?3,key_pem=?4,updated_at=?5 WHERE id=?1", params![id, domain, cert_pem, key_pem, now])?;
            } else {
                c.execute("INSERT INTO tls_certs(domain,cert_pem,key_pem,updated_at) VALUES(?1,?2,?3,?4)", params![domain, cert_pem, key_pem, now])?;
            }
            Ok(())
        })
        .await
    }
    pub async fn tls_delete(&self) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM tls_certs", []).map(|_| ()))
            .await
    }
}
