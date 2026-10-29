//! 设置屏：**外观**（主题下拉框）+ **共享证书** + **关于**。
//!
//! 三件事都"看一眼就够"，所以不设页签、不设保存按钮：主题选中即生效并落盘，
//! 证书只读（私钥根本不在这里），关于段展示运行参数。
//!
//! 两条纪律：
//!
//! * **主题的唯一入口在这里**。标题栏曾经有个深色开关，已移除 —— 一个只有两态的
//!   开关表达不了"跟随系统"，而两处都能改主题就会出现"谁赢"的问题。
//! * **写失败要能看见**：偏好写入 `ui.json` 失败时，错误显示在选项下面，
//!   而不是静默变成"下次启动又回去了"。

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Disableable;
use gpui_kit::component::IndexPath;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{Icon, IconName};
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};

use crate::host::Host;
use crate::ui_prefs::{ThemePreference, UiPrefs};

/// 外观段下拉框的宽度（够放"跟随系统"+ 箭头，又不至于横到像输入框）。
const THEME_SELECT_W: f32 = 160.;

/// 设置屏。
pub struct Settings {
    theme: ThemePreference,
    /// 主题下拉框（选中即生效；状态由组件自己持有，这里只负责响应确认事件）。
    theme_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// 写偏好失败的说明（`None` = 正常）。
    save_error: Option<SharedString>,
    /// 复制动作的轻提示（"已复制证书路径" 这类；2 秒后自己消失太麻烦，切屏即清）。
    copied: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl Settings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = UiPrefs::current(cx).theme;
        let items =
            SearchableVec::new(ThemePreference::ALL.map(|pref| SharedString::from(pref.label())));
        let theme_select =
            cx.new(|cx| SelectState::new(items, Some(IndexPath::new(theme.index())), window, cx));
        let subscriptions = vec![cx.subscribe_in(
            &theme_select,
            window,
            |this: &mut Self, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                // 只有"确认"才是改主题；关闭菜单 / 清空不算（这个框也不开 cleanable）。
                if let SelectEvent::Confirm(Some(label)) = event
                    && let Some(theme) = ThemePreference::from_label(label)
                {
                    this.select_theme(theme, window, cx);
                }
            },
        )];
        Self {
            theme,
            theme_select,
            save_error: None,
            copied: None,
            _subscriptions: subscriptions,
        }
    }

    fn select_theme(
        &mut self,
        theme: ThemePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if theme == self.theme {
            return;
        }
        let host = cx.global::<Host>();
        let state_dir = host.state_dir().clone();
        let prefs = UiPrefs::current(cx).set_theme(theme, Some(window), cx);
        self.theme = theme;
        self.save_error = match prefs.save(&state_dir) {
            Ok(()) => None,
            Err(error) => Some(
                format!(
                    "主题已切换，但偏好没能写入 {}/ui.json：{error}",
                    state_dir.display()
                )
                .into(),
            ),
        };
        cx.notify();
    }

    fn copy(&mut self, text: String, note: &'static str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(note.into());
        cx.notify();
    }

    /// 外观段：主题下拉框 + 说明（+ 写失败）。
    fn render_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let hint = match self.theme {
            ThemePreference::System => "跟随 Windows 的外观设置；系统切换明暗时界面即时跟随。",
            ThemePreference::Light => "始终使用明亮主题。",
            ThemePreference::Dark => "始终使用暗色主题。",
        };
        let save_error = self.save_error.clone();

        section(
            "外观",
            "主题",
            vec![
                // 下拉框用现成的 `Select`：触发器显示当前档位，展开是带勾选的双行高列表。
                div()
                    .w(px(THEME_SELECT_W))
                    .child(
                        Select::new(&self.theme_select)
                            .id("theme-pref")
                            .accessibility_label("主题")
                            .placeholder("选择主题"),
                    )
                    .into_any_element(),
                muted_line(hint, muted),
                muted_line(
                    "选择会写入状态目录的 ui.json，下次启动沿用；它与环境账本无关。",
                    muted,
                ),
                save_error
                    .map(|message| {
                        div()
                            .id("settings-theme-error")
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(message)
                            .into_any_element()
                    })
                    .unwrap_or_else(|| div().into_any_element()),
            ],
        )
    }

    /// 证书段：摘要（只读）+ 文件位置 + 复制入口。
    fn render_ca(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = cx.global::<Host>();
        let info = host.ca().info.clone();
        let path = host.ca_cert_path();
        let pem = String::from_utf8_lossy(host.ca().pem.as_slice()).into_owned();
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
        let this = cx.weak_entity();
        let path_text = path.display().to_string();
        let pem_available = !pem.trim().is_empty();

        let summary: AnyElement = match info.as_ref().and_then(|value| value.as_object()) {
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
                    .label_width(px(96.))
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
                .id("settings-ca-absent")
                .text_sm()
                .text_color(muted)
                .child("这份宿主没有物化共享 CA（测试注入），所以没有摘要可看。")
                .into_any_element(),
        };

        section(
            "共享证书",
            "客户端要信任的根证书",
            vec![
                summary,
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(muted).child("证书文件"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(mono(path_text.clone().into())),
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
                                        let pem = pem.clone();
                                        move |_, _, cx| {
                                            let _ = this.update(cx, |view, cx| {
                                                view.copy(pem.clone(), "已复制证书 PEM", cx)
                                            });
                                        }
                                    }),
                            ),
                    )
                    .into_any_element(),
                muted_line(
                    "客户端装这张证书后，所有环境共用同一张 CA；重建环境不用重装。私钥不在本界面里。",
                    muted,
                ),
                self.copied
                    .clone()
                    .map(|note| {
                        div()
                            .id("settings-copied")
                            .text_xs()
                            .text_color(cx.theme().success)
                            .child(note)
                            .into_any_element()
                    })
                    .unwrap_or_else(|| div().into_any_element()),
            ],
        )
    }

    /// 关于段：版本、状态目录、端口区间、装配期警告。
    fn render_about(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = cx.global::<Host>();
        let core = host.admin().core_info();
        let (min, max) = host.port_range();
        let muted = cx.theme().muted_foreground;
        let mono_family = cx.theme().mono_font_family.clone();
        let mono = |text: String| {
            div()
                .font(gpui_kit::Font {
                    family: mono_family.clone(),
                    ..Default::default()
                })
                .child(SharedString::from(text))
                .into_any_element()
        };
        let warnings: Vec<AnyElement> = host
            .warnings()
            .iter()
            .map(|warning| {
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(SharedString::from(warning.clone()))
                    .into_any_element()
            })
            .collect();

        section(
            "关于",
            "运行参数",
            vec![
                DescriptionList::horizontal()
                    .columns(1)
                    .bordered(true)
                    .label_width(px(96.))
                    .item("引擎", format!("{} {}", core.name, core.version), 1)
                    .item("版本", env!("CARGO_PKG_VERSION"), 1)
                    .item("状态目录", mono(host.state_dir().display().to_string()), 1)
                    .item("端口区间", format!("{min}-{max}"), 1)
                    .item(
                        "界面偏好",
                        format!("{}/ui.json", host.state_dir().display()),
                        1,
                    )
                    .into_any_element(),
                if warnings.is_empty() {
                    muted_line("启动时没有装配警告。", muted)
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xs().text_color(muted).child("装配警告"))
                        .children(warnings)
                        .into_any_element()
                },
            ],
        )
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let appearance = self.render_appearance(cx);
        let ca = self.render_ca(cx);
        let about = self.render_about(cx);
        let muted = cx.theme().muted_foreground;

        div()
            .id("settings-screen")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_8()
                    .p_6()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("本机的界面偏好与共享证书。"),
                    )
                    .child(appearance)
                    .child(ca)
                    .child(about),
            )
    }
}

/// 一段设置：小标题 + 内容。段与段之间用大间距，段内用固定的两档间距。
///
/// 不套卡片：设置页的边界靠标题与间距就够了（设计守则：避免卡内套卡）。
fn section(title: &'static str, caption: &'static str, children: Vec<AnyElement>) -> AnyElement {
    div()
        .id(SharedString::from(format!("settings-section-{title}")))
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(div().text_xs().child(caption)),
        )
        .children(children)
        .into_any_element()
}

fn muted_line(text: &'static str, color: gpui_kit::Hsla) -> AnyElement {
    div()
        .text_xs()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn shared(value: Option<&str>) -> Option<SharedString> {
    value.map(|text| text.to_string().into())
}
