//! 域：调试与抓包 —— 会话视图、调试单例与 HAR 导入会话。
//!
//! 收什么：抓包读取（`captures` / `capture_detail` / `clear_capture`）、
//! 工作区级调试会话单例（`start_debug` / `stop_debug` / `debug_view` /
//! `debug_delta`，切换即换代：停旧、清旧、开新）、HAR 导入会话库
//! （`har_import` / `har_list` / `har_get` / `har_delete`）与故障注入
//! 转发（`inject_fault`，live 断言面）。
//!
//! capture 字段本身是环境的 hot 字段：开关经 `update` 落账本并热应用，
//! 本域只是它的两个驱动方之一（另一个是环境页的开关）。

use envboard_engine::{
    CaptureDelta as EngineCaptureDelta, CaptureView as EngineCaptureView, Error, ErrorCode,
};
use envboard_protocol::DebugView;
use serde_json::{Value, json};

use envboard_engine::domain::Environment;

use crate::manager::Manager;
use envboard_events::ControlEvent;

impl Manager {
    /// 故障注入转发（live 断言面）：核心不支持或环境没在跑 → false，不假装。
    pub fn inject_fault(&self, name: &str, reason: &str) -> bool {
        self.engine.inject_failed(name, reason)
    }

    /// 抓包会话视图（易失；会话 = 实例生命周期）。
    pub fn captures(&self, name: &str, limit: usize) -> Result<Option<EngineCaptureView>, Error> {
        self.require(name)?;
        Ok(self.engine.capture_view(name, limit))
    }

    /// 单条抓包详情（全缓冲查找；被淘汰/不存在 → None）。
    pub fn capture_detail(&self, name: &str, request_id: u64) -> Result<Option<Value>, Error> {
        self.require(name)?;
        Ok(self.engine.capture_get(name, request_id))
    }

    /// 手动清空抓包会话（generation +1，会话延续）。
    pub fn clear_capture(&self, name: &str) -> Result<bool, Error> {
        self.require(name)?;
        Ok(self.engine.clear_capture(name))
    }

    // ---- 调试会话（工作区级单例）：切换即换代 —— 停旧、清旧、开新 ---- //

    /// 把某环境的 capture 字段设为给定值（热字段；经 commit 走账本与审计事件）。
    fn set_capture_field(&self, name: &str, enabled: bool) -> Result<(), Error> {
        let raw = self.require(name)?;
        let environment = Environment::from_json(&raw)?;
        if environment.capture() == enabled {
            return Ok(());
        }
        let patch = json!({ "capture": enabled });
        self.update(name, &patch)?;
        Ok(())
    }

    /// 开启/切换调试会话。原目标环境的抓包**停止并清空**。
    pub fn start_debug(&self, env: &str) -> Result<DebugView, Error> {
        self.require(env)?;
        if !self.is_live(env) {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("environment {env:?} is not running; start it before debugging"),
            ));
        }
        let previous = self.debug_target.lock().unwrap().clone();
        if let Some(old) = previous.as_ref().filter(|old| *old != env) {
            let _ = self.engine.clear_capture(old);
            let _ = self.set_capture_field(old, false);
        }
        self.set_capture_field(env, true)?;
        *self.debug_target.lock().unwrap() = Some(env.to_string());
        self.events.emit(
            self.clock.now_unix_ms(),
            ControlEvent::Custom {
                kind: "debug/switched".to_string(),
                payload: json!({ "env": env, "previous": previous }),
            },
        );
        self.debug_view().ok_or_else(|| {
            Error::new(
                ErrorCode::InternalError,
                "debug session vanished immediately".to_string(),
            )
        })
    }

    /// 停止调试会话：停记录 + 清空记录（不留痕迹）。
    pub fn stop_debug(&self) -> Result<bool, Error> {
        let Some(target) = self.debug_target.lock().unwrap().clone() else {
            return Ok(false);
        };
        let _ = self.engine.clear_capture(&target);
        let _ = self.set_capture_field(&target, false);
        *self.debug_target.lock().unwrap() = None;
        self.events.emit(
            self.clock.now_unix_ms(),
            ControlEvent::Custom {
                kind: "debug/stopped".to_string(),
                payload: json!({ "env": target }),
            },
        );
        Ok(true)
    }

    /// 当前调试会话视图（目标环境不存在/没在跑 → None）。
    pub fn debug_view(&self) -> Option<DebugView> {
        let target = self.debug_target.lock().unwrap().clone()?;
        let capture = self.engine.capture_view(&target, 500)?;
        Some(DebugView {
            env: target,
            capture,
        })
    }

    /// 调试实时流的轮询原语：目标 + 游标之后的增量记录。
    /// None = 没有调试会话（未设目标，或目标实例不在跑）。
    pub fn debug_delta(&self, after: u64, limit: usize) -> Option<(String, EngineCaptureDelta)> {
        let target = self.debug_target.lock().unwrap().clone()?;
        let delta = self.engine.capture_delta(&target, after, limit)?;
        Some((target, delta))
    }

    // ---- 导入会话（HAR；只读、多会话并存） ---- //

    pub fn har_import(&self, name: &str, body: &Value) -> Result<Value, Error> {
        let imported = crate::har::parse_session(self.har_library.next_id(), name, body)?;
        self.har_library.admit(imported)
    }

    pub fn har_list(&self) -> Vec<Value> {
        self.har_library.list()
    }

    pub fn har_get(&self, id: u64, limit: usize) -> Option<Value> {
        self.har_library.get(id, limit)
    }

    pub fn har_delete(&self, id: u64) -> bool {
        self.har_library.delete(id)
    }
}
