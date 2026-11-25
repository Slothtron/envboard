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

use crate::model::{self, Env, Health};
use crate::prefs::{self, ThemeChoice};
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
    /// 证书页的数据（`None` = 还没读到）。
    pub ca: Signal<Option<model::Ca>>,
    /// CA 读不到（没物化 / 探测失败）：页面显示「不可用」与「重试」，不排一张空表。
    pub ca_missing: Signal<bool>,
    /// 设置框开着没有。
    pub settings: Signal<bool>,
    /// 待确认的破坏性动作（`None` = 没有确认框）。
    pub confirm: Signal<Option<Confirm>>,
    /// 当前主题三档：标题栏的标签与设置框的三档共用这一个值（V-1）。
    pub theme: Signal<ThemeChoice>,
    /// 抓包子页正在看哪个环境（`None` = 在环境列表上）。
    pub capture_env: Signal<Option<String>>,
    /// 关窗后常驻托盘（`ui.json` 的 `close_to_tray`，默认开）。
    pub close_to_tray: Signal<bool>,
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
    match service::create(&envboard_protocol::EnvCreateReq {
        name: next,
        ..Default::default()
    }) {
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

/// 只说一下、不碰错误槽的提示（例如“主题切了但没写盘”这种不影响结果的边角）。
fn note(cx: Ctx, text: String) {
    cx.toast.set(Some(text.clone()));
    auto_dismiss(cx, text);
}

// ---- 主题 ------------------------------------------------------------------

/// 切主题：**先改外观再落盘**。外观是即时的（`set_color_scheme` 让整窗重解令牌），
/// 落盘失败只提示不回滚 —— 看着对了比记着对了重要，下次启动再按文件走。
pub fn set_theme(cx: Ctx, choice: ThemeChoice) {
    cx.theme.set(choice);
    set_color_scheme(choice.appearance());
    if let Some(path) = service::ui_prefs_file()
        && let Err(error) = prefs::set_theme(path, choice)
    {
        note(cx, format!("主题已切换，但没写进 ui.json：{error}"));
    }
}

/// 「关闭窗口时退出程序」开关（方案 §6.1 第 6 条）。写进 `ui.json` 的
/// `close_to_tray`，**立刻**生效：关窗处理当场读这个信号，不等重启。
pub fn set_close_to_tray(cx: Ctx, on: bool) {
    cx.close_to_tray.set(on);
    if let Some(path) = service::ui_prefs_file()
        && let Err(error) = prefs::set_close_to_tray(path, on)
    {
        note(cx, format!("开关已生效，但没写进 ui.json：{error}"));
    }
}

// ---- 证书页 ----------------------------------------------------------------

/// 读一屏证书数据。**放到工作线程**：`trust()` 是 Win32 证书存储探测，
/// 压在渲染线程上会让切到证书页的那一帧掉速。
///
/// 读失败与“没有 CA”都只置 `ca_missing`，**不写错误槽** —— 证书页自己有一条
/// 「不可用 + 重试」的提示位，比在表格上方弹一条红条更贴切（那里说的事是
/// “这一页没数据”，不是“你刚才点的操作失败了”）。
pub fn refresh_ca(cx: Ctx) {
    if !service::ca_available() {
        cx.ca.set(None);
        cx.ca_missing.set(true);
        return;
    }
    spawn(async move {
        match background(async { service::ca_snapshot() }).await {
            Ok(Ok(ca)) => {
                cx.ca_missing.set(false);
                cx.ca.set(Some(ca));
            }
            _ => {
                cx.ca.set(None);
                cx.ca_missing.set(true);
            }
        }
    });
}

/// 安装根证书（当前用户 ROOT 存储，免管理员、幂等）。
///
/// 写根存储会弹 Windows 的「安全警告」（框里带指纹，要求人点「是」）：
/// 那个框开着的时候这一次调用是**阻塞**的，页面上必须先把话说清楚，
/// 否则看上去就是“点了没反应”——M4 验收时正是这样被误判成按钮没接上。
pub fn install_ca(cx: Ctx) {
    note(
        cx,
        "正在写入当前用户信任库；若弹出系统「安全警告」，请核对指纹后点「是」".to_owned(),
    );
    spawn(async move {
        match service::ca_install().await {
            Ok(Ok(())) => {
                refresh_ca(cx);
                succeed(cx, "根证书已安装到当前用户信任库");
            }
            Ok(Err(error)) => {
                fail(cx, error);
            }
            Err(_) => {
                cx.error.set(Some("安装失败：回传通道已断开".to_owned()));
            }
        }
    });
}

/// 导出 HAR：拉全量记录 + 拼 JSON + 写下载目录，几百 MB 级，全在普通线程上跑。
///
/// 任务挂**无主** owner：导出途中按「← 环境」返回时页面会被拆掉，
/// 挂在页面 owner 上的任务还没被首次轮询就没了（M4 卸载证书踩过同一个坑），
/// 而用户点下导出后期待的是“文件一定会出现”。只写信号、不建元素，挂出去是安全的。
pub fn export_har(cx: Ctx, name: String) {
    note(cx, format!("正在导出 {name} 的 HAR…"));
    Owner::detached().run(|| {
        spawn(async move {
            match service::export_har(name).await {
                Ok(Ok(export)) => {
                    let file = export
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| export.path.display().to_string());
                    succeed(cx, format!("HAR 已导出：{file}（{} 条）", export.entries));
                }
                Ok(Err(error)) => {
                    fail(cx, error);
                }
                Err(_) => {
                    cx.error.set(Some("导出失败：回传通道已断开".to_owned()));
                }
            }
        });
    });
}

/// 卸载根证书：**不直接做**，先弹确认（可逆，所以确认框不印“不可撤销”、
/// 确认键也不是胭脂）。
pub fn ask_uninstall_ca(cx: Ctx) {
    cx.confirm.set(Some(Confirm::uninstall_ca()));
}

fn uninstall_ca(cx: Ctx) {
    note(
        cx,
        "正在从信任库移除；若弹出系统确认，请点「是」".to_owned(),
    );
    // 卸载是从**确认框**里发起的，而 `run_confirm` 会先把 `confirm` 清空——确认框这个
    // owner 同一帧就被拆掉。`spawn` 的任务归“当前 owner”所有，于是它在被首次轮询之前
    // 就跟着框一起没了：现象是提示一直挂着、存储一动不动、连报错都不会有。
    // 所以这里把任务挂到**无主** owner 上，让它独立跑完（只写信号，不建元素）。
    Owner::detached().run(|| {
        spawn(async move {
            match service::ca_uninstall().await {
                Ok(Ok(result)) => {
                    refresh_ca(cx);
                    let head = if result.removed {
                        "已从当前用户信任库卸载根证书；被代理的 HTTPS 流量将报不受信任"
                    } else {
                        "当前用户信任库里没有这张证书，无需卸载"
                    };
                    let line = match result.warning {
                        Some(warning) => format!("{head}（{warning}）"),
                        None => head.to_owned(),
                    };
                    succeed(cx, line);
                }
                Ok(Err(error)) => {
                    fail(cx, error);
                }
                Err(_) => {
                    cx.error.set(Some("卸载失败：回传通道已断开".to_owned()));
                }
            }
        })
    });
}

// ---- 确认框 ----------------------------------------------------------------
//
// 破坏性动作**只有一处需要确认**：删除环境（不可逆）。其余“看着危险”的动作
// （停止、换个端口、卸载证书）都可逆，直接做、给一句轻提示就够了 —— 确认框一多，
// 人就闭眼点“确定”，真正该拦的那次也拦不住。

/// 要确认的动作。
#[derive(Clone, PartialEq, Debug)]
pub enum Ask {
    DeleteEnv(String),
    UninstallCa,
    ClearCapture(String),
}

/// 一屏确认框的内容。文案定在这里而不是页面里：同一件事只写一遍。
#[derive(Clone, PartialEq, Debug)]
pub struct Confirm {
    pub ask: Ask,
    pub title: String,
    pub note: String,
    pub body: String,
    /// 不可逆才印“此操作不可撤销”，并用胭脂确认键。
    pub irreversible: bool,
    pub confirm_label: String,
}

impl Confirm {
    pub fn delete_env(name: &str) -> Self {
        Self {
            ask: Ask::DeleteEnv(name.to_owned()),
            title: format!("删除环境 {name}？"),
            note: "不可撤销".to_owned(),
            // 文案对齐实现：`Manager::remove` 对「desired=running」或「还有活实例」的环境
            // 直接报 Conflict，**不会**代你停掉（停环境有账本副作用，不该由一次删除顺手做）。
            body: format!(
                "环境 {name} 的配置与规则绑定会一并从账本里移除。\
                 正在运行的环境不能直接删除：请先在抽屉里「停止」，再来删除。"
            ),
            irreversible: true,
            confirm_label: "删除".to_owned(),
        }
    }

    pub fn uninstall_ca() -> Self {
        Self {
            ask: Ask::UninstallCa,
            title: "从当前用户信任库卸载根证书？".to_owned(),
            note: "可逆".to_owned(),
            body: "卸载后，本机经 envboard 代理的 HTTPS 流量会报证书不受信任；\
                   随时可以再点「安装证书」装回来。"
                .to_owned(),
            irreversible: false,
            confirm_label: "卸载".to_owned(),
        }
    }

    /// 清空抓包缓冲：记录只在这块内存里，清掉就真没了 —— 不可逆。
    pub fn clear_capture(name: &str, count: usize) -> Self {
        Self {
            ask: Ask::ClearCapture(name.to_owned()),
            title: format!("清空 {name} 的抓包记录？"),
            note: "不可撤销".to_owned(),
            body: format!(
                "当前缓存的 {count} 条记录会从内存里移除；正在进行的抓包会话不受影响，\
                 之后的新请求仍会记录。"
            ),
            irreversible: true,
            confirm_label: "清空".to_owned(),
        }
    }
}

/// 弹确认框（删除环境走这里，不直接调 `remove`）。
pub fn ask(cx: Ctx, confirm: Confirm) {
    cx.confirm.set(Some(confirm));
}

pub fn cancel_confirm(cx: Ctx) {
    cx.confirm.set(None);
}

/// 确认键：先关框再执行 —— 动作里可能又失败，错误条要落在干净的一屏上。
pub fn run_confirm(cx: Ctx) {
    let Some(confirm) = cx.confirm.get() else {
        return;
    };
    cx.confirm.set(None);
    match confirm.ask {
        Ask::DeleteEnv(name) => remove(cx, name),
        Ask::UninstallCa => uninstall_ca(cx),
        // 清空之后本地的行由抓包页在下一拍发现代次变化时自己清掉：
        // 页面手里的行是页面自己的状态，`actions` 不越界去改它。
        Ask::ClearCapture(name) => match service::clear_capture(&name) {
            Ok(_) => succeed(cx, "抓包会话已清空"),
            Err(error) => {
                fail(cx, error);
            }
        },
    }
}
