//! CA 屏：共享根证书的**看**、**换**与**按域名签发**。
//!
//! 主线：*确认这是哪一张证书 → 看它装没装到系统 → 需要时换一张或给某个域名
//! 签一张*。屏面按这个顺序排：**证书摘要** → **系统信任** → **证书文件** →
//! **按域名签发** → **重新签发** → **如何使用**。
//!
//! 四条纪律：
//!
//! * **CA 私钥不在这里**，也不在任何交付给界面的结构里：摘要与 PEM 都来自
//!   `CaControl`。**按域名签发**是唯一会把一把叶子私钥交给界面的动作 —— 那是
//!   用户显式要的产物，只进剪贴板与内存，不落盘；
//! * **摘要是读来的，不是算的**：字段全部来自引擎物化的 CA，界面不做证书解析；
//! * **信任状态只读**：探测回答"当前用户 / 本机的 ROOT 存储里有没有这张证书"，
//!   安装与卸载是用户的动作，本工具不写系统存储；
//! * 这张 CA 是**工作区级单例**：所有环境共用，只有重新签发才需要重装证书。

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::form::Field;
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, div, px,
};

use envboard_host::{LeafPem, StorePresence};

use crate::host::Host;

/// CA 屏。
pub struct CaScreen {
    /// 最近一次「按域名签发」的产物。只活在内存里：留给用户核对与再次复制。
    issued: Option<LeafPem>,
}

impl CaScreen {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { issued: None }
    }

    /// 复制到剪贴板，并给一个**会自己消失**的成功提示。
    ///
    /// 原来是把"已复制"写在屏底一行常驻文字上：它永远不消失，下一次复制时
    /// 也说不清是上一次的反馈。轻提示（toast）才是这个语义的形状。
    fn copy(
        &mut self,
        text: String,
        note: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(Notification::success(note).id1::<Self>("ca-copy"), cx);
    }

    /// 证书摘要：`CaControl` 给出的字段逐条摆出来，一个都不自己算。
    fn render_summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = cx.global::<Host>();
        let muted = cx.theme().muted_foreground;
        let mono_family = cx.theme().mono_font_family.clone();
        let mono = |text: SharedString| {
            div()
                .font(gpui_kit::Font {
                    family: mono_family.clone(),
                    ..Default::default()
                })
                .child(text)
                .into_any_element()
        };

        let summary: AnyElement = match host.ca().and_then(|control| control.info()) {
            Some(value) => match value.as_object() {
                Some(cert) => {
                    let text = |key: &str| shared(cert.get(key).and_then(|v| v.as_str()));
                    let san = cert
                        .get("san")
                        .and_then(|v| v.as_array())
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.as_str())
                                .collect::<Vec<_>>()
                                .join("、")
                        })
                        .filter(|joined| !joined.is_empty())
                        .unwrap_or_else(|| "（无）".to_string());
                    let public_key = match (
                        text("pubkey_alg"),
                        cert.get("pubkey_curve").and_then(|v| v.as_str()),
                        cert.get("pubkey_bits").and_then(|v| v.as_u64()),
                    ) {
                        (Some(alg), Some(curve), _) => format!("{alg} · {curve}"),
                        (Some(alg), None, Some(bits)) => format!("{alg} · {bits} 位"),
                        (Some(alg), None, None) => alg.to_string(),
                        _ => "—".to_string(),
                    };
                    DescriptionList::horizontal()
                        .columns(1)
                        .bordered(true)
                        // 120 px：最宽的那条标签是「指纹 (SHA-256)」，96 会让它折成两行。
                        .label_width(px(120.))
                        .item("类型", "自签根证书（CA）", 1)
                        .item("主体", text("common_name").unwrap_or_else(|| "—".into()), 1)
                        .item(
                            "组织",
                            text("organization").unwrap_or_else(|| "—".into()),
                            1,
                        )
                        .item(
                            "版本",
                            // CertInfo 的 version 是数字（DER 里 0 基，序列化时已 +1 成人读值）。
                            match cert.get("version").and_then(|v| v.as_u64()) {
                                Some(version) => format!("X.509 v{version}"),
                                None => "X.509".to_string(),
                            },
                            1,
                        )
                        .item(
                            "序列号",
                            mono(text("serial_hex").unwrap_or_else(|| "—".into())),
                            1,
                        )
                        .item("生效", text("not_before").unwrap_or_else(|| "—".into()), 1)
                        .item("到期", text("not_after").unwrap_or_else(|| "—".into()), 1)
                        .item("签名算法", text("sig_alg").unwrap_or_else(|| "—".into()), 1)
                        .item("公钥", public_key, 1)
                        .item("SAN", san, 1)
                        .item(
                            "指纹 (SHA-256)",
                            mono(text("fingerprint").unwrap_or_else(|| "—".into())),
                            1,
                        )
                        .into_any_element()
                }
                None => ca_absent(muted),
            },
            None => ca_absent(muted),
        };

        group(
            "证书摘要",
            "客户端要信任的就是这一张；指纹用来与设备上手到的证书核对。",
            summary,
            None,
            muted,
        )
    }

    /// 系统信任：只读探测当前用户与本机两个 ROOT 存储。
    fn render_trust(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = cx.global::<Host>();
        let muted = cx.theme().muted_foreground;
        let ok = cx.theme().success;
        let warn = cx.theme().warning;

        let body: AnyElement = match host.ca() {
            Some(control) => {
                let probe = control.trust();
                let line = |label: &'static str, presence: StorePresence| {
                    let (text, color) = match presence {
                        StorePresence::Installed => ("已安装", ok),
                        StorePresence::Missing => ("未安装", warn),
                        StorePresence::Unknown => ("未知", muted),
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(
                            div()
                                .w_24()
                                .text_color(muted)
                                .child(SharedString::from(label)),
                        )
                        .child(div().text_color(color).child(SharedString::from(text)))
                };
                let verdict = if probe.is_installed() {
                    "客户端在这台机器上可以信任本工具签发的证书。"
                } else {
                    "还没装进系统：把下面的证书导入「受信任的根证书颁发机构」，重启浏览器再试。"
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(line("当前用户 · 根证书", probe.current_user))
                    .child(line("本机 · 根证书", probe.machine))
                    .child(div().text_sm().child(verdict))
                    .when_some(probe.detail, |this, detail| {
                        this.child(div().text_xs().text_color(muted).child(detail))
                    })
                    .into_any_element()
            }
            None => div()
                .text_sm()
                .text_color(muted)
                .child("这份宿主没有物化共享 CA（测试注入），无从探测系统信任。")
                .into_any_element(),
        };

        group(
            "系统信任",
            "只读探测：本工具不写系统证书存储，安装与卸载由你决定。",
            body,
            Some(
                "重新签发之后，旧证书仍留在系统里，需要你手工清理。"
                    .to_string()
                    .into(),
            ),
            muted,
        )
    }

    /// 证书文件：路径 + 两个剪贴板动作。
    fn render_file(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = cx.global::<Host>();
        let muted = cx.theme().muted_foreground;
        let mono_family = cx.theme().mono_font_family.clone();
        let path_text = host.ca_cert_path().display().to_string();
        let pem_available = host
            .ca()
            .map(|control| control.pem())
            .is_some_and(|pem| !String::from_utf8_lossy(pem.as_slice()).trim().is_empty());
        let this = cx.weak_entity();

        let body = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div().flex_1().min_w_0().child(
                    div()
                        .font(gpui_kit::Font {
                            family: mono_family,
                            ..Default::default()
                        })
                        .child(SharedString::from(path_text.clone())),
                ),
            )
            .child(
                Button::new("copy-ca-path")
                    .xsmall()
                    .ghost()
                    .icon(Icon::new(IconName::Copy))
                    .label("复制路径")
                    .tooltip("复制证书文件路径")
                    .on_click({
                        let this = this.clone();
                        let path = path_text.clone();
                        move |_, window, cx| {
                            let _ = this.update(cx, |view, cx| {
                                view.copy(path.clone(), "已复制证书路径", window, cx)
                            });
                        }
                    }),
            )
            .child(
                Button::new("copy-ca-pem")
                    .xsmall()
                    .ghost()
                    .icon(Icon::new(IconName::File))
                    .label("复制 PEM")
                    .tooltip("把证书正文复制到剪贴板（只含证书，不含私钥）")
                    .disabled(!pem_available)
                    .on_click({
                        let this = this.clone();
                        move |_, window, cx| {
                            let Some(pem) = cx.global::<Host>().ca().map(|control| control.pem())
                            else {
                                return;
                            };
                            let pem = String::from_utf8_lossy(pem.as_slice()).into_owned();
                            let _ = this.update(cx, |view, cx| {
                                view.copy(pem, "已复制证书 PEM", window, cx)
                            });
                        }
                    }),
            )
            .into_any_element();

        group(
            "证书文件",
            "文件在状态目录下，只读；这里只给路径与正文，CA 私钥不在界面里。",
            body,
            Some(
                "所有环境共用这一张 CA：客户端装一次就够，重建环境不用重装。"
                    .to_string()
                    .into(),
            ),
            muted,
        )
    }

    /// 按域名签发：给指定域名签一张叶子证书（证书 + 私钥都在剪贴板）。
    fn render_issue(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let has_ca = cx.global::<Host>().ca().is_some();
        let this = cx.weak_entity();

        let action = Button::new("ca-issue")
            .small()
            .outline()
            .icon(Icon::new(IconName::FileText))
            .label("按域名签发")
            .disabled(!has_ca)
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |view, cx| view.open_issue_dialog(window, cx));
            });

        // 按钮**不能直接挂在分组内容里**：`GroupBox` 是 `v_flex`，子项沿交叉轴
        // stretch，按钮会被拉成一条整行宽的横条（看起来像输入框）。包一层行容器
        // 让按钮回到自己的宽度。
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().flex().items_start().child(action))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("签发结果只做展示与复制，不写盘、也不改变 MITM 的现签行为。"),
            );

        if let Some(leaf) = &self.issued {
            let this = cx.weak_entity();
            let mono_family = cx.theme().mono_font_family.clone();
            let cert = leaf.cert_pem.clone();
            let key = leaf.key_pem.clone();
            body = body
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .font(gpui_kit::Font {
                                    family: mono_family,
                                    ..Default::default()
                                })
                                .text_sm()
                                .child(SharedString::from(format!("{} · 已签发", leaf.host))),
                        )
                        .child({
                            let this = this.clone();
                            let cert = cert.clone();
                            Button::new("ca-issue-copy-cert")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(IconName::Copy))
                                .label("复制证书")
                                .on_click(move |_, window, cx| {
                                    let cert = cert.clone();
                                    let _ = this.update(cx, |view, cx| {
                                        view.copy(cert, "已复制叶子证书", window, cx)
                                    });
                                })
                        })
                        .child({
                            let this = this.clone();
                            Button::new("ca-issue-copy-key")
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(IconName::Copy))
                                .label("复制私钥")
                                .on_click(move |_, window, cx| {
                                    let key = key.clone();
                                    let _ = this.update(cx, |view, cx| {
                                        view.copy(key, "已复制叶子私钥", window, cx)
                                    });
                                })
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("私钥只在这一刻给到剪贴板；屏上不落盘，关掉本页即清空。"),
                );
        }

        group(
            "按域名签发",
            "给某个域名单独签一张叶子证书，用来核对签发结果或交给别的程序。",
            body.into_any_element(),
            None,
            muted,
        )
    }

    /// 重新签发：换一张新 CA（已装的旧证书会失效）。
    fn render_rotate(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let has_ca = cx.global::<Host>().ca().is_some();
        let fingerprint = cx
            .global::<Host>()
            .ca()
            .map(|control| control.fingerprint())
            .unwrap_or_else(|| "—".to_string());
        let this = cx.weak_entity();

        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            // 同 `render_issue`：分组内容默认 stretch，按钮要包一层行容器。
            .child(
                div().flex().items_start().child(
                    Button::new("ca-rotate")
                        .small()
                        .outline()
                        .icon(Icon::new(IconName::RotateCw))
                        .label("重新签发")
                        .disabled(!has_ca)
                        .on_click(move |_, window, cx| {
                            let _ = this.update(cx, |view, cx| view.open_rotate_dialog(window, cx));
                        }),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(format!(
                        "当前指纹 {fingerprint}。换一张之后所有客户端都要重装证书；运行中的环境不用重启，下一条连接就用新证书。"
                    ))),
            )
            .into_any_element();

        group(
            "重新签发",
            "生成一张全新的自签根证书，覆盖状态目录里的证书文件。",
            body,
            None,
            muted,
        )
    }

    /// 如何使用：拿到证书之后要做的事，以及这张证书的边界。
    fn render_usage(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let steps = [
            "把证书文件交给目标设备（手机、浏览器所在机器、命令行工具）。",
            "在设备上把它作为「受信任的根证书颁发机构」导入；浏览器需重启。",
            "只给需要抓包的环境用：装了它，这台设备就会信任本工具签出的任意域名。",
        ];
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .children(steps.into_iter().enumerate().map(|(ix, step)| {
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .text_sm()
                    .child(
                        div()
                            .text_color(muted)
                            .child(SharedString::from(format!("{}.", ix + 1))),
                    )
                    // 文本要包一层吃剩余宽度的容器才会换行：直接当 flex 子项时
                    // 它按内容宽度参与布局，窄列里会被裁掉（两列版面暴露的）。
                    .child(div().flex_1().min_w_0().child(SharedString::from(step)))
            }))
            .into_any_element();

        group(
            "如何使用",
            "装上之后，客户端才会信任本工具签发的证书。",
            body,
            None,
            muted,
        )
    }

    /// 按域名签发：输入框 → 签发 → 复制到剪贴板。
    fn open_issue_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view: WeakEntity<Self> = cx.weak_entity();
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("api.example.com 或 127.0.0.1"));
        window.open_dialog(cx, move |dialog, _window, cx| {
            let view = view.clone();
            let input = input.clone();
            dialog
                .title("按域名签发")
                .child(
                    Field::new()
                        .label("域名或 IP")
                        .child(Input::new(&input).w_full()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "签一张服务端用途的叶子证书（CN 与 SAN 都是它）。IP 字面量走 iPAddress。",
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("取消")))
                        .child(
                            DialogAction::new()
                                .child(Button::new("ca-issue-ok").primary().label("签发")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let host = input.read(cx).value().trim().to_string();
                    if host.is_empty() {
                        return false;
                    }
                    let _ = view.update(cx, |view, cx| view.issue(host, window, cx));
                    true
                })
        });
    }

    fn issue(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(control) = cx.global::<Host>().ca().cloned() else {
            window.push_notification(
                Notification::error("这份宿主没有物化共享 CA，无法签发")
                    .id1::<Self>("ca-issue-done"),
                cx,
            );
            return;
        };
        match control.issue_pem(&host) {
            Ok(leaf) => {
                // 证书 + 私钥一起进剪贴板：这是"交给外部程序用"最直接的形态。
                cx.write_to_clipboard(ClipboardItem::new_string(format!(
                    "{}\n{}",
                    leaf.cert_pem, leaf.key_pem
                )));
                self.issued = Some(leaf);
                cx.notify();
                window.push_notification(
                    Notification::success("已签发并复制证书与私钥").id1::<Self>("ca-issue-done"),
                    cx,
                );
            }
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("{}: {}", error.code.as_str(), error.message))
                        .id1::<Self>("ca-issue-done"),
                    cx,
                );
            }
        }
    }

    /// 重新签发：先确认，再换一张新的。
    fn open_rotate_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view: WeakEntity<Self> = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let view = view.clone();
            dialog
                .title("重新签发共享 CA")
                .child(
                    div()
                        .text_sm()
                        .child("会生成一张全新的根证书并覆盖现有文件。"),
                )
                .child(
                    div()
                        .mt_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "已安装旧证书的客户端会立刻失去信任：需要重装新证书。运行中的环境不用重启。",
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("取消")))
                        .child(
                            DialogAction::new()
                                .child(Button::new("ca-rotate-ok").primary().label("重新签发")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let _ = view.update(cx, |view, cx| view.rotate(window, cx));
                    true
                })
        });
    }

    fn rotate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(control) = cx.global::<Host>().ca().cloned() else {
            window.push_notification(
                Notification::error("这份宿主没有物化共享 CA，无法重新签发")
                    .id1::<Self>("ca-rotate-done"),
                cx,
            );
            return;
        };
        match control.rotate() {
            Ok(_) => {
                // 旧叶子证书记载的是旧 CA，屏上的签发结果一并作废。
                self.issued = None;
                cx.notify();
                window.push_notification(
                    Notification::success(format!("已重新签发，新指纹 {}", control.fingerprint()))
                        .id1::<Self>("ca-rotate-done"),
                    cx,
                );
            }
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("{}: {}", error.code.as_str(), error.message))
                        .id1::<Self>("ca-rotate-done"),
                    cx,
                );
            }
        }
    }
}

impl Render for CaScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.render_summary(cx);
        let trust = self.render_trust(cx);
        let file = self.render_file(cx);
        let issue = self.render_issue(cx);
        let rotate = self.render_rotate(cx);
        let usage = self.render_usage(cx);
        let muted = cx.theme().muted_foreground;

        // 两组一列：证书摘要与信任判定是最常看的两块，竖着排首屏只到第三组；
        // 窗口够宽时并成两列，六个分组一屏能看全。阈值按**默认窗口**（1180）取 ——
        // 再窄每列就不够放 120px 标签的摘要表了。
        let wide = window.viewport_size().width >= px(1160.);
        let body: AnyElement = if wide {
            div()
                .flex()
                .items_start()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap_4()
                        .child(summary)
                        .child(file)
                        .child(issue),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap_4()
                        .child(trust)
                        .child(rotate)
                        .child(usage),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(summary)
                .child(trust)
                .child(file)
                .child(issue)
                .child(rotate)
                .child(usage)
                .into_any_element()
        };

        div()
            .id("ca-screen")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_4()
            .overflow_y_scroll()
            .p_4()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("本机所有环境共用的那张共享 CA。"),
            )
            .child(body)
    }
}

/// 没有 CA 时的降级说明（测试注入的宿主）。
fn ca_absent(muted: Hsla) -> AnyElement {
    div()
        .id("ca-absent")
        .text_sm()
        .text_color(muted)
        .child("这份宿主没有物化共享 CA（测试注入），所以没有摘要可看。")
        .into_any_element()
}

/// 一组：小标题 + 说明 + 内容（+ 组脚注）。表面与设置分组同档（`outline`，理由见
/// `views/settings.rs` 的 `group`：暗色下填充色不可辨）。
fn group(
    title: &'static str,
    description: &'static str,
    body: AnyElement,
    footer: Option<SharedString>,
    muted: Hsla,
) -> AnyElement {
    GroupBox::new()
        .id(SharedString::from(format!("ca-group-{title}")))
        .outline()
        .title(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    Label::new(title)
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(div().text_xs().text_color(muted).child(description)),
        )
        .child(body)
        .when_some(footer, |this, note| {
            this.footer(div().text_xs().text_color(muted).child(note))
        })
        .into_any_element()
}

fn shared(value: Option<&str>) -> Option<SharedString> {
    value.map(|text| text.to_string().into())
}
