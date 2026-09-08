//! 运行状态（前端徽标与托盘联动的事件源）。

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload {
    /// checking | unconfigured | needs-sms | logging-in | connected | network-down | failed
    pub kind: String,
    pub text: String,
}

#[derive(Default)]
struct Inner {
    last: Option<StatusPayload>,
}

#[derive(Default)]
pub struct RuntimeState(Mutex<Inner>);

pub const STATUS_EVENT: &str = "yulink://status";

impl RuntimeState {
    /// 更新状态；与上次相同则跳过，避免重复广播。
    pub fn set(&self, app: &AppHandle, kind: &str, text: String) {
        let payload = StatusPayload {
            kind: kind.to_string(),
            text,
        };
        let changed = {
            let Ok(mut inner) = self.0.lock() else {
                return;
            };
            let same = inner
                .last
                .as_ref()
                .map(|p| p.kind == payload.kind && p.text == payload.text)
                .unwrap_or(false);
            if !same {
                inner.last = Some(payload.clone());
            }
            !same
        };
        if changed {
            log::info!("状态: {} - {}", payload.kind, payload.text);
            let _ = app.emit(STATUS_EVENT, &payload);
        }
    }

    pub fn current(&self) -> Option<StatusPayload> {
        self.0.lock().ok().and_then(|inner| inner.last.clone())
    }
}
