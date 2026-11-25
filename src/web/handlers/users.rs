//! 用户管理(仅管理员):列表 / 创建 / 改权限 / 重置密码 / 删除。

use axum::extract::{Path, State};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::app::{App, HOME_NAME};
use crate::domain::{auth, pathguard};
use crate::error::{ok, ApiError, ApiResult};
use crate::web::identity::Identity;

pub async fn list(State(app): State<App>, identity: Identity) -> ApiResult<Response> {
    identity.require_admin()?;
    Ok(ok(json!(app.db.all_users().await?)))
}

/// 能力开关(创建 / 修改共用)。
#[derive(Deserialize, Default, Clone, Copy)]
pub struct Permit {
    #[serde(default)]
    can_upload: bool,
    #[serde(default)]
    can_download: bool,
    #[serde(default)]
    can_mkdir: bool,
    #[serde(default)]
    can_delete: bool,
}

impl Permit {
    fn flags(self) -> [bool; 4] {
        [self.can_upload, self.can_download, self.can_delete, self.can_mkdir]
    }
}

#[derive(Deserialize)]
pub struct NewUserReq {
    username: String,
    password: String,
    #[serde(default)]
    is_admin: bool,
    #[serde(default)]
    permit: Option<Permit>,
}

pub async fn create(State(app): State<App>, identity: Identity, Json(req): Json<NewUserReq>) -> ApiResult<Response> {
    let me = identity.require()?;
    if !me.is_admin {
        return Err(ApiError::forbidden("需要管理员权限"));
    }
    let name = pathguard::check_name(&req.username).map_err(|_| ApiError::bad_request("非法用户名"))?;
    auth::validate_credentials(&name, &req.password)?;
    if app.db.user_by_name(&name).await?.is_some() {
        return Err(ApiError::conflict("用户名已存在"));
    }
    let hash = auth::hash_password(&req.password).map_err(ApiError::bad_request)?;
    // 默认能力:上传 / 下载 / 建目录开,删除关(与历史一致)。
    let flags = req.permit.map(|p| p.flags()).unwrap_or([true, true, false, true]);
    let id = app.db.create_user(&name, &hash, req.is_admin, flags).await?;
    let home = app.home_dir(&name);
    std::fs::create_dir_all(&home)?;
    app.db.create_share(HOME_NAME, &pathguard::pretty(&home), Some(id), "home").await.ok();
    Ok(ok(json!({ "id": id, "ok": true })))
}

#[derive(Deserialize)]
pub struct PatchUserReq {
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    is_admin: Option<bool>,
    #[serde(default)]
    permit: Option<Permit>,
}

pub async fn patch(
    State(app): State<App>,
    identity: Identity,
    Path(id): Path<i64>,
    Json(req): Json<PatchUserReq>,
) -> ApiResult<Response> {
    let me = identity.require()?;
    if !me.is_admin {
        return Err(ApiError::forbidden("需要管理员权限"));
    }
    let target = app.db.user_by_id(id).await?.ok_or_else(|| ApiError::not_found("用户不存在"))?;
    if let Some(pw) = &req.password {
        auth::validate_credentials(&target.username, pw)?;
        let hash = auth::hash_password(pw).map_err(ApiError::bad_request)?;
        app.db.set_password(target.id, &hash).await?;
    }
    let new_admin = req.is_admin.unwrap_or(target.is_admin);
    if target.is_admin && !new_admin && app.db.count_admins().await? <= 1 {
        return Err(ApiError::bad_request("最后一个管理员不能降级"));
    }
    let flags = req.permit.map(|p| p.flags()).unwrap_or_else(|| target.flags());
    app.db.update_flags(target.id, new_admin, flags).await?;
    Ok(ok(json!({ "ok": true })))
}

pub async fn delete(State(app): State<App>, identity: Identity, Path(id): Path<i64>) -> ApiResult<Response> {
    let me = identity.require()?.clone();
    if !me.is_admin {
        return Err(ApiError::forbidden("需要管理员权限"));
    }
    if me.id == id {
        return Err(ApiError::bad_request("不能删除自己"));
    }
    let target = app.db.user_by_id(id).await?.ok_or_else(|| ApiError::not_found("用户不存在"))?;
    if target.is_admin && app.db.count_admins().await? <= 1 {
        return Err(ApiError::bad_request("最后一个管理员不能删除"));
    }
    // 删除其个人空间共享登记与目录(仅在数据目录内才删,防误删外部路径)。
    let home_root = app.users_dir.canonicalize().unwrap_or_else(|_| app.users_dir.clone());
    for s in app.db.all_shares().await? {
        if s.kind == "home" && s.owner_id == Some(id) {
            let _ = app.db.delete_share(s.id).await;
            if let Ok(p) = std::path::PathBuf::from(&s.host_path).canonicalize() {
                if p.starts_with(&home_root) && p.is_dir() {
                    let _ = std::fs::remove_dir_all(&p);
                }
            }
        }
    }
    app.db.delete_user(id).await?;
    Ok(ok(json!({ "ok": true })))
}
