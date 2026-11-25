//! offline_tasks / settings / tls_certs 三张表。
//!
//! `offline_tasks` 对外 JSON 采用 camelCase(`userId/relDir/...`),这是前端
//! 依赖的历史字段命名,保持不变。

use rusqlite::params;
use serde_json::json;

use super::{Db, TlsCert};
use crate::domain::now_secs;

impl Db {
    // ---------- offline_tasks ----------

    pub async fn insert_offline_task(
        &self,
        user_id: i64,
        share_id: i64,
        rel_dir: &str,
        url: &str,
        kind: &str,
    ) -> rusqlite::Result<i64> {
        let now = now_secs();
        self.with(|c| {
            c.execute(
                "INSERT INTO offline_tasks(user_id,share_id,rel_dir,url,kind,state,created_at,updated_at) \
                 VALUES(?1,?2,?3,?4,?5,'parsing',?6,?6)",
                params![user_id, share_id, rel_dir, url, kind, now],
            )
            .map(|_| c.last_insert_rowid())
        })
        .await
    }

    /// 单条任务(camelCase JSON)。`files` 存库时是字符串,读出时解析为数组。
    pub async fn offline_task(&self, id: i64) -> rusqlite::Result<Option<serde_json::Value>> {
        self.rows(
            "SELECT id,user_id,share_id,rel_dir,url,kind,state,name,error,size,downloaded,files,created_at,updated_at \
             FROM offline_tasks WHERE id=?1",
            (id,),
            |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "userId": r.get::<_, i64>(1)?,
                    "shareId": r.get::<_, i64>(2)?,
                    "relDir": r.get::<_, String>(3)?,
                    "url": r.get::<_, String>(4)?,
                    "kind": r.get::<_, String>(5)?,
                    "state": r.get::<_, String>(6)?,
                    "name": r.get::<_, String>(7)?,
                    "error": r.get::<_, String>(8)?,
                    "size": r.get::<_, i64>(9)?,
                    "downloaded": r.get::<_, i64>(10)?,
                    "files": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(11)?).unwrap_or(json!([])),
                    "createdAt": r.get::<_, i64>(12)?,
                    "updatedAt": r.get::<_, i64>(13)?,
                }))
            },
        )
        .await
        .map(|v| v.into_iter().next())
    }

    pub async fn offline_tasks_of(&self, user_id: i64) -> rusqlite::Result<Vec<serde_json::Value>> {
        self.rows(
            "SELECT id,share_id,rel_dir,url,kind,state,name,error,size,downloaded,files,created_at,updated_at \
             FROM offline_tasks WHERE user_id=?1 ORDER BY id DESC LIMIT 100",
            (user_id,),
            |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "shareId": r.get::<_, i64>(1)?,
                    "relDir": r.get::<_, String>(2)?,
                    "url": r.get::<_, String>(3)?,
                    "kind": r.get::<_, String>(4)?,
                    "state": r.get::<_, String>(5)?,
                    "name": r.get::<_, String>(6)?,
                    "error": r.get::<_, String>(7)?,
                    "size": r.get::<_, i64>(8)?,
                    "downloaded": r.get::<_, i64>(9)?,
                    "files": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(10)?).unwrap_or(json!([])),
                    "createdAt": r.get::<_, i64>(11)?,
                    "updatedAt": r.get::<_, i64>(12)?,
                }))
            },
        )
        .await
    }

    /// 局部更新任务字段。`state='cancelled'` 是终态,不会被其它非 cancelled 状态覆盖;
    /// 空字符串表示"不更新该字段"。
    pub async fn update_offline_task(&self, id: i64, patch: serde_json::Value) -> rusqlite::Result<()> {
        let now = now_secs();
        let state = patch.get("state").and_then(|v| v.as_str()).unwrap_or("");
        let name = patch.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let error = patch.get("error").and_then(|v| v.as_str()).unwrap_or("");
        let size = patch.get("size").and_then(|v| v.as_i64());
        let downloaded = patch.get("downloaded").and_then(|v| v.as_i64());
        let files = match patch.get("files") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(v @ serde_json::Value::Array(_)) => Some(serde_json::to_string(v).unwrap_or_default()),
            _ => None,
        };
        self.with(|c| {
            c.execute(
                "UPDATE offline_tasks SET \
                 state=CASE WHEN state='cancelled' AND ?2 NOT IN ('','cancelled') THEN state WHEN ?2<>'' THEN ?2 ELSE state END, \
                 name=CASE WHEN ?3<>'' THEN ?3 ELSE name END, \
                 error=?4, \
                 size=COALESCE(?5,size), downloaded=COALESCE(?6,downloaded), \
                 files=COALESCE(?7,files), updated_at=?8 WHERE id=?1",
                params![id, state, name, error, size, downloaded, files, now],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn delete_offline_task(&self, id: i64) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM offline_tasks WHERE id=?1", (id,)).map(|_| ())).await
    }

    pub async fn prune_old_offline_tasks(&self, older_than: i64) -> rusqlite::Result<usize> {
        self.with(|c| {
            c.execute(
                "DELETE FROM offline_tasks WHERE state IN ('completed','failed','cancelled') AND updated_at < ?1",
                (older_than,),
            )
        })
        .await
    }

    /// 启动时调用:上次进程退出后仍处于中间态的任务(parsing / downloading /
    /// awaiting_select)已不可能继续推进(worker 随进程消亡,恢复选择也没有
    /// 会话可应用),统一置为 failed,避免前端永远显示"下载中"。
    pub async fn fail_running_offline_tasks(&self) -> rusqlite::Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE offline_tasks SET state='failed', error='服务重启,任务中断', updated_at=?1 \
                 WHERE state IN ('parsing','downloading','awaiting_select')",
                (now_secs(),),
            )
        })
        .await
    }

    // ---------- settings ----------

    pub async fn setting(&self, key: &str) -> Option<String> {
        self.rows("SELECT value FROM settings WHERE key = ?1", (key,), |r| r.get::<_, String>(0))
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
                (key, value),
            )
            .map(|_| ())
        })
        .await
    }

    // ---------- tls_certs ----------

    pub async fn tls_cert(&self) -> rusqlite::Result<Option<TlsCert>> {
        self.with(|c| {
            match c.query_row(
                "SELECT domain,cert_pem,key_pem FROM tls_certs ORDER BY id LIMIT 1",
                [],
                |r| Ok(TlsCert { domain: r.get(0)?, cert_pem: r.get(1)?, key_pem: r.get(2)? }),
            ) {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
    }

    pub async fn tls_upsert(&self, domain: &str, cert_pem: &str, key_pem: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            let now = now_secs();
            let id: Option<i64> = c.query_row("SELECT id FROM tls_certs ORDER BY id LIMIT 1", [], |r| r.get(0)).ok();
            if let Some(id) = id {
                c.execute(
                    "UPDATE tls_certs SET domain=?2,cert_pem=?3,key_pem=?4,updated_at=?5 WHERE id=?1",
                    params![id, domain, cert_pem, key_pem, now],
                )?;
            } else {
                c.execute(
                    "INSERT INTO tls_certs(domain,cert_pem,key_pem,updated_at) VALUES(?1,?2,?3,?4)",
                    params![domain, cert_pem, key_pem, now],
                )?;
            }
            Ok(())
        })
        .await
    }

    pub async fn tls_delete(&self) -> rusqlite::Result<()> {
        self.with(|c| c.execute("DELETE FROM tls_certs", []).map(|_| ())).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约:重启清理只翻转中间态(parsing/downloading/awaiting_select)为
    /// failed;终态(completed/failed/cancelled)保持原样不动。
    #[tokio::test]
    async fn fail_running_marks_only_intermediate_states() {
        let path = std::env::temp_dir().join(format!("ndisk-dbtest-{}-restart.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path).await.unwrap();
        let mut ids = Vec::new();
        for state in ["parsing", "downloading", "awaiting_select", "completed", "failed", "cancelled"] {
            let id = db.insert_offline_task(1, 1, "下载", "u", "http").await.unwrap();
            if state != "parsing" {
                db.update_offline_task(id, serde_json::json!({ "state": state })).await.unwrap();
            }
            ids.push((id, state));
        }
        db.fail_running_offline_tasks().await.unwrap();
        for (id, original) in ids {
            let row = db.offline_task(id).await.unwrap().unwrap();
            let now = row.get("state").and_then(|v| v.as_str()).unwrap_or_default();
            let err = row.get("error").and_then(|v| v.as_str()).unwrap_or_default();
            if matches!(original, "parsing" | "downloading" | "awaiting_select") {
                assert_eq!(now, "failed", "原始 {original} 应翻转为 failed");
                assert!(err.contains("服务重启"), "错误信息应说明重启中断");
            } else {
                assert_eq!(now, original, "终态 {original} 不应被改动");
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}
