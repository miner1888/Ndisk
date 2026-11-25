//! 任务中心:取消标志、进度采样器、数据库更新与 SSE 广播的统一出口。
//! 实例由 `app::App` 持有,全项目只有这一个,不再是全局 OnceCell。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::json;
use tokio::sync::{broadcast, Mutex};

use crate::app::App;
use crate::engine::events::{Meter, TaskEvent};

pub const OFFLINE_SIZE_LIMIT_KEY: &str = "offline_size_limit";
pub const DEFAULT_SIZE_LIMIT: i64 = 20 * 1024 * 1024 * 1024;

/// 一个任务的运行期登记:取消标志 + 速度采样器。
/// `done` 表示任务已取消 / 已删除,worker 借此感知"不要再写回"。
struct Handle {
    flag: Arc<AtomicBool>,
    done: bool,
    meter: Meter,
}

#[derive(Clone)]
pub struct Hub {
    inner: Arc<HubInner>,
}

struct HubInner {
    tasks: Mutex<HashMap<i64, Handle>>,
    bus: broadcast::Sender<TaskEvent>,
}

impl Hub {
    pub fn new() -> Self {
        let (bus, _) = broadcast::channel(256);
        Self { inner: Arc::new(HubInner { tasks: Mutex::new(HashMap::new()), bus }) }
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.inner.bus.subscribe()
    }

    /// worker 启动时登记,拿到取消标志。
    pub async fn register(&self, task_id: i64) -> Arc<AtomicBool> {
        let mut tasks = self.inner.tasks.lock().await;
        let h = tasks.entry(task_id).or_insert_with(|| Handle {
            flag: Arc::new(AtomicBool::new(false)),
            done: false,
            meter: Meter::default(),
        });
        h.done = false;
        h.meter = Meter::default();
        Arc::clone(&h.flag)
    }

    pub async fn cancel(&self, task_id: i64) {
        if let Some(h) = self.inner.tasks.lock().await.get_mut(&task_id) {
            h.flag.store(true, Ordering::Relaxed);
            h.done = true;
        }
    }

    pub async fn is_cancelled(&self, task_id: i64) -> bool {
        self.inner
            .tasks
            .lock()
            .await
            .get(&task_id)
            .is_some_and(|h| h.done || h.flag.load(Ordering::Relaxed))
    }

    /// 任务终结后回收登记。
    pub async fn finish(&self, task_id: i64) {
        self.inner.tasks.lock().await.remove(&task_id);
    }

    /// 局部更新任务并广播 SSE。
    pub async fn update(&self, app: &App, user_id: i64, task_id: i64, patch: serde_json::Value) {
        let _ = app.db.update_offline_task(task_id, patch.clone()).await;
        let _ = self.inner.bus.send(TaskEvent::Update { user_id, id: task_id, patch });
    }

    /// 进度更新:经采样器算速度 / ETA 后落库 + 广播。
    /// 登记已被回收(终态竞态)时降级为无速度广播,不重新插入条目。
    pub async fn progress(&self, app: &App, user_id: i64, task_id: i64, downloaded: i64, size: i64) {
        let sampled = {
            let mut tasks = self.inner.tasks.lock().await;
            tasks.get_mut(&task_id).map(|h| h.meter.sample(downloaded, size))
        };
        let patch = match sampled {
            Some((speed, eta)) if eta >= 0 && speed > 0 => {
                json!({"downloaded": downloaded, "size": size, "speed": speed, "eta": eta})
            }
            _ => json!({"downloaded": downloaded, "size": size}),
        };
        self.update(app, user_id, task_id, patch).await;
    }

    /// 置为 completed(终态),附带最终字段。
    pub async fn set_done(&self, app: &App, user_id: i64, task_id: i64, mut patch: serde_json::Value) {
        patch["state"] = serde_json::json!("completed");
        let _ = app.db.update_offline_task(task_id, patch.clone()).await;
        let _ = self.inner.bus.send(TaskEvent::Update { user_id, id: task_id, patch });
        self.finish(task_id).await;
    }

    /// 任务被移除时广播。
    pub async fn removed(&self, user_id: i64, task_id: i64) {
        let _ = self.inner.bus.send(TaskEvent::Removed { user_id, id: task_id });
    }
}

/// 单任务大小上限(settings 表,默认 20GB)。
pub async fn size_limit(app: &App) -> i64 {
    app.db
        .setting(OFFLINE_SIZE_LIMIT_KEY)
        .await
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_SIZE_LIMIT)
}

/// 统一的终态 / 中间态写入入口;到达终态时回收任务登记。
pub async fn set_state(app: &App, user_id: i64, task_id: i64, state: &str, error: Option<String>) {
    let mut patch = json!({ "state": state });
    if let Some(e) = error {
        patch["error"] = json!(e);
    }
    app.hub.update(app, user_id, task_id, patch).await;
    if state == "failed" || state == "cancelled" || state == "completed" {
        app.hub.finish(task_id).await;
    }
}
