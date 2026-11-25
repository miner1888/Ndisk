//! 建表语句。
//!
//! ⚠ 兼容性不变量:表名、列名、类型、默认值必须与历史版本逐字一致,
//! 这样旧版本创建的数据库文件可以直接被新程序打开,不做迁移。

pub const SCHEMA: &str = r#"
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
CREATE TABLE IF NOT EXISTS offline_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    share_id INTEGER NOT NULL,
    rel_dir TEXT NOT NULL DEFAULT '',
    url TEXT NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    error TEXT NOT NULL DEFAULT '',
    size INTEGER NOT NULL DEFAULT 0,
    downloaded INTEGER NOT NULL DEFAULT 0,
    files TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
"#;
