//! 界面能做的写操作，以及**错误的三层落点**。
//!
//! 三层不是三种样式，而是三种**距离**：
//!
//! - **行内**（`Ctx::error` → 表格上方的告警条）：用户刚点的那一下失败了，
//!   条上写清哪一步、为什么，带「关闭」；它留在原地，直到用户自己消掉或下一次成功；
//! - **状态条**（`Ctx::error` 同时进 `status_bar` 中段）：一次都没点也能看见
//!   “宿主连不上”这类持续故障，不会被抽屉或滚动藏起来；
//! - **轻提示**（`Ctx::toast`）：成功路径的确认，3.2 秒自己走，不占版面。
//!
//! 写操作一律「先算结论、再刷一次列表」：不依赖轮询的下一拍，点完立刻看到结果。

use std::time::Duration;

use envboard_engine::Error;
use nsh_core::prelude::*;

use crate::model::{Env, Health};
use crate::service;

/// 界面共享的那几个信号。字段全是 `Copy` 的信号句柄，所以 `Ctx` 本身是 `Copy`，
/// 可以随点随传，不需要 `Rc<RefCell<..>>` 那一套。
#[derive(Clone, Copy)]
pub struct Ctx {
    pub envs: Signal<Vec<Env>>,
    pub error: Signal<Option<String>>,
    /// 错误条的标题：写操作失败、读快照失败、宿主没起来是三种不同的事，
    /// 标题说清是哪一种，正文（`code · message`）才有落点。
    pub error_title: Signal<&'static str>,
    pub toast: Signal<Option<String>>,
    pub selected: Signal<Option<String>>,
    pub drawer: Signal<bool>,
}

/// 轻提示自动消失：每次显示都起一个定时器，到点只清掉**自己那次**写进去的值
/// （期间用户又触发一条提示，就不该被前一条的定时器抹掉）。
fn auto_dismiss(cx: Ctx, text: String) {
    spawn(async move {
        timer(Duration::from_millis(3200)).await;
        if cx.toast.get().as_deref() == Some(text.as_str()) {
            cx.toast.set(None);
        }
    });
}

pub fn succeed(cx: Ctx, text: impl Into<String>) {
    cx.error.set(None);
    let text = text.into();
    cx.toast.set(Some(text.clone()));
    auto_dismiss(cx, text);
}

/// 一个错误只在这一处翻译成两层文案：行内条要短（`code · message`），
/// 状态条要更短（message），轻提示不再重复——三层里只响两层，避免同一件事说三遍。
pub fn fail(cx: Ctx, error: Error) -> String {
    let line = format!("{} · {}", error.code.as_str(), error.message);
    cx.error_title.set("操作未生效");
    cx.error.set(Some(line.clone()));
    cx.toast.set(Some(line.clone()));
    auto_dismiss(cx, line.clone());
    line
}

/// 读一次列表。同步（管理器内存态），在本进程里就是一次借用，不阻塞引擎线程。
pub fn refresh(cx: Ctx) {
    match service::list() {
        Ok(views) => cx.envs.set(views.into_iter().map(Env::from_view).collect()),
        Err(error) => {
            fail(cx, error);
        }
    }
}

/// 行尾动作：在跑的停止、停着的启动、其余打开抽屉看原因（不在列表里解释故障）。
///
/// `service::stop` 返回的是 oneshot：外层是“回传通道是否还在”，内层才是门面的
/// 结果。两层都要落进错误槽，否则线程一挂，界面会静默地什么都不发生。
pub fn act(cx: Ctx, name: String, health: Health) {
    match health {
        Health::Running => {
            let name2 = name.clone();
            spawn(async move {
                match service::stop(name).await {
                    Ok(Ok(_)) => {
                        refresh(cx);
                        succeed(cx, format!("{name2} 已停止"));
                    }
                    Ok(Err(error)) => {
                        fail(cx, error);
                    }
                    Err(_) => {
                        cx.error.set(Some("停止失败：回传通道已断开".to_owned()));
                    }
                }
            });
        }
        Health::Stopped => {
            let name2 = name.clone();
            spawn(async move {
                match service::start(name).await {
                    Ok(Ok(_)) => {
                        refresh(cx);
                        succeed(cx, format!("{name2} 已启动"));
                    }
                    Ok(Err(error)) => {
                        fail(cx, error);
                    }
                    Err(_) => {
                        cx.error.set(Some("启动失败：回传通道已断开".to_owned()));
                    }
                }
            });
        }
        _ => open_drawer(cx, name),
    }
}

pub fn open_drawer(cx: Ctx, name: String) {
    cx.selected.set(Some(name));
    cx.drawer.set(true);
}

/// 新建：名字按现有数量顺延，端口交给管理器在区间里随机分（界面不编端口）。
/// 新建的环境是**停止态**：让用户先看一眼配置再启动，不做“建完就在跑”的惊喜。
pub fn create(cx: Ctx) {
    let next = {
        let envs = cx.envs.get();
        let mut n = envs.len() + 1;
        let existing: Vec<String> = envs.iter().map(|e| e.name.clone()).collect();
        while existing.iter().any(|name| name == &format!("env-{n:02}")) {
            n += 1;
        }
        format!("env-{n:02}")
    };
    match service::create(serde_json::json!({ "name": next })) {
        Ok(_) => {
            refresh(cx);
            succeed(cx, "已新建环境");
        }
        Err(error) => {
            fail(cx, error);
        }
    }
}

pub fn remove(cx: Ctx, name: String) {
    match service::remove(&name) {
        Ok(()) => {
            cx.drawer.set(false);
            cx.selected.set(None);
            refresh(cx);
            succeed(cx, format!("{name} 已删除"));
        }
        Err(error) => {
            fail(cx, error);
        }
    }
}

pub fn reallocate_port(cx: Ctx, name: String) {
    match service::reallocate_port(&name) {
        Ok(_) => {
            refresh(cx);
            succeed(cx, "已换一个空闲端口");
        }
        Err(error) => {
            fail(cx, error);
        }
    }
}

/// 复制代理地址（"复制端口"沿用抽屉按钮的位置，但复制的是**可粘进终端**的一整行）。
/// NSH 没有单独的 `set_clipboard_text`；文字走 `ClipboardItem { text, html: None }`。
pub fn copy_proxy(cx: Ctx, command: String) {
    let mut item = ClipboardItem::default();
    item.text = command;
    set_clipboard_item(item);
    succeed(cx, "代理地址已复制");
}

/// 刷新按钮的失败也要落进错误槽：`refresh()` 自己会写，这里只补一个成功的轻提示。
pub fn refresh_now(cx: Ctx) {
    refresh(cx);
    if cx.error.get().is_none() {
        succeed(cx, "已刷新");
    }
}
