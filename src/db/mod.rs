//! SQLite 持久层。
//!
//! `Db` 是唯一的数据库句柄(`Arc<tokio::sync::Mutex<Connection>>`,WAL 模式),
//! 各领域 DAO 拆分到子模块,均为 `impl Db` 扩展。闭包在独占锁下同步执行,
//! 避免 rusqlite 的异步包装开销;全部查询都是毫秒级小事务,不会阻塞运行时。

pub mod links;
pub mod media;
pub mod models;
pub mod offline;
pub mod schema;
pub mod shares;
pub mod uploads;
pub mod users;

use std::path::Path;
use std::sync::Arc;

use rusqlite::{Connection, Row};
use tokio::sync::Mutex;

pub use models::*;

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub async fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        let db = Self { conn: Arc::new(Mutex::new(conn)) };
        db.with(|c| c.execute_batch(schema::SCHEMA)).await.map_err(|e| e.to_string())?;
        Ok(db)
    }

    /// 独占锁下执行同步闭包。
    pub async fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send,
    ) -> rusqlite::Result<T> {
        let conn = self.conn.lock().await;
        f(&conn)
    }

    /// 预编译 + 逐行映射的查询。
    pub async fn rows<T>(
        &self,
        sql: &str,
        p: impl rusqlite::Params + Send,
        map: fn(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Vec<T>> {
        self.with(|c| {
            let mut stmt = c.prepare(sql)?;
            let collected: rusqlite::Result<Vec<T>> = stmt.query_map(p, map)?.collect();
            collected
        })
        .await
    }

    /// 取第一行;无行返回 `None`。
    pub async fn one<T>(
        &self,
        sql: &str,
        p: impl rusqlite::Params + Send,
        map: fn(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Option<T>> {
        Ok(self.rows(sql, p, map).await?.into_iter().next())
    }
}
