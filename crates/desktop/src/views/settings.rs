//! 设置屏：**外观**（主题）+ **关于**（运行参数）。
//!
//! 共享证书不在这里 —— 它是独立的一屏（`views/ca.rs`，侧栏「CA」）：那里有它自己的
//! 任务（把证书交到客户端手里），塞进设置页只会让两边都看不清。
//!
//! 版面走组件库的设置几何：每组一个 `GroupBox` 填充表面，组内一行一项
//! （**标题与说明在左、控件在右**，与 `SettingItem` 同一横向布局），选中即生效，
//! 不设保存按钮。
//!
//! 两条纪律：
//!
//! * **主题的唯一入口在这里**。标题栏曾经有个深色开关，已移除 —— 一个只有两态的
//!   开关表达不了"系统"这一档，而两处都能改主题就会出现"谁赢"的问题。
//! * **写失败要能看见**：偏好写入 `ui.json` 失败时，错误显示在这一行下面，
//!   而不是静默变成"下次启动又回去了"。

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::IndexPath;
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};

use crate::host::Host;
use crate::ui_prefs::{ThemePreference, UiPrefs};

/// 主题下拉框的宽度（够放最长的那档文案 + 箭头，又不至于横到像输入框）。
const THEME_SELECT_W: f32 = 160.;

/// 设置屏。
pub struct Settings {
    theme: ThemePreference,
    /// 主题下拉框（选中即生效；状态由组件自己持有，这里只负责响应确认事件）。
    theme_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// 写偏好失败的说明（`None` = 正常）。
    save_error: Option<SharedString>,
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
            _subscriptions: subscriptions,
        }
    }

    /// 当前档位（只读）。给无头 UI 测试断言用 —— 视图层的状态本来就该可观察。
    pub fn theme(&self) -> ThemePreference {
        self.theme
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

    /// 外观组：一行「主题」，说明随当前档位变化。
    fn render_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme_colors = cx.theme();
        let muted = theme_colors.muted_foreground;
        let hint = match self.theme {
            ThemePreference::System => "跟随 Windows 的外观设置；系统切换明暗时界面即时跟随。",
            ThemePreference::Light => "始终使用浅色主题。",
            ThemePreference::Dark => "始终使用深色主题。",
        };
        let save_error = self.save_error.clone();

        group(
            "外观",
            "界面在本机上的样子。",
            vec![
                row(
                    "主题",
                    Some(hint.into()),
                    div()
                        .w(px(THEME_SELECT_W))
                        // 给无头 UI 测试留的定位锚点（`debug_selector` 只在
                        // debug + test-support 下记录，release 是空操作）。
                        // 组件库的下拉本身不注册 selector，没有这个锚点就只能硬编码坐标。
                        .debug_selector(|| "theme-select".to_string())
                        .child(
                            Select::new(&self.theme_select)
                                .id("theme-pref")
                                .accessibility_label("主题")
                                .placeholder("选择主题"),
                        )
                        .into_any_element(),
                    muted,
                ),
                save_error
                    .map(|message| {
                        div()
                            .id("settings-theme-error")
                            .text_xs()
                            .text_color(theme_colors.danger)
                            .child(message)
                            .into_any_element()
                    })
                    .unwrap_or_else(|| div().into_any_element()),
            ],
        )
    }

    /// 关于组：版本与本机路径，全部只读。
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
        let value = |text: String| div().text_sm().child(text).into_any_element();

        let mut items = vec![
            row(
                "引擎",
                None,
                value(format!("{} {}", core.name, core.version)),
                muted,
            ),
            row(
                "版本",
                None,
                value(env!("CARGO_PKG_VERSION").to_string()),
                muted,
            ),
            row(
                "状态目录",
                Some("环境账本、规则库、日志与共享 CA 都在它下面。".into()),
                mono(host.state_dir().display().to_string()),
                muted,
            ),
            row("端口区间", None, value(format!("{min}-{max}")), muted),
            row(
                "界面偏好",
                Some("主题只改这台机器上的界面，不动环境账本。".into()),
                mono(format!("{}/ui.json", host.state_dir().display())),
                muted,
            ),
        ];
        // 装配警告是"这一份宿主跑得不对"的信息，归在运行参数这一组，不单独开组。
        let warnings: Vec<String> = host.warnings().to_vec();
        if !warnings.is_empty() {
            items.push(
                div()
                    .id("settings-warnings")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(muted).child("装配警告"))
                    .children(warnings.into_iter().map(|warning| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(SharedString::from(warning))
                    }))
                    .into_any_element(),
            );
        }

        group("关于", "运行参数与本机路径。", items)
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let appearance = self.render_appearance(cx);
        let about = self.render_about(cx);
        let muted = cx.theme().muted_foreground;

        div()
            .id("settings-screen")
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
                    .child("本机的界面偏好与运行参数。"),
            )
            .child(appearance)
            .child(about)
    }
}

/// 一组设置：`GroupBox` 的描边表面。
///
/// 为什么是 `outline` 而不是 `fill`：暗色主题下 `group_box` 填充色与窗口底色几乎同色，
/// 组边界直接消失（实测截图确认），分组的意义就没了；描边在两个主题里都成立。
/// 组标题也自己上色：`GroupBox` 那层把标题区染成 `muted_foreground`，而设置页只有两组，
/// 标题必须与说明一眼分得开。
fn group(title: &'static str, description: &'static str, items: Vec<AnyElement>) -> AnyElement {
    GroupBox::new()
        .id(SharedString::from(format!("settings-group-{title}")))
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
                .child(div().text_xs().child(description)),
        )
        .children(items)
        .into_any_element()
}

/// 一行设置：**标题与说明在左，控件在右**（`SettingItem` 的横向几何）。
fn row(
    title: &'static str,
    description: Option<SharedString>,
    field: AnyElement,
    muted: Hsla,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .max_w_3_5()
                .child(div().text_sm().child(title))
                .children(description.map(|text| div().text_xs().text_color(muted).child(text))),
        )
        .child(field)
        .into_any_element()
}
