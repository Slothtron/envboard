//! 设计系统的颜色层：24 个具名角色，全部在绘制时从 `themes/zhongguose.json`
//! 解析（`theme().color(name)`），所以切换浅/深只是重解名字，没有一处重绘代码。
//!
//! 本文件里**没有** hex：值是数据，名字是代码。名字与 JSON 键的对账由
//! `tests/tokens_match.rs` 守着；拼错的名字会解析成透明（表现为一个洞），
//! 不会悄悄变成一个错色。
//!
//! 字号与度量不在这里：见 `font.rs`（12/14/16/20）与 `metrics.rs`（行高、抽屉宽）。
//! 颜色是 NSH 令牌能承载的东西，尺寸不是。

use nsh_base::theme;
use nsh_core::element::AdaptiveColor;

/// 角色名，唯一一处字符串真值；`ROLES` 与取色函数都引用它。
pub mod role {
    pub const PAGE: &str = "page";
    pub const LAYER: &str = "layer";
    pub const CARD_LAYER: &str = "card-layer";
    pub const CONTROL_FILL: &str = "control-fill";
    pub const SUBTLE_HOVER: &str = "subtle-hover";
    pub const SUBTLE_PRESSED: &str = "subtle-pressed";
    pub const SELECTION: &str = "selection";
    pub const DIVIDER: &str = "divider";
    pub const CONTROL_STROKE: &str = "control-stroke";
    pub const CONTROL_STROKE_STRONG: &str = "control-stroke-strong";
    pub const CARD_STROKE: &str = "card-stroke";
    pub const TEXT_PRIMARY: &str = "text-primary";
    pub const TEXT_SECONDARY: &str = "text-secondary";
    pub const TEXT_TERTIARY: &str = "text-tertiary";
    pub const TEXT_DISABLED: &str = "text-disabled";
    pub const ACCENT: &str = "accent";
    pub const ACCENT_HOVER: &str = "accent-hover";
    pub const ACCENT_INK: &str = "accent-ink";
    pub const ON_ACCENT: &str = "on-accent";
    pub const SUCCESS: &str = "success";
    pub const CAUTION: &str = "caution";
    pub const CAUTION_DOT: &str = "caution-dot";
    pub const CRITICAL: &str = "critical";
    pub const CRITICAL_HOVER: &str = "critical-hover";
}

/// 全部语义角色（24）。测试用它核对 JSON 的 light/dark 两套键。
pub const ROLES: [&str; 24] = [
    role::PAGE,
    role::LAYER,
    role::CARD_LAYER,
    role::CONTROL_FILL,
    role::SUBTLE_HOVER,
    role::SUBTLE_PRESSED,
    role::SELECTION,
    role::DIVIDER,
    role::CONTROL_STROKE,
    role::CONTROL_STROKE_STRONG,
    role::CARD_STROKE,
    role::TEXT_PRIMARY,
    role::TEXT_SECONDARY,
    role::TEXT_TERTIARY,
    role::TEXT_DISABLED,
    role::ACCENT,
    role::ACCENT_HOVER,
    role::ACCENT_INK,
    role::ON_ACCENT,
    role::SUCCESS,
    role::CAUTION,
    role::CAUTION_DOT,
    role::CRITICAL,
    role::CRITICAL_HOVER,
];

fn tok(name: &'static str) -> AdaptiveColor {
    theme().color(name).unwrap_or(AdaptiveColor::TRANSPARENT)
}

// ---- 表面 ----------------------------------------------------------------

/// 缟 / 乌黑：窗口自身底色，α 0.80 透 Mica。
pub fn page() -> AdaptiveColor {
    tok(role::PAGE)
}

/// 象牙白 / 玄青：列表体、表头、状态条。
pub fn layer() -> AdaptiveColor {
    tok(role::LAYER)
}

/// 象牙白 / 黛：卡片、抽屉、浮层。
pub fn card() -> AdaptiveColor {
    tok(role::CARD_LAYER)
}

/// 精白 / 黛：输入框与次按钮的面。
pub fn control_fill() -> AdaptiveColor {
    tok(role::CONTROL_FILL)
}

/// 玄青 6% / 霜色 10%。
pub fn hover_fill() -> AdaptiveColor {
    tok(role::SUBTLE_HOVER)
}

/// 玄青 4% / 霜色 7%。
pub fn pressed_fill() -> AdaptiveColor {
    tok(role::SUBTLE_PRESSED)
}

/// 月白 / 藏青：选中行底。
pub fn selection() -> AdaptiveColor {
    tok(role::SELECTION)
}

// ---- 描边 ----------------------------------------------------------------

pub fn divider() -> AdaptiveColor {
    tok(role::DIVIDER)
}

pub fn control_stroke() -> AdaptiveColor {
    tok(role::CONTROL_STROKE)
}

pub fn control_stroke_strong() -> AdaptiveColor {
    tok(role::CONTROL_STROKE_STRONG)
}

pub fn card_stroke() -> AdaptiveColor {
    tok(role::CARD_STROKE)
}

// ---- 墨 ------------------------------------------------------------------

pub fn ink() -> AdaptiveColor {
    tok(role::TEXT_PRIMARY)
}

pub fn ink_secondary() -> AdaptiveColor {
    tok(role::TEXT_SECONDARY)
}

pub fn ink_tertiary() -> AdaptiveColor {
    tok(role::TEXT_TERTIARY)
}

pub fn ink_disabled() -> AdaptiveColor {
    tok(role::TEXT_DISABLED)
}

// ---- 强调与状态 ----------------------------------------------------------

pub fn accent() -> AdaptiveColor {
    tok(role::ACCENT)
}

pub fn accent_hover() -> AdaptiveColor {
    tok(role::ACCENT_HOVER)
}

/// 强调色**当文字**时用它：靛蓝在缟上 7.16:1，靛青本身只有 3.92:1，只能做填充。
pub fn accent_ink() -> AdaptiveColor {
    tok(role::ACCENT_INK)
}

pub fn on_accent() -> AdaptiveColor {
    tok(role::ON_ACCENT)
}

pub fn success() -> AdaptiveColor {
    tok(role::SUCCESS)
}

/// 琥珀 / 缃色，只给 6 pt 点与 4 pt 条，永远不做句子。
pub fn caution() -> AdaptiveColor {
    tok(role::CAUTION_DOT)
}

/// 赭石 / 缃色：告警**文字**（琥珀作文字只有 3.21:1）。
pub fn caution_ink() -> AdaptiveColor {
    tok(role::CAUTION)
}

pub fn critical() -> AdaptiveColor {
    tok(role::CRITICAL)
}

pub fn critical_hover() -> AdaptiveColor {
    tok(role::CRITICAL_HOVER)
}
