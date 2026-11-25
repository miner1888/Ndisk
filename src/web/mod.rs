//! HTTP 编排层:路由表 + 各领域 handler + WebDAV + 请求上下文。
//! handler 只做 参数解析 → 权限检查 → 调 domain/db → 组装响应。

pub mod assets;
pub mod ctx;
pub mod dav;
pub mod handlers;
pub mod identity;
pub mod router;

pub use ctx::{can_manage, child_rel, ensure_manageable, file_name_of, locate, locate_readable, locate_writable, parse_range, stream_file, Spot};
