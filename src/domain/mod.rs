//! 纯业务逻辑层:不依赖 axum,不持有全局状态。
//!
//! - `pathguard`  路径规范化 / 文件名校验 / 防穿越拼接
//! - `auth`       口令哈希、随机 token、凭据校验
//! - `access`     guest/login/admin 三级权限模型
//! - `ticket`     HMAC 下载票据(直链 / 打包)
//! - `zipx`       流式 ZIP 打包(store,恒定内存)
//! - `chunkup`    分片上传核心(分片数学、磁盘空间、合并落盘)
//! - `media/`     类型判定、ffmpeg 封装、缩略图、转码、缓存治理
//! - `fsx`        文件系统原语(原子写、递归统计、收集)
//! - `cached`     带磁盘缓存与 TTL 的远程资源订阅
//! - `fmt`        人类可读字节格式化(全项目唯一实现)

pub mod access;
pub mod auth;
pub mod cached;
pub mod chunkup;
pub mod fmt;
pub mod fsx;
pub mod media;
pub mod pathguard;
pub mod ticket;
pub mod zipx;

pub use fmt::human_bytes;

/// 当前 UNIX 时间(秒)。时钟回拨时退化为 0,不会 panic。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
