use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// 请求体上限:256 MiB(分片上传单片远小于此)。
pub const MAX_BODY: usize = 256 * 1024 * 1024;

pub fn build(st: AppState) -> Router {
    use crate::api as h;
    Router::new()
        // 认证
        .route("/api/captcha", get(h::auth::captcha))
        .route("/api/auth/login", post(h::auth::login))
        .route("/api/auth/logout", post(h::auth::logout))
        .route("/api/auth/me", get(h::auth::me))
        .route("/api/auth/password", post(h::auth::change_password))
        // 系统状态与维护
        .route("/api/admin/status", get(h::admin::status))
        .route("/api/admin/cache/clear", post(h::admin::clear_cache))
        .route(
            "/api/admin/transcode-replace",
            post(h::admin::set_transcode_replace),
        )
        // 用户管理
        .route("/api/users", get(h::users::list).post(h::users::create))
        .route(
            "/api/users/{id}",
            patch(h::users::patch).delete(h::users::delete),
        )
        // 共享目录与规则
        .route("/api/shares", get(h::shares::list).post(h::shares::create))
        .route(
            "/api/shares/{id}",
            patch(h::shares::patch).delete(h::shares::delete),
        )
        .route(
            "/api/shares/{id}/rules",
            get(h::shares::rules_list).post(h::shares::rule_add),
        )
        .route(
            "/api/shares/{id}/rules/{rid}",
            delete(h::shares::rule_delete),
        )
        .route("/api/browse-host", get(h::shares::browse_host))
        // 文件
        .route("/api/browse/{share_id}", get(h::files::browse))
        .route("/api/album/{share_id}", get(h::files::album))
        .route("/api/file/meta/{share_id}", get(h::files::meta))
        .route("/api/file/detail/{share_id}", get(h::files::detail))
        .route(
            "/api/file/text/{share_id}",
            get(h::files::text_get).post(h::files::text_put),
        )
        .route("/api/file/binary/{share_id}", post(h::files::binary_put))
        .route("/api/mkdir", post(h::files::mkdir))
        .route("/api/rename", post(h::files::rename))
        .route("/api/move", post(h::files::move_items))
        .route("/api/delete", post(h::files::delete))
        .route("/api/zip", get(h::files::zip_get).post(h::files::zip_post))
        .route("/api/file/dlticket", post(h::files::dlticket))
        .route("/api/dl/{share_id}/{*name}", get(h::files::dl_signed))
        .route("/dl/{*path}", get(h::files::dl_short))
        // 上传
        .route("/api/upload/init", post(h::uplink::init))
        .route("/api/upload/direct", post(h::uplink::direct_upload))
        .route("/api/uploads", get(h::uplink::list_mine))
        .route("/api/upload/parts/{token}", get(h::uplink::parts))
        .route("/api/upload/part/{token}/{part}", put(h::uplink::part))
        .route("/api/upload/complete/{token}", post(h::uplink::complete))
        .route("/api/upload/{token}", delete(h::uplink::abort))
        // 媒体
        .route(
            "/api/media/{share_id}",
            get(h::media::get).head(h::media::head),
        )
        .route("/api/media/status/{share_id}", get(h::media::status))
        .route("/api/media/thumb/{share_id}", get(h::media::video_thumb))
        .route("/api/media/imgthumb/{share_id}", get(h::media::image_thumb))
        // 分享链接
        .route(
            "/api/fileshares",
            get(h::links::list).post(h::links::create),
        )
        .route("/api/fileshares/{token}", delete(h::links::delete))
        .route("/s/{token}", get(h::links::page))
        .route("/s/{token}/api/download", get(h::links::download))
        // TLS
        .route(
            "/api/tls",
            get(h::tls::get).put(h::tls::put).delete(h::tls::delete),
        )
        // 页面与静态资源
        .route("/api/config", get(h::misc::config))
        .route("/", get(h::misc::index))
        .route("/static/{*path}", get(h::misc::static_file))
        .fallback(h::misc::fallback)
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(TraceLayer::new_for_http())
        .with_state(st)
}
