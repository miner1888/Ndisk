//! 离线下载引擎:HTTP 直链 / 磁力·BT / 迅雷 / eD2K。
//!
//! - `hub`:任务中心 —— 取消标志、进度采样、状态落库 + SSE 广播(唯一出口)
//! - `classify`:链接解析、协议归类、内网地址校验、去重命名
//! - `direct`:HTTP(S) 直链(多段并行 + 单连接回退)
//! - `torrent`:磁力 / BT(librqbit,两阶段:解析 → 勾选 → 下载)
//! - `ed2k`:eD2K(协议、服务器 / KAD 查源、块级并行、MD4 校验)
//!
//! 引擎不依赖 web 层;由 web 的 handler 创建任务并 spawn 到这里。

pub mod classify;
pub mod direct;
pub mod ed2k;
pub mod events;
pub mod hub;
pub mod torrent;
pub mod trackers;

pub use classify::{normalize_url, unique_name, validate_url};
pub use hub::{set_state, size_limit, Hub};
pub use torrent::{parse_torrent_upload, run_torrent_task};
pub use direct::run_direct_task;

pub use ed2k::{concurrency, default_servers, ED2K_CONCURRENCY_KEY, ED2K_SERVERS_KEY};

use std::future::Future;

use futures_util::FutureExt;

use crate::app::App;

/// 后台任务的 panic 兜底原语:返回 true 表示该任务 panic 了。
/// 独立成函数便于契约测试(见 tests),spawn_task 只是它的 App 绑定。
pub async fn guard<F>(fut: F) -> bool
where
    F: Future<Output = ()>,
{
    std::panic::AssertUnwindSafe(fut).catch_unwind().await.is_err()
}

/// 后台任务受控启动:worker 内部 panic 时把任务标记为 failed 并记录日志,
/// 进程继续服务(panic 不再让任务永远停在 running)。
///
/// 这是"任务层"的兜底,与 release 的 `panic = "unwind"`(请求/任务级隔离)配套;
/// 若改成 abort,任何 panic 都会直接终止进程,这里的兜底也就失效了。
pub fn spawn_task<F>(app: App, user_id: i64, task_id: i64, what: &'static str, fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        if guard(fut).await {
            tracing::error!("[{what}] 任务 {task_id} 内部异常(panic),已标记失败");
            set_state(&app, user_id, task_id, "failed", Some(format!("{what}任务内部异常,已中止"))).await;
        }
    });
}

/// 外部来源(HTTP 响应头 / 链接字段)的**单段文件名**落盘前的统一消毒口:
/// 非法名回退为 `download-<id>`,保证不逃逸下载目录、不含非法字符。
pub fn safe_name(raw: &str, task_id: i64) -> String {
    crate::domain::pathguard::check_name(raw).unwrap_or_else(|_| format!("download-{task_id}"))
}

/// 外部来源的**多段相对路径**(种子内文件树)逐段消毒:
/// 任一段非法即拒绝整个文件,绝不逃逸临时 / 目标目录。
pub fn safe_rel_path(raw: &str) -> Option<String> {
    let mut out = Vec::new();
    for seg in raw.split('/') {
        out.push(crate::domain::pathguard::check_name(seg).ok()?);
    }
    Some(out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约:worker panic 必须被兜底捕获(否则任务永远停在 running),
    /// 正常结束则不应误报。
    #[tokio::test]
    async fn supervisor_catches_worker_panic() {
        assert!(guard(async { panic!("模拟 worker 内部 panic") }).await);
        assert!(!guard(async {}).await);
    }
}
