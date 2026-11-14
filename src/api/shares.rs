use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::domain::auth::Identity;
use crate::domain::paths;
use crate::domain::perms::{self, Level};
use crate::error::{ok, ApiError, ApiResult};
use crate::state::AppState;

fn share_visible(
    identity: &Identity,
    share: &crate::db::Share,
    rules: &[crate::db::AccessRule],
) -> bool {
    if perms::is_private(share) {
        return identity
            .user
            .as_ref()
            .map(|u| perms::is_owner(share, u))
            .unwrap_or(false);
    }
    match perms::level_of(rules, "") {
        Level::Guest => true,
        Level::Login => identity.user.is_some(),
        Level::Admin => identity.is_admin(),
    }
}

pub async fn list(State(st): State<AppState>, identity: Identity) -> ApiResult<Response> {
    let mut out = Vec::new();
    let users = st.db.all_users().await.unwrap_or_default();
    let name_of = |id: i64| {
        users
            .iter()
            .find(|u| u.id == id)
            .map(|u| u.username.clone())
            .unwrap_or_default()
    };
    for s in st.db.all_shares().await? {
        let rules = st.db.rules_of(s.id).await?;
        if !share_visible(&identity, &s, &rules) {
            continue;
        }
        out.push(json!({
            "id": s.id,
            "name": s.name,
            "kind": s.kind,
            "owner_id": s.owner_id,
            // 个人空间的归属账号(管理员据此进入各用户空间管理)
            "owner_name": s.owner_id.map(name_of).unwrap_or_default(),
            // 非管理员不暴露服务器物理路径
            "host_path": if identity.is_admin() { s.host_path.clone() } else { String::new() },
            "owner": identity.user.as_ref().map(|u| perms::is_owner(&s, u)).unwrap_or(false),
            "mine": s.owner_id.is_some() && s.owner_id == identity.user.as_ref().map(|u| u.id),
            "access": if perms::is_private(&s) { "private".to_string() } else { perms::level_of(&rules, "").as_str().to_string() },
            "home": s.kind == "home",
        }));
    }
    Ok(ok(json!({ "shares": out, "direct_max": st.chunk_size })))
}

fn path_ok(dir: &PathBuf) -> Result<String, ApiError> {
    if dir.is_file() {
        return Err(ApiError::bad_request("路径是文件而非目录"));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| ApiError::bad_request(format!("无法创建目录: {e}")))?;
    Ok(paths::pretty(
        &dir.canonicalize().unwrap_or_else(|_| dir.clone()),
    ))
}

#[derive(Deserialize)]
pub struct NewShareReq {
    name: String,
    host_path: String,
}

pub async fn create(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<NewShareReq>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    let host = path_ok(&PathBuf::from(&req.host_path))?;
    let id = st.db.create_share(&req.name, &host, None, "shared").await?;
    st.db.upsert_rule(id, "", Level::Login).await?;
    Ok(ok(json!({ "id": id, "ok": true })))
}

#[derive(Deserialize)]
pub struct PatchShareReq {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    host_path: Option<String>,
}

pub async fn patch(
    State(st): State<AppState>,
    identity: Identity,
    Path(id): Path<i64>,
    Json(req): Json<PatchShareReq>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    let share = st
        .db
        .share(id)
        .await?
        .ok_or_else(|| ApiError::not_found("共享不存在"))?;
    let name = req.name.unwrap_or(share.name);
    let host = match req.host_path {
        Some(h) => path_ok(&PathBuf::from(&h))?,
        None => share.host_path.clone(),
    };
    st.db.update_share(id, &name, &host).await?;
    Ok(ok(json!({ "ok": true })))
}

pub async fn delete(
    State(st): State<AppState>,
    identity: Identity,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    st.db.delete_share(id).await?;
    Ok(ok(json!({ "ok": true })))
}

pub async fn rules_list(
    State(st): State<AppState>,
    identity: Identity,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    let rules = st.db.rules_of(id).await?;
    let out: Vec<_> = rules
        .iter()
        .map(|r| json!({ "id": r.id, "rel_path": r.rel_path, "access": r.level.as_str() }))
        .collect();
    Ok(ok(json!({ "rules": out })))
}

#[derive(Deserialize)]
pub struct RuleReq {
    #[serde(default)]
    rel_path: String,
    access: String,
}

pub async fn rule_add(
    State(st): State<AppState>,
    identity: Identity,
    Path(id): Path<i64>,
    Json(req): Json<RuleReq>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    let level = Level::parse(&req.access)
        .ok_or_else(|| ApiError::bad_request("access 必须是 guest/login/admin"))?;
    let rel = paths::normalize(&req.rel_path).map_err(ApiError::bad_request)?;
    let rule_id = st.db.upsert_rule(id, &rel, level).await?;
    Ok(ok(json!({ "id": rule_id, "ok": true })))
}

pub async fn rule_delete(
    State(st): State<AppState>,
    identity: Identity,
    Path((id, rid)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    // 规则必须属于路径中的共享,防止跨共享删除
    if !st.db.delete_rule(id, rid).await? {
        return Err(ApiError::not_found("规则不存在"));
    }
    Ok(ok(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct HostBrowseQ {
    #[serde(default)]
    path: String,
    /// true 时同时列出证书文件(.crt/.cer/.pem/.key)
    #[serde(default)]
    files: Option<String>,
}

/// 浏览服务器本机目录(仅管理员):共享目录与证书文件选择器共用。
pub async fn browse_host(
    State(_st): State<AppState>,
    identity: Identity,
    Query(q): Query<HostBrowseQ>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    let raw = q.path.trim();
    let want_files = matches!(q.files.as_deref(), Some("1") | Some("true") | Some("yes"));
    // 根:Windows 返回盘符列表,Linux 返回 / 内容
    if raw.is_empty() || raw == "/" {
        let drives = paths::list_drives();
        if !drives.is_empty() {
            return Ok(ok(
                json!({ "path": "/", "parent": serde_json::Value::Null, "dirs": drives, "files": [] }),
            ));
        }
        return list_dir(PathBuf::from("/"), "/", None, want_files);
    }
    // 盘符输入(如 C: 或 C:/)规范化为 C:\
    let mut p = raw.to_string();
    if cfg!(windows) {
        let b = p.as_bytes();
        if b.len() == 2 && b[1] == b':' {
            p = format!("{p}\\");
        } else if b.len() == 3 && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\') {
            p = format!("{}\\", &p[..2]);
        }
    }
    let dir = PathBuf::from(&p);
    if !dir.is_dir() {
        return Err(ApiError::not_found("目录不存在"));
    }
    let real = dir.canonicalize().unwrap_or(dir.clone());
    let display = paths::pretty(&real);
    let parent = real.parent().map(paths::pretty).unwrap_or_default();
    list_dir(
        real,
        &display,
        if parent.is_empty() {
            None
        } else {
            Some(parent)
        },
        want_files,
    )
}

fn list_dir(
    dir: PathBuf,
    display: &str,
    parent: Option<String>,
    want_files: bool,
) -> ApiResult<Response> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for de in std::fs::read_dir(&dir).map_err(|e| ApiError::internal(format!("读取失败: {e}")))?
    {
        let Ok(de) = de else { continue };
        let name = de.file_name().to_string_lossy().to_string();
        let Ok(meta) = de.metadata() else { continue };
        if meta.is_dir() {
            dirs.push(name);
        } else if want_files && meta.is_file() {
            let ext = paths::ext_of(&name);
            if ["crt", "cer", "pem", "key"].contains(&ext.as_str()) {
                files.push(name);
            }
        }
    }
    dirs.sort_by_key(|a| a.to_lowercase());
    files.sort_by_key(|a| a.to_lowercase());
    Ok(ok(
        json!({ "path": display, "parent": parent, "dirs": dirs, "files": files }),
    ))
}
