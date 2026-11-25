//! 共享空间与访问规则,以及服务器目录选择器(管理员)。

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::app::App;
use crate::db::{AccessRule, Share};
use crate::domain::access::{self, Level};
use crate::domain::pathguard;
use crate::error::{ok, ApiError, ApiResult};
use crate::web::identity::Identity;

/// 顶层可见性判定(与 access::can_read 对根路径的语义一致)。
fn share_visible(identity: &Identity, share: &Share, rules: &[AccessRule]) -> bool {
    if access::is_private(share) {
        return identity.as_user().map(|u| access::is_owner(share, u)).unwrap_or(false);
    }
    match access::level_of(rules, "") {
        Level::Guest => true,
        Level::Login => identity.as_user().is_some(),
        Level::Admin => identity.is_admin(),
    }
}

pub async fn list(State(app): State<App>, identity: Identity) -> ApiResult<Response> {
    let mut out = Vec::new();
    let users = app.db.all_users().await.unwrap_or_default();
    let name_of = |id: i64| {
        users
            .iter()
            .find(|u| u.id == id)
            .map(|u| u.username.clone())
            .unwrap_or_default()
    };
    for s in app.db.all_shares().await? {
        let rules = app.db.rules_of(s.id).await?;
        if !share_visible(&identity, &s, &rules) {
            continue;
        }
        out.push(json!({
            "id": s.id,
            "name": s.name,
            "kind": s.kind,
            "owner_id": s.owner_id,
            "owner_name": s.owner_id.map(name_of).unwrap_or_default(),
            "host_path": if identity.is_admin() { s.host_path.clone() } else { String::new() },
            "owner": identity.as_user().map(|u| access::is_owner(&s, u)).unwrap_or(false),
            "mine": s.owner_id.is_some() && s.owner_id == identity.as_user().map(|u| u.id),
            "access": if access::is_private(&s) { "private".to_string() } else { access::level_of(&rules, "").as_str().to_string() },
            "home": s.kind == "home",
        }));
    }
    Ok(ok(json!({ "shares": out, "direct_max": app.chunk_size })))
}

/// 校验并准备挂载目录:不能是文件;不存在则创建;返回规范化显示路径。
fn path_ok(dir: &PathBuf) -> Result<String, ApiError> {
    if dir.is_file() {
        return Err(ApiError::bad_request("路径是文件而非目录"));
    }
    std::fs::create_dir_all(dir).map_err(|e| ApiError::bad_request(format!("无法创建目录: {e}")))?;
    Ok(pathguard::pretty(&dir.canonicalize().unwrap_or_else(|_| dir.clone())))
}

#[derive(Deserialize)]
pub struct NewShareReq {
    name: String,
    host_path: String,
}

pub async fn create(State(app): State<App>, identity: Identity, Json(req): Json<NewShareReq>) -> ApiResult<Response> {
    identity.require_admin()?;
    let host = path_ok(&PathBuf::from(&req.host_path))?;
    let id = app.db.create_share(&req.name, &host, None, "shared").await?;
    app.db.upsert_rule(id, "", Level::Login).await?;
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
    State(app): State<App>,
    identity: Identity,
    Path(id): Path<i64>,
    Json(req): Json<PatchShareReq>,
) -> ApiResult<Response> {
    identity.require_admin()?;
    let share = app.db.share(id).await?.ok_or_else(|| ApiError::not_found("共享不存在"))?;
    let name = req.name.unwrap_or(share.name);
    let host = match req.host_path {
        Some(h) => path_ok(&PathBuf::from(&h))?,
        None => share.host_path.clone(),
    };
    app.db.update_share(id, &name, &host).await?;
    Ok(ok(json!({ "ok": true })))
}

pub async fn delete(State(app): State<App>, identity: Identity, Path(id): Path<i64>) -> ApiResult<Response> {
    identity.require_admin()?;
    app.db.delete_share(id).await?;
    Ok(ok(json!({ "ok": true })))
}

pub async fn rules_list(State(app): State<App>, identity: Identity, Path(id): Path<i64>) -> ApiResult<Response> {
    identity.require_admin()?;
    let rules = app.db.rules_of(id).await?;
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
    State(app): State<App>,
    identity: Identity,
    Path(id): Path<i64>,
    Json(req): Json<RuleReq>,
) -> ApiResult<Response> {
    identity.require_admin()?;
    let level = Level::parse(&req.access).ok_or_else(|| ApiError::bad_request("access 必须是 guest/login/admin"))?;
    let rel = pathguard::normalize(&req.rel_path).map_err(ApiError::bad_request)?;
    let rule_id = app.db.upsert_rule(id, &rel, level).await?;
    Ok(ok(json!({ "id": rule_id, "ok": true })))
}

pub async fn rule_delete(
    State(app): State<App>,
    identity: Identity,
    Path((id, rid)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    identity.require_admin()?;
    if !app.db.delete_rule(id, rid).await? {
        return Err(ApiError::not_found("规则不存在"));
    }
    Ok(ok(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct HostBrowseQ {
    #[serde(default)]
    path: String,
    #[serde(default)]
    files: Option<String>,
}

/// 服务器目录选择器:Windows 返回盘符列表;`files=1` 时附带证书类文件。
pub async fn browse_host(
    State(_app): State<App>,
    identity: Identity,
    Query(q): Query<HostBrowseQ>,
) -> ApiResult<Response> {
    identity.require_admin()?;
    let raw = q.path.trim();
    let want_files = matches!(q.files.as_deref(), Some("1") | Some("true") | Some("yes"));
    if raw.is_empty() || raw == "/" {
        let drives = pathguard::list_drives();
        if !drives.is_empty() {
            return Ok(ok(json!({ "path": "/", "parent": serde_json::Value::Null, "dirs": drives, "files": [] })));
        }
        return list_dir(PathBuf::from("/"), "/", None, want_files);
    }
    let mut p = raw.to_string();
    if cfg!(windows) {
        // "D:" / "D:/" / "D:\" → "D:\"
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
    let display = pathguard::pretty(&real);
    let parent = real.parent().map(pathguard::pretty).unwrap_or_default();
    list_dir(real, &display, if parent.is_empty() { None } else { Some(parent) }, want_files)
}

fn list_dir(dir: PathBuf, display: &str, parent: Option<String>, want_files: bool) -> ApiResult<Response> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for de in std::fs::read_dir(&dir).map_err(|e| ApiError::internal(format!("读取失败: {e}")))? {
        let Ok(de) = de else { continue };
        let name = de.file_name().to_string_lossy().to_string();
        let Ok(meta) = de.metadata() else { continue };
        if meta.is_dir() {
            dirs.push(name);
        } else if want_files && meta.is_file() {
            let ext = pathguard::ext_of(&name);
            if ["crt", "cer", "pem", "key"].contains(&ext.as_str()) {
                files.push(name);
            }
        }
    }
    dirs.sort_by_key(|a| a.to_lowercase());
    files.sort_by_key(|a| a.to_lowercase());
    Ok(ok(json!({ "path": display, "parent": parent, "dirs": dirs, "files": files })))
}
