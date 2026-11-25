//! shares / rules / file_meta 三张表(共享空间、访问规则、条目归属)。

use rusqlite::{params, Row};

use super::{AccessRule, Db, Share};
use crate::domain::access::Level;
use crate::domain::now_secs;

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

impl Db {
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
                params![name, host_path, owner_id, kind, now_secs()],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    pub async fn share(&self, id: i64) -> rusqlite::Result<Option<Share>> {
        self.one(&format!("SELECT {SCOL} FROM shares WHERE id=?1"), (id,), share_row).await
    }

    pub async fn all_shares(&self) -> rusqlite::Result<Vec<Share>> {
        self.rows(&format!("SELECT {SCOL} FROM shares ORDER BY id"), (), share_row).await
    }

    pub async fn update_share(&self, id: i64, name: &str, host_path: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("UPDATE shares SET name=?2,host_path=?3 WHERE id=?1", params![id, name, host_path]).map(|_| ())
        })
        .await
    }

    pub async fn delete_share(&self, id: i64) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM shares WHERE id=?1", (id,)).map(|_| ())).await
    }

    /// 规则按"路径长度降序、再按字典序"返回,最具体规则在前。
    pub async fn rules_of(&self, share_id: i64) -> rusqlite::Result<Vec<AccessRule>> {
        self.rows(
            "SELECT id,rel_path,level FROM rules WHERE share_id=?1 ORDER BY length(rel_path) DESC, rel_path",
            (share_id,),
            rule_row,
        )
        .await
    }

    pub async fn upsert_rule(&self, share_id: i64, rel_path: &str, level: Level) -> rusqlite::Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO rules(share_id,rel_path,level,created_at) VALUES(?1,?2,?3,?4) \
                 ON CONFLICT(share_id,rel_path) DO UPDATE SET level=?3",
                params![share_id, rel_path, level.as_str(), now_secs()],
            )?;
            c.query_row("SELECT id FROM rules WHERE share_id=?1 AND rel_path=?2", params![share_id, rel_path], |r| r.get(0))
        })
        .await
    }

    /// 删除一条规则;返回是否存在过。
    pub async fn delete_rule(&self, share_id: i64, rule_id: i64) -> rusqlite::Result<bool> {
        self.with(|c| {
            let n = c.execute("DELETE FROM rules WHERE id=?1 AND share_id=?2", params![rule_id, share_id])?;
            Ok(n > 0)
        })
        .await
    }

    // ---------- file_meta:共享目录内条目的归属 ----------

    pub async fn set_meta_owner(&self, share_id: i64, rel_path: &str, owner_id: i64) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO file_meta(share_id,rel_path,owner_id,created_at) VALUES(?1,?2,?3,?4) \
                 ON CONFLICT(share_id,rel_path) DO NOTHING",
                params![share_id, rel_path, owner_id, now_secs()],
            )?;
            Ok(())
        })
        .await
    }

    /// 列出 `rel` 自身与其子树内已登记的 `(rel_path, owner_id)`。
    pub async fn meta_owners_under(&self, share_id: i64, rel: &str) -> rusqlite::Result<Vec<(String, i64)>> {
        let prefix = format!("{rel}/");
        self.rows(
            "SELECT rel_path,owner_id FROM file_meta WHERE share_id=?1 AND (rel_path=?2 OR substr(rel_path,1,?3)=?4)",
            (share_id, rel, prefix.len() as i64, prefix.as_str()),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .await
    }

    pub async fn drop_meta(&self, share_id: i64, rel: &str) -> rusqlite::Result<()> {
        let prefix = format!("{rel}/");
        self.with(|c| {
            c.execute(
                "DELETE FROM file_meta WHERE share_id=?1 AND (rel_path=?2 OR substr(rel_path,1,?3)=?4)",
                params![share_id, rel, prefix.len() as i64, prefix],
            )?;
            Ok(())
        })
        .await
    }

    /// 重命名 `from` 子树内全部归属记录到 `to`(含自身)。
    pub async fn rename_meta(&self, share_id: i64, from: &str, to: &str) -> rusqlite::Result<()> {
        let from_slash = format!("{from}/");
        let plen = from_slash.len() as i64;
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("DELETE FROM file_meta WHERE share_id=?1 AND rel_path=?2", params![share_id, to])?;
            tx.execute(
                "UPDATE file_meta SET rel_path=?3 || substr(rel_path, ?4) WHERE share_id=?1 AND substr(rel_path,1,?4)=?2",
                params![share_id, from_slash, to, plen],
            )?;
            tx.execute("UPDATE file_meta SET rel_path=?3 WHERE share_id=?1 AND rel_path=?2", params![share_id, from, to])?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
