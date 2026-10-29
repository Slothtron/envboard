//! CA 屏：共享根证书的**看**与**取用**。
//!
//! 主任务只有一个：*确认这是哪一张证书，并把它交到客户端手里*。屏面按这个顺序排：
//! **证书摘要**（我是谁、管到什么时候）→ **证书文件**（在哪、怎么拿走）→
//! **如何使用**（拿到之后要做的事与边界）。
//!
//! 三条纪律：
//!
//! * **私钥不在这里**，也不在任何交付给界面的结构里（`CaBundle` 只有证书 PEM 与摘要）；
//! * **摘要与有效期都是读来的，不是算的**：字段全部来自 `CaBundle.info`
//!   （引擎物化 CA 时解析），界面不做证书解析，也不推断"还剩几天"；
//! * 这张 CA 是**工作区级单例**：所有环境共用，重建环境不需要重装证书 —— 这句话必须
//!   写在屏上，否则用户会以为每个环境各有一张。

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::host::Host;

/// CA 屏。
pub struct CaScreen {
    /// 复制动作的轻提示（切屏即清；不做会自己消失的 toast）。
    copied: Option<SharedString>,
}

impl CaScreen {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { copied: None }
    }

    fn copy(&mut self, text: String, note: &'static str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(note.into());
        cx.notify();
    }

    /// 证书摘要：`CaBundle.info` 的字段逐条摆出来，一个都不自己算。
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

        let summary: AnyElement = match host.ca().info.as_ref().and_then(|v| v.as_object()) {
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
            None => div()
                .id("ca-absent")
                .text_sm()
                .text_color(muted)
                .child("这份宿主没有物化共享 CA（测试注入），所以没有摘要可看。")
                .into_any_element(),
        };

        group(
            "证书摘要",
            "客户端要信任的就是这一张；指纹用来与设备上手到的证书核对。",
            summary,
            None,
            muted,
        )
    }

    /// 证书文件：路径 + 两个剪贴板动作。
    fn render_file(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = cx.global::<Host>();
        let muted = cx.theme().muted_foreground;
        let mono_family = cx.theme().mono_font_family.clone();
        let path_text = host.ca_cert_path().display().to_string();
        let pem_available = !String::from_utf8_lossy(host.ca().pem.as_slice())
            .trim()
            .is_empty();
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
                        move |_, _, cx| {
                            let _ = this.update(cx, |view, cx| {
                                view.copy(path.clone(), "已复制证书路径", cx)
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
                        move |_, _, cx| {
                            let pem =
                                String::from_utf8_lossy(cx.global::<Host>().ca().pem.as_slice())
                                    .into_owned();
                            let _ =
                                this.update(cx, |view, cx| view.copy(pem, "已复制证书 PEM", cx));
                        }
                    }),
            )
            .into_any_element();

        group(
            "证书文件",
            "文件在状态目录下，只读；这里只给路径与正文，私钥不在界面里。",
            body,
            Some(
                "所有环境共用这一张 CA：客户端装一次就够，重建环境不用重装。"
                    .to_string()
                    .into(),
            ),
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
                    .child(SharedString::from(step))
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
}

impl Render for CaScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.render_summary(cx);
        let file = self.render_file(cx);
        let usage = self.render_usage(cx);
        let muted = cx.theme().muted_foreground;

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
            .child(summary)
            .child(file)
            .child(usage)
            .child(
                self.copied
                    .clone()
                    .map(|note| {
                        div()
                            .id("ca-copied")
                            .text_xs()
                            .text_color(cx.theme().success)
                            .child(note)
                            .into_any_element()
                    })
                    .unwrap_or_else(|| div().into_any_element()),
            )
    }
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
