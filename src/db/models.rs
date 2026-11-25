//! 领域对象(与数据库表一一对应)。

use crate::domain::access::Level;

/// 系统用户。`password_hash` 不参与 JSON 序列化。
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
    pub fn from_row(r: &rusqlite::Row) -> rusqlite::Result<User> {
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

    /// 能力开关,顺序:[上传, 下载, 删除, 建目录]。
    pub fn flags(&self) -> [bool; 4] {
        [self.can_upload, self.can_download, self.can_delete, self.can_mkdir]
    }
}

/// 共享空间:`kind = home` 是个人空间(私有),`shared` 是管理员挂载的公共目录。
#[derive(Clone, Debug)]
pub struct Share {
    pub id: i64,
    pub name: String,
    pub host_path: String,
    pub owner_id: Option<i64>,
    pub kind: String,
}

/// 目录访问规则(guest/login/admin),路径越具体优先级越高。
#[derive(Clone, Debug)]
pub struct AccessRule {
    pub id: i64,
    pub rel_path: String,
    pub level: Level,
}

/// 分片上传会话。
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

/// 匿名分享链接(/s/{token})。
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
    /// `expires_at == 0` 表示永不过期。
    pub fn expired(&self, now: i64) -> bool {
        self.expires_at > 0 && now >= self.expires_at
    }
}

/// HTTPS 证书(PEM 原文)。
#[derive(Clone, Debug)]
pub struct TlsCert {
    pub domain: String,
    pub cert_pem: String,
    pub key_pem: String,
}

/// 转码任务行。
#[derive(Clone, Debug)]
pub struct TranscodeRow {
    pub status: String,
    pub out_path: String,
    pub updated_at: i64,
}

/// 媒体缓存登记行。
#[derive(Clone, Debug)]
pub struct CacheRow {
    pub kind: String,
    pub out_path: String,
    pub updated_at: i64,
}
