//! 字体族与字阶。四个字号，各级只差一档，没有 13 / 15 这种半级。
//!
//! 雅黑是**开发期**的替身：它随 Windows 附带、`Microsoft YaHei UI` 在 125% 缩放下
//! 的度量已实测过（golden 就是按它拍的）。发布版必须换成思源黑体（SIL OFL，
//! 可分发），换字体等于重拍全部 golden —— 这是 D6 的由来。

/// 界面正文族。中文优先，不写 fallback 链：Windows 自带雅黑、macOS 自带苹方，
/// 让平台自己兜住比写一条死链更稳。
pub const FONT_TEXT: &str = "Microsoft YaHei UI";

/// 标题族。当前与正文同族，靠字重与字号拉开层级，不引入第二套字形。
pub const FONT_DISPLAY: &str = "Microsoft YaHei UI";

/// 等宽族：端口、路径、指纹、规则代码块。
pub const FONT_MONO: &str = "Cascadia Mono";

/// 12：表头、单元格、说明、按钮文字。
pub const CAPTION: f32 = 12.0;

/// 14：行名、页签、抽屉标题。
pub const BODY: f32 = 14.0;

/// 16：卡片标题（证书页）。
pub const SUBTITLE: f32 = 16.0;

/// 20：窗口级标题。
pub const TITLE: f32 = 20.0;
