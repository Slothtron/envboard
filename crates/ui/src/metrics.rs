//! 几何：行高、档位高、抽屉宽。全部是 `const`，因为有三处要用它们**算数**：
//! 页面里的 `px()`、原语的固定高度、以及 golden 断言里的期望行格子。
//!
//! 刻意不做成主题令牌：NSH 的令牌是填充与描边（`Theme::number()` 也能放数字，
//! 但那样密度档就成了运行时可变），而密度是要被 D1 断言钉死的东西。
//! 真要出密度档了，再把这里换成 `fn` 从 `theme().number()` 取，调用点不用动。
//!
//! 列宽**不在**这里：列宽是页面的排版决定（见 `crates/app/nsh/src/pages.rs`）。

/// 列表行高。Win11 设置页 32–40，取 33。
pub const ROW_H: f32 = 33.0;

/// 表头高。
pub const HEAD_H: f32 = 28.0;

/// 工具条 / 抽屉头 / 抽屉脚高。
pub const BAR_H: f32 = 48.0;

/// 顶部页签高。
pub const TAB_H: f32 = 40.0;

/// 输入框与按钮的标准高度。
pub const CONTROL_H: f32 = 32.0;

/// 全局状态条高。只此一条，页面不许再各画一条。
pub const STATUS_H: f32 = 28.0;

/// 平台标题栏高（与 `TitleBarOptions::height` 同一个值）。
pub const TITLE_BAR_H: f32 = 40.0;

/// 平台标题栏右侧三个系统按钮的总宽（40 × 3）。
pub const CAPTION_W: f32 = 40.0 * 3.0;

/// 控件圆角。与主题 JSON 的 `radius` 曾是双份真值，现已只留此处。
pub const RADIUS: f32 = 4.0;

/// 浮层圆角（抽屉、卡片）：比控件大一档但没有胶囊感。
pub const OVERLAY_RADIUS: f32 = 8.0;

/// 详情抽屉宽。可拖 420–640，默认 480。
pub const DRAWER_W: f32 = 480.0;

/// 确认框宽（一句话 + 两个按钮，再宽就是空白的行）。
pub const DIALOG_W: f32 = 420.0;

/// 设置框宽（放得下三档外观与版本行）。
pub const DIALOG_W_LG: f32 = 480.0;

/// 状态形编码的条宽（选中 3 pt 竖条、告警 3 pt 横条都用它）。
pub const BAR_W: f32 = 3.0;
