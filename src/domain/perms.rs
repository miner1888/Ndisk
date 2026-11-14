use crate::db::{AccessRule, Share, User};
use crate::error::ApiError;

/// 目录访问级别:游客 < 登录 < 管理员。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Guest,
    Login,
    Admin,
}

impl Level {
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Guest => "guest",
            Level::Login => "login",
            Level::Admin => "admin",
        }
    }
    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "guest" => Some(Level::Guest),
            "login" => Some(Level::Login),
            "admin" => Some(Level::Admin),
            _ => None,
        }
    }
}

/// 规则覆盖判断:空规则路径 = 全局;否则要求路径相等或在其子树下。
fn covers(rule_path: &str, rel: &str) -> bool {
    rule_path.is_empty() || rel == rule_path || rel.starts_with(&format!("{rule_path}/"))
}

/// 取某路径的访问级别:命中的最深规则优先;无规则时默认仅管理员。
pub fn level_of(rules: &[AccessRule], rel: &str) -> Level {
    rules
        .iter()
        .filter(|r| covers(&r.rel_path, rel))
        .max_by_key(|r| (r.rel_path.matches('/').count(), r.rel_path.len()))
        .map(|r| r.level)
        .unwrap_or(Level::Admin)
}

/// 私有空间(个人空间):只有属主与管理员可见,忽略规则表。
pub fn is_private(share: &Share) -> bool {
    share.owner_id.is_some()
}

pub fn is_owner(share: &Share, user: &User) -> bool {
    user.is_admin || share.owner_id == Some(user.id)
}

/// 读权限:私有空间限属主;公开空间按规则级别判断。
pub fn can_read(
    identity: &crate::domain::auth::Identity,
    share: &Share,
    rules: &[AccessRule],
    rel: &str,
) -> bool {
    if is_private(share) {
        return identity
            .user
            .as_ref()
            .map(|u| is_owner(share, u))
            .unwrap_or(false);
    }
    match level_of(rules, rel) {
        Level::Guest => true,
        Level::Login => identity.user.is_some(),
        Level::Admin => identity.is_admin(),
    }
}

pub fn ensure_read(
    identity: &crate::domain::auth::Identity,
    share: &Share,
    rules: &[AccessRule],
    rel: &str,
) -> Result<(), ApiError> {
    if can_read(identity, share, rules, rel) {
        return Ok(());
    }
    if identity.user.is_none() {
        return Err(ApiError::unauthorized("该目录需要登录后才能访问"));
    }
    Err(ApiError::forbidden("无权访问该目录"))
}

/// 下载 = 可读;登录用户额外要求账号下载开关(游客与私有空间属主不受限)。
pub fn can_download(
    identity: &crate::domain::auth::Identity,
    share: &Share,
    rules: &[AccessRule],
    rel: &str,
) -> bool {
    if !can_read(identity, share, rules, rel) {
        return false;
    }
    match identity.user.as_ref() {
        Some(u) => is_private(share) || u.can_download,
        None => true,
    }
}

/// 以显式用户身份判断下载权(分享链接访问时复核创建者权限)。
pub fn can_download_user(user: &User, share: &Share, rules: &[AccessRule], rel: &str) -> bool {
    if is_private(share) {
        return is_owner(share, user);
    }
    if level_of(rules, rel) == Level::Admin && !user.is_admin {
        return false;
    }
    user.can_download
}

pub fn ensure_download(
    identity: &crate::domain::auth::Identity,
    share: &Share,
    rules: &[AccessRule],
    rel: &str,
) -> Result<(), ApiError> {
    if can_download(identity, share, rules, rel) {
        Ok(())
    } else if identity.user.is_none() {
        Err(ApiError::unauthorized("该目录需要登录后才能访问"))
    } else {
        Err(ApiError::forbidden("当前账号无下载权限"))
    }
}

/// 写操作(上传 / 建目录 / 重命名 / 删除):必须登录,私有空间限属主,
/// 公开空间 admin 级目录限管理员,并要求账号对应能力开关打开。
/// 游客天然被拒(未登录),实现"游客只读,登录用户才可修改"。
pub fn can_write(
    identity: &crate::domain::auth::Identity,
    share: &Share,
    rules: &[AccessRule],
    rel: &str,
    flag: impl Fn(&User) -> bool,
) -> bool {
    let Some(u) = identity.user.as_ref() else {
        return false;
    };
    let authorized = if is_private(share) {
        is_owner(share, u)
    } else {
        level_of(rules, rel) != Level::Admin || u.is_admin
    };
    authorized && flag(u)
}

pub fn ensure_write(
    identity: &crate::domain::auth::Identity,
    share: &Share,
    rules: &[AccessRule],
    rel: &str,
    flag: impl Fn(&User) -> bool,
    what: &str,
) -> Result<(), ApiError> {
    if identity.user.is_none() {
        return Err(ApiError::unauthorized("需要登录"));
    }
    if !can_write(identity, share, rules, rel, flag) {
        return Err(ApiError::forbidden(format!("无权在当前目录{what}")));
    }
    Ok(())
}
