use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::domain::auth::{self, Identity};
use crate::error::{err, ok, ApiError, ApiResult};
use crate::state::{AppState, CAPTCHA_TTL};

pub async fn captcha(State(st): State<AppState>) -> Response {
    let (id, code) = st.issue_captcha().await;
    Json(json!({ "id": id, "svg": render_svg(&code), "ttl": CAPTCHA_TTL })).into_response()
}

/// 渲染 4 字符验证码 SVG:随机位置 / 旋转 / 配色 + 干扰线。
fn render_svg(code: &str) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let palette = [
        "#4e6ef2", "#e5484d", "#1fa66e", "#f59e0b", "#7c6cf0", "#12a5b0",
    ];
    let mut svg = String::from(
        "<svg xmlns='http://www.w3.org/2000/svg' width='132' height='42' viewBox='0 0 132 42'>",
    );
    svg.push_str("<rect width='132' height='42' fill='#f2f5fc' rx='8'/>");
    for _ in 0..6 {
        let (x1, y1, x2, y2) = (
            rng.gen_range(0..132),
            rng.gen_range(0..42),
            rng.gen_range(0..132),
            rng.gen_range(0..42),
        );
        let c = palette[rng.gen_range(0..palette.len())];
        svg.push_str(&format!("<line x1='{x1}' y1='{y1}' x2='{x2}' y2='{y2}' stroke='{c}' stroke-opacity='0.28' stroke-width='1'/>"));
    }
    for (i, ch) in code.chars().enumerate() {
        let x = 17 + i as i32 * 28 + rng.gen_range(-3..4);
        let y = 27 + rng.gen_range(-4..5);
        let rot = rng.gen_range(-22..23);
        let c = palette[rng.gen_range(0..palette.len())];
        svg.push_str(&format!(
            "<text x='{x}' y='{y}' fill='{c}' font-size='21' font-family='Consolas,Menlo,monospace' font-weight='bold' transform='rotate({rot} {x} {y})'>{ch}</text>"
        ));
    }
    svg.push_str("</svg>");
    svg
}

#[derive(Deserialize)]
pub struct LoginReq {
    username: String,
    password: String,
    #[serde(default)]
    captcha_id: Option<String>,
    #[serde(default)]
    captcha: Option<String>,
}

pub async fn login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> Response {
    let ok_cap = match (&req.captcha_id, &req.captcha) {
        (Some(id), Some(code)) => st.check_captcha(id, code).await,
        _ => false,
    };
    if !ok_cap {
        return err(StatusCode::FORBIDDEN, "验证码错误或已过期,请重试");
    }
    let user = match st.db.user_by_name(&req.username).await {
        Ok(Some(u)) => u,
        Ok(None) => return err(StatusCode::UNAUTHORIZED, "用户名或密码错误"),
        Err(e) => return ApiError::from(e).into_response(),
    };
    if !auth::verify_password(&req.password, &user.password_hash) {
        return err(StatusCode::UNAUTHORIZED, "用户名或密码错误");
    }
    let token = auth::new_token();
    if let Err(e) = st
        .db
        .create_session(user.id, &token, auth::SESSION_TTL)
        .await
    {
        return ApiError::from(e).into_response();
    }
    let mut resp = ok(json!({ "token": token, "user": user }));
    auth::session_cookie(&mut resp, &token, auth::SESSION_TTL);
    resp
}

pub async fn logout(State(st): State<AppState>, identity: Identity) -> Response {
    if let Some(t) = identity.token {
        let _ = st.db.drop_session(&t).await;
    }
    let mut resp = ok(json!({ "ok": true }));
    auth::session_cookie(&mut resp, "", 0);
    resp
}

pub async fn me(identity: Identity) -> ApiResult<Response> {
    let u = identity.require()?;
    Ok(ok(json!(u)))
}

#[derive(Deserialize)]
pub struct PasswordReq {
    #[serde(default)]
    old_password: Option<String>,
    password: String,
}

pub async fn change_password(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<PasswordReq>,
) -> ApiResult<Response> {
    let u = identity.require()?;
    let old = req
        .old_password
        .ok_or_else(|| ApiError::bad_request("需要提供原密码"))?;
    if !auth::verify_password(&old, &u.password_hash) {
        return Err(ApiError::bad_request("原密码错误"));
    }
    auth::validate_credentials(&u.username, &req.password)?;
    let hash = auth::hash_password(&req.password).map_err(ApiError::bad_request)?;
    st.db.set_password(u.id, &hash).await?;
    st.db
        .drop_other_sessions(u.id, identity.token.as_deref().unwrap_or(""))
        .await?;
    Ok(ok(json!({ "ok": true })))
}
