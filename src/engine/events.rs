//! 离线任务 SSE 事件 + 指数滑动平均速度采样器。

use serde_json::json;

#[derive(Clone, Debug)]
pub enum TaskEvent {
    Update {
        user_id: i64,
        id: i64,
        patch: serde_json::Value,
    },
    Removed {
        user_id: i64,
        id: i64,
    },
}

impl TaskEvent {
    pub fn user_id(&self) -> i64 {
        match self {
            TaskEvent::Update { user_id, .. } | TaskEvent::Removed { user_id, .. } => *user_id,
        }
    }

    pub fn to_json(&self) -> String {
        match self {
            TaskEvent::Update { user_id, id, patch } => json!({
                "type": "update", "userId": user_id, "id": id, "patch": patch
            })
            .to_string(),
            TaskEvent::Removed { user_id, id } => json!({
                "type": "removed", "userId": user_id, "id": id
            })
            .to_string(),
        }
    }
}

/// 速度采样:指数滑动平均(0.6 旧 + 0.4 新),并按需推算剩余时间。
#[derive(Default)]
pub struct Meter {
    last_t: Option<std::time::Instant>,
    last_d: i64,
    pub speed: f64,
}

impl Meter {
    /// 返回 `(速度 B/s, 剩余秒数)`;无有效速度时 ETA 为 -1。
    pub fn sample(&mut self, downloaded: i64, size: i64) -> (i64, i64) {
        let now = std::time::Instant::now();
        if let Some(t) = self.last_t {
            let dt = now.duration_since(t).as_secs_f64();
            if dt >= 0.5 {
                let inst = (downloaded - self.last_d).max(0) as f64 / dt;
                self.speed = if self.speed > 0.0 { self.speed * 0.6 + inst * 0.4 } else { inst };
                self.last_t = Some(now);
                self.last_d = downloaded;
            }
        } else {
            self.last_t = Some(now);
            self.last_d = downloaded;
        }
        let speed = self.speed as i64;
        let eta = if self.speed > 1.0 && size > downloaded {
            ((size - downloaded) as f64 / self.speed) as i64
        } else {
            -1
        };
        (speed, eta)
    }
}
