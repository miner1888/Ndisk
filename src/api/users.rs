use axum::extract::{Path, State};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::domain::auth::{self, Identity};
use crate::domain::paths;
use crate::error::{ok, ApiError, ApiResult};
use crate::state::AppState;

/// 用户列表仅管理员可见(避免普通用户窥探其他账号)。
pub async fn list(State(st): State<AppState>, identity: Identity) -> ApiResult<Response> {
    identity.require_adminish()?;
    Ok(ok(json!(st.db.all_users().await?)))
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

/// 用户能力开关(对象格式,缺省全关)。
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
        [
            self.can_upload,
            self.can_download,
            self.can_delete,
            self.can_mkdir,
        ]
    }
}

pub async fn create(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<NewUserReq>,
) -> ApiResult<Response> {
    let me = identity.require()?;
    if !me.is_admin {
        return Err(ApiError::forbidden("需要管理员权限"));
    }
    let name = paths::check_name(&req.username).map_err(|_| ApiError::bad_request("非法用户名"))?;
    auth::validate_credentials(&name, &req.password)?;
    if st.db.user_by_name(&name).await?.is_some() {
        return Err(ApiError::conflict("用户名已存在"));
    }
    let hash = auth::hash_password(&req.password).map_err(ApiError::bad_request)?;
    let flags = req
        .permit
        .map(|p| p.flags())
        .unwrap_or([true, true, false, true]);
    let id = st.db.create_user(&name, &hash, req.is_admin, flags).await?;
    // 每个账号(含管理员)都有独立空间 data/users/<用户名>,互不混放
    let home = st.home_dir(&name);
    std::fs::create_dir_all(&home)?;
    st.db
        .create_share(
            crate::state::HOME_NAME,
            &paths::pretty(&home),
            Some(id),
            "home",
        )
        .await
        .ok();
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
    State(st): State<AppState>,
    identity: Identity,
    Path(id): Path<i64>,
    Json(req): Json<PatchUserReq>,
) -> ApiResult<Response> {
    let me = identity.require()?;
    if !me.is_admin {
        return Err(ApiError::forbidden("需要管理员权限"));
    }
    let target = st
        .db
        .user_by_id(id)
        .await?
        .ok_or_else(|| ApiError::not_found("用户不存在"))?;
    if let Some(pw) = &req.password {
        auth::validate_credentials(&target.username, pw)?;
        let hash = auth::hash_password(pw).map_err(ApiError::bad_request)?;
        st.db.set_password(target.id, &hash).await?;
    }
    let new_admin = req.is_admin.unwrap_or(target.is_admin);
    // 与删除同理:最后一个管理员不可降级,否则实例失去管理员
    if target.is_admin && !new_admin && st.db.count_admins().await? <= 1 {
        return Err(ApiError::bad_request("最后一个管理员不能降级"));
    }
    let flags = req
        .permit
        .map(|p| p.flags())
        .unwrap_or_else(|| target.flags());
    st.db.update_flags(target.id, new_admin, flags).await?;
    Ok(ok(json!({ "ok": true })))
}

pub async fn delete(
    State(st): State<AppState>,
    identity: Identity,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let me = identity.require()?.clone();
    if !me.is_admin {
        return Err(ApiError::forbidden("需要管理员权限"));
    }
    if me.id == id {
        return Err(ApiError::bad_request("不能删除自己"));
    }
    let target = st
        .db
        .user_by_id(id)
        .await?
        .ok_or_else(|| ApiError::not_found("用户不存在"))?;
    if target.is_admin && st.db.count_admins().await? <= 1 {
        return Err(ApiError::bad_request("最后一个管理员不能删除"));
    }
    // 个人空间目录仅在位于数据目录内时才删除,防止误删外部目录
    let home_root = st
        .users_dir
        .canonicalize()
        .unwrap_or_else(|_| st.users_dir.clone());
    for s in st.db.all_shares().await? {
        if s.kind == "home" && s.owner_id == Some(id) {
            let _ = st.db.delete_share(s.id).await;
            if let Ok(p) = std::path::PathBuf::from(&s.host_path).canonicalize() {
                if p.starts_with(&home_root) && p.is_dir() {
                    let _ = std::fs::remove_dir_all(&p);
                }
            }
        }
    }
    st.db.delete_user(id).await?;
    Ok(ok(json!({ "ok": true })))
}
