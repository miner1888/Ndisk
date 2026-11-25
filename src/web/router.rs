//! 路由表:与历史版本逐条一致的方法 + 路径(前端与第三方客户端零改动兼容)。

use axum::extract::DefaultBodyLimit;
use axum::routing::{any, delete, get, patch, post, put};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::app::App;

/// 请求体上限 256MB(WebDAV PUT 与直传共用;更大文件走分片上传)。
pub const MAX_BODY: usize = 256 * 1024 * 1024;

pub fn build(app: App) -> Router {
    use crate::web::dav as dav;
    use crate::web::handlers as h;
    Router::new()
        .route("/api/captcha", get(h::auth::captcha))
        .route("/api/auth/login", post(h::auth::login))
        .route("/api/auth/logout", post(h::auth::logout))
        .route("/api/auth/me", get(h::auth::me))
        .route("/api/auth/password", post(h::auth::change_password))
        .route("/api/admin/status", get(h::admin::status))
        .route("/api/admin/cache/clear", post(h::admin::clear_cache))
        .route("/api/admin/transcode-replace", post(h::admin::set_transcode_replace))
        .route("/api/admin/offline-settings", post(h::admin::set_offline_settings))
        .route("/dav", any(dav::dav_root))
        .route("/dav/", any(dav::dav_root))
        .route("/dav/{*path}", any(dav::dav_handler))
        .route("/api/offline/add", post(h::offline::add))
        .route("/api/offline/add-torrent", post(h::offline::add_torrent))
        .route("/api/offline/retry", post(h::offline::retry))
        .route("/api/offline/select", post(h::offline::select))
        .route("/api/offline/list", get(h::offline::list))
        .route("/api/offline/events", get(h::offline::events))
        .route("/api/offline/cancel", post(h::offline::cancel))
        .route("/api/offline/remove", post(h::offline::remove))
        .route("/api/users", get(h::users::list).post(h::users::create))
        .route("/api/users/{id}", patch(h::users::patch).delete(h::users::delete))
        .route("/api/shares", get(h::shares::list).post(h::shares::create))
        .route("/api/shares/{id}", patch(h::shares::patch).delete(h::shares::delete))
        .route("/api/shares/{id}/rules", get(h::shares::rules_list).post(h::shares::rule_add))
        .route("/api/shares/{id}/rules/{rid}", delete(h::shares::rule_delete))
        .route("/api/browse-host", get(h::shares::browse_host))
        .route("/api/browse/{share_id}", get(h::files::browse))
        .route("/api/album/{share_id}", get(h::files::album))
        .route("/api/file/meta/{share_id}", get(h::files::meta))
        .route("/api/file/detail/{share_id}", get(h::files::detail))
        .route("/api/file/text/{share_id}", get(h::files::text_get).post(h::files::text_put))
        .route("/api/file/binary/{share_id}", post(h::files::binary_put))
        .route("/api/mkdir", post(h::files::mkdir))
        .route("/api/rename", post(h::files::rename))
        .route("/api/move", post(h::files::move_items))
        .route("/api/delete", post(h::files::delete))
        .route("/api/zip", get(h::files::zip_get).post(h::files::zip_post))
        .route("/api/file/dlticket", post(h::files::dlticket))
        .route("/api/dl/{share_id}/{*name}", get(h::files::dl_signed))
        .route("/dl/{*path}", get(h::files::dl_short))
        .route("/api/upload/init", post(h::uploads::init))
        .route("/api/upload/direct", post(h::uploads::direct_upload))
        .route("/api/uploads", get(h::uploads::list_mine))
        .route("/api/upload/parts/{token}", get(h::uploads::parts))
        .route("/api/upload/part/{token}/{part}", put(h::uploads::part))
        .route("/api/upload/complete/{token}", post(h::uploads::complete))
        .route("/api/upload/{token}", delete(h::uploads::abort))
        .route("/api/media/{share_id}", get(h::media::get).head(h::media::head))
        .route("/api/media/status/{share_id}", get(h::media::status))
        .route("/api/media/thumb/{share_id}", get(h::media::video_thumb))
        .route("/api/media/imgthumb/{share_id}", get(h::media::image_thumb))
        .route("/api/fileshares", get(h::links::list).post(h::links::create))
        .route("/api/fileshares/{token}", delete(h::links::delete))
        .route("/s/{token}", get(h::links::page))
        .route("/s/{token}/api/download", get(h::links::download))
        .route("/api/tls", get(h::tls::get).put(h::tls::put).delete(h::tls::delete))
        .route("/api/config", get(h::misc::config))
        .route("/", get(h::misc::index))
        .route("/static/{*path}", get(h::misc::static_file))
        .fallback(h::misc::fallback)
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(TraceLayer::new_for_http())
        .with_state(app)
}
