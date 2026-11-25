//! links 表:匿名分享链接(/s/{token})。

use rusqlite::{params, Row};

use super::{Db, Link};
use crate::domain::{auth, now_secs, pathguard};

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

const LCOLS: &str = "id,token,share_id,rel_path,created_by,created_at,hits,expires_at";

impl Db {
    pub async fn link_by_token(&self, token: &str) -> rusqlite::Result<Option<Link>> {
        self.one(&format!("SELECT {LCOLS} FROM links WHERE token=?1"), (token,), link_row).await
    }

    pub async fn links_of(&self, share_id: i64) -> rusqlite::Result<Vec<Link>> {
        self.rows(&format!("SELECT {LCOLS} FROM links WHERE share_id=?1 ORDER BY id"), (share_id,), link_row).await
    }

    /// 为 `(share_id, rel)` 创建或复用一条分享链接。`ttl <= 0` 表示永不过期;
    /// 已存在的链接只更新过期时间,token 保持不变(前端"再次分享"得到同一链接)。
    pub async fn ensure_link(
        &self,
        share_id: i64,
        rel: &str,
        created_by: Option<i64>,
        ttl: i64,
    ) -> Result<Link, String> {
        let rel = pathguard::normalize(rel)?;
        let expires = if ttl > 0 { now_secs() + ttl } else { 0 };
        if let Some(mut existing) = self
            .one::<Link>(
                &format!("SELECT {LCOLS} FROM links WHERE share_id=?1 AND rel_path=?2"),
                (share_id, &rel),
                link_row,
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
        // token 随机生成,极小概率撞 UNIQUE;重试几次。
        for _ in 0..6 {
            let token = auth::new_token();
            let r = self
                .with(|c| {
                    c.execute(
                        "INSERT OR IGNORE INTO links(token,share_id,rel_path,created_by,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6)",
                        params![token, share_id, rel, created_by, now_secs(), expires],
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
        self.with(|c| c.execute("DELETE FROM links WHERE token=?1", (token,)).map(|_| ())).await
    }

    pub async fn bump_link(&self, token: &str) -> rusqlite::Result<()> {
        self.with(|c| c.execute("UPDATE links SET hits=hits+1 WHERE token=?1", (token,)).map(|_| ())).await
    }

    pub async fn prune_expired_links(&self, now: i64) -> rusqlite::Result<usize> {
        self.with(|c| c.execute("DELETE FROM links WHERE expires_at > 0 AND expires_at < ?1", (now,))).await
    }
}
