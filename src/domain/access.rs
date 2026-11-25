//! 访问权限模型:guest < login < admin 三级。
//!
//! 规则向下继承、深路径优先(最具体规则的级别就是有效级别);
//! 个人空间(`owner_id` 非空)永远是私有的,只有属主与管理员能进。

use crate::db::{AccessRule, Share, User};
use crate::error::ApiError;

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

/// `rule_path` 是否覆盖 `rel`(根规则 `""` 覆盖一切)。
fn covers(rule_path: &str, rel: &str) -> bool {
    rule_path.is_empty() || rel == rule_path || rel.starts_with(&format!("{rule_path}/"))
}

/// 给定路径的有效访问级别:所有覆盖它的规则中,取路径最具体的那条。
/// 无规则时默认 admin(只对管理员开放)。
pub fn level_of(rules: &[AccessRule], rel: &str) -> Level {
    rules
        .iter()
        .filter(|r| covers(&r.rel_path, rel))
        .max_by_key(|r| (r.rel_path.matches('/').count(), r.rel_path.len()))
        .map(|r| r.level)
        .unwrap_or(Level::Admin)
}

pub fn is_private(share: &Share) -> bool {
    share.owner_id.is_some()
}

/// 管理员视为一切个人空间的"虚拟属主"。
pub fn is_owner(share: &Share, user: &User) -> bool {
    user.is_admin || share.owner_id == Some(user.id)
}

pub fn can_read(user: Option<&User>, share: &Share, rules: &[AccessRule], rel: &str) -> bool {
    if is_private(share) {
        return user.map(|u| is_owner(share, u)).unwrap_or(false);
    }
    match level_of(rules, rel) {
        Level::Guest => true,
        Level::Login => user.is_some(),
        Level::Admin => user.is_some_and(|u| u.is_admin),
    }
}

pub fn ensure_read(user: Option<&User>, share: &Share, rules: &[AccessRule], rel: &str) -> Result<(), ApiError> {
    if can_read(user, share, rules, rel) {
        return Ok(());
    }
    if user.is_none() {
        return Err(ApiError::unauthorized("该目录需要登录后才能访问"));
    }
    Err(ApiError::forbidden("无权访问该目录"))
}

/// 读到 ≠ 能下载:普通账号还受 `can_download` 能力开关约束(游客不受账号开关约束)。
pub fn can_download(user: Option<&User>, share: &Share, rules: &[AccessRule], rel: &str) -> bool {
    if !can_read(user, share, rules, rel) {
        return false;
    }
    match user {
        Some(u) => is_private(share) || u.can_download,
        None => true,
    }
}

/// 分享链接有效性的判定:以**链接创建者**的能力为准,而不是访问者。
pub fn can_download_user(user: &User, share: &Share, rules: &[AccessRule], rel: &str) -> bool {
    if is_private(share) {
        return is_owner(share, user);
    }
    if level_of(rules, rel) == Level::Admin && !user.is_admin {
        return false;
    }
    user.can_download
}

pub fn ensure_download(user: Option<&User>, share: &Share, rules: &[AccessRule], rel: &str) -> Result<(), ApiError> {
    if can_download(user, share, rules, rel) {
        Ok(())
    } else if user.is_none() {
        Err(ApiError::unauthorized("该目录需要登录后才能访问"))
    } else {
        Err(ApiError::forbidden("当前账号无下载权限"))
    }
}

/// 写操作 = 目录级别授权 + 账号能力开关(`flag`)双重检查。
pub fn can_write(
    user: Option<&User>,
    share: &Share,
    rules: &[AccessRule],
    rel: &str,
    flag: impl Fn(&User) -> bool,
) -> bool {
    let Some(u) = user else { return false };
    let authorized = if is_private(share) {
        is_owner(share, u)
    } else {
        level_of(rules, rel) != Level::Admin || u.is_admin
    };
    authorized && flag(u)
}

pub fn ensure_write(
    user: Option<&User>,
    share: &Share,
    rules: &[AccessRule],
    rel: &str,
    flag: impl Fn(&User) -> bool,
    what: &str,
) -> Result<(), ApiError> {
    if user.is_none() {
        return Err(ApiError::unauthorized("需要登录"));
    }
    if !can_write(user, share, rules, rel, flag) {
        return Err(ApiError::forbidden(format!("无权在当前目录{what}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(rel: &str, level: Level) -> AccessRule {
        AccessRule { id: 0, rel_path: rel.to_string(), level }
    }

    #[test]
    fn deepest_rule_wins() {
        let rules = [rule("", Level::Guest), rule("pub", Level::Admin)];
        assert_eq!(level_of(&rules, ""), Level::Guest);
        assert_eq!(level_of(&rules, "pub"), Level::Admin);
        assert_eq!(level_of(&rules, "pub/deep/x"), Level::Admin);
        assert_eq!(level_of(&rules, "other"), Level::Guest);
        // 同深度两条规则:更长者(更具体)优先。
        let rules2 = [rule("ab", Level::Guest), rule("abc", Level::Login)];
        assert_eq!(level_of(&rules2, "abc"), Level::Login);
    }

    #[test]
    fn private_share_is_owner_only() {
        let share = Share { id: 1, name: "h".into(), host_path: "p".into(), owner_id: Some(7), kind: "home".into() };
        let owner = User {
            id: 7,
            username: "u".into(),
            password_hash: String::new(),
            is_admin: false,
            can_upload: true,
            can_download: true,
            can_delete: true,
            can_mkdir: true,
            created_at: 0,
        };
        let other = User { id: 8, is_admin: false, ..owner.clone() };
        let admin = User { id: 9, is_admin: true, ..owner.clone() };
        assert!(can_read(Some(&owner), &share, &[], ""));
        assert!(!can_read(Some(&other), &share, &[], ""));
        assert!(can_read(Some(&admin), &share, &[], ""));
        assert!(!can_read(None, &share, &[], ""));
    }
}
