//! 错误的界面呈现规范：**错误码 + 英文 msg + 中文提示**，三层各司其职。
//!
//! 契约在 `spec/errors.md`。三层分工固定，任何一层都不许越位：
//!
//! * **错误码**（`envboard_engine::ErrorCode`，即契约字面量 `invalid_config` …）——
//!   core 判定，界面只读：不据它重新分类，也不把码改写成别的字；
//! * **英文 msg**（`Error::message` 与 `Error::field`）—— 服务端原文，原样保留。
//!   它常含唯一可行动的细节（端口号、路径、占用人），排障与提 issue 要的就是这一句；
//! * **中文提示**（本模块）—— 用户读到的第一句，必须说清"哪个动作失败 + 什么结论"。
//!
//! 为什么不能只显示英文原文：整个界面其余文字都是中文，最显眼的位置突然出现英文，
//! 用户的第一反应是"程序出错了"而不是"我的操作没成功"（走查 F10）。
//! 为什么又不能只显示中文：中文结论由错误码推导，粒度只到"哪一类"，具体是哪个端口、
//! 哪个文件只有服务端原文说得清 —— 两层都留，中文负责"怎么了"，原文负责"具体是什么"。
//!
//! 于是界面上的一条错误长这样（错误条里就是这两行）：
//!
//! ```text
//! 启动 dev 失败：端口已被其他程序占用          ← 中文主句（summary）
//! [port_conflict] port 8080 already in use · field: environment.listen.port   ← 原文（detail）
//! ```

use envboard_engine::{Error, ErrorCode};
use gpui_kit::SharedString;

/// 一次失败对应的**动作**（决定中文主句的前半截）。
///
/// 用枚举而不是让调用方传字符串：动作是有限的、会被单测逐个覆盖的；写成字符串
/// 就会在某个新加的调用点上悄悄漂成另一种说法，主句的语法也就散了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 拉一次全量快照（环境列表）。
    LoadSnapshot,
    CreateEnvironment,
    StartEnvironment,
    StopEnvironment,
    RestartEnvironment,
    DeleteEnvironment,
    /// 详情面板保存环境配置。
    SaveEnvironment,
    /// 规则目录列名字。
    LoadRules,
    /// 读规则正文。
    ReadRules,
    DeleteRules,
    ImportRules,
    /// 代理目录列表。
    LoadProxies,
    /// 新建 / 编辑上游代理。
    SaveProxy,
    DeleteProxy,
    StartDebug,
    StopDebug,
    ClearCapture,
}

impl Action {
    /// 主句的「做什么 + 失败了」部分（`启动 dev 失败` / `加载环境列表失败`）。
    ///
    /// 有对象名时是「动词 + 对象 + 失败」，没有时用自解释的短语 —— 后者必须能独立成句，
    /// 不能读成「加载环境列表环境」，也不能把拉丁名字和「失败」挤在一起（`dev失败`）。
    fn headline(self, subject: Option<&str>) -> String {
        fn with(subject: Option<&str>, verb: &str, alone: &str) -> String {
            match subject {
                Some(name) => format!("{verb} {name} 失败"),
                None => format!("{alone}失败"),
            }
        }
        match self {
            Action::LoadSnapshot => "加载环境列表失败".to_string(),
            Action::CreateEnvironment => with(subject, "新建", "新建环境"),
            Action::StartEnvironment => with(subject, "启动", "启动环境"),
            Action::StopEnvironment => with(subject, "停止", "停止环境"),
            Action::RestartEnvironment => with(subject, "重启", "重启环境"),
            Action::DeleteEnvironment => with(subject, "删除", "删除环境"),
            Action::SaveEnvironment => with(subject, "保存", "保存环境配置"),
            Action::LoadRules => "加载规则目录失败".to_string(),
            Action::ReadRules => with(subject, "读取", "读取规则集"),
            Action::DeleteRules => with(subject, "删除规则集", "删除规则集"),
            Action::ImportRules => with(subject, "导入规则集", "导入规则集"),
            Action::LoadProxies => "加载代理列表失败".to_string(),
            Action::SaveProxy => with(subject, "保存上游代理", "保存上游代理"),
            Action::DeleteProxy => with(subject, "删除代理", "删除代理"),
            Action::StartDebug => with(subject, "开始调试", "开始调试"),
            Action::StopDebug => with(subject, "停止调试", "停止调试"),
            Action::ClearCapture => with(subject, "清空记录", "清空记录"),
        }
    }

    /// 成功回执的短句（`已启动 dev`）。只对**写操作**有意义；读操作返回 `None`，
    /// 免得"加载环境列表成功"这种没人要的话冒出来。
    ///
    /// 与 [`Self::headline`] 同一份动词表：成功和失败是同一件事的两面，
    /// 各写一份必然漂成"启动 dev 失败" / "dev 已开始"这样的两种说法。
    pub fn success(self, subject: Option<&str>) -> Option<String> {
        let verb = match self {
            Action::CreateEnvironment => "已新建",
            Action::StartEnvironment => "已启动",
            Action::StopEnvironment => "已停止",
            Action::RestartEnvironment => "已重启",
            Action::DeleteEnvironment => "已删除",
            Action::SaveEnvironment => "已保存",
            Action::DeleteRules => "已删除规则集",
            Action::ImportRules => "已导入规则集",
            Action::SaveProxy => "已保存上游代理",
            Action::DeleteProxy => "已删除代理",
            Action::StartDebug => "已开始调试",
            Action::StopDebug => "已停止调试",
            Action::ClearCapture => "已清空记录",
            // 读操作没有"成功"可报：数据出现在界面上就是回执。
            Action::LoadSnapshot | Action::LoadRules | Action::ReadRules | Action::LoadProxies => {
                return None;
            }
        };
        Some(match subject {
            Some(name) => format!("{verb} {name}"),
            None => verb.to_string(),
        })
    }
}

/// 错误码 → 中文结论。**每个码都必须有一句**，且与 `spec/errors.md` 的"含义"一列对齐：
/// 这张表就是契约那列的界面说法，改判定要改契约，改措辞就改这里。
///
/// 结论只说"哪一类 + 下一步大概做什么"，不重复服务端原文里的具体值 —— 那在 detail 行。
pub fn hint(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidConfig => "配置不合法，请按提示修正后重试",
        ErrorCode::NotFound => "目标不存在，可能已被删除",
        ErrorCode::Conflict => "与现有数据冲突（重名，或仍被其他资源引用）",
        ErrorCode::PortConflict => "端口已被其他程序占用",
        ErrorCode::PortRangeExhausted => "端口区间内已没有可用端口",
        ErrorCode::ConfigMismatch => "配置没有生效",
        ErrorCode::StoreFailure => "状态文件读写失败",
        ErrorCode::InternalError => "内部错误，具体原因见下一行原文",
    }
}

/// 一条错误在界面上的最小形状：中文主句 + 契约错误码 + 服务端原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// 中文主句：`{动作}{对象} 失败：{结论}`。
    pub summary: SharedString,
    /// 契约错误码字面量。本地失败（例如偏好写盘）没有服务端错误码，为 `None`。
    pub code: Option<&'static str>,
    /// 服务端英文原文，原样保留。
    pub message: Option<SharedString>,
    /// 契约里的点分字段路径（`environment.listen.port`），有则附在原文后面。
    pub field: Option<SharedString>,
}

impl Notice {
    /// 从门面返回的 [`Error`] 构造：码进 `code`，原文与字段路径进 detail。
    pub fn from_error(action: Action, subject: Option<&str>, error: &Error) -> Self {
        Self {
            summary: SharedString::from(format!(
                "{}：{}",
                action.headline(subject),
                hint(error.code)
            )),
            code: Some(error.code.as_str()),
            message: Some(SharedString::from(error.message.clone())),
            field: error.field.as_deref().map(SharedString::from),
        }
    }

    /// 没有后台回执的错误（界面自己判出来的、或来自非门面通道的说明）。
    ///
    /// 这类消息本来就该是中文：它没有错误码可映射，也没有服务端原文可保留。
    pub fn plain(summary: impl Into<SharedString>) -> Self {
        Self {
            summary: summary.into(),
            code: None,
            message: None,
            field: None,
        }
    }

    /// 错误条第二行：`[code] 英文原文 · field: 路径`。
    ///
    /// 拼成一个字符串而不是结构化渲染，是因为错误条用的是组件库的 `Alert`
    /// （只有 title + message 两个文本位），拆成多个元素就得自己重造一遍错误条的
    /// 视觉语汇 —— 本轮只定"显示什么"，不重画"长什么样"。
    pub fn detail_text(&self) -> Option<SharedString> {
        let message = self.message.as_ref()?;
        let mut text = String::new();
        if let Some(code) = self.code {
            text.push('[');
            text.push_str(code);
            text.push_str("] ");
        }
        text.push_str(message);
        if let Some(field) = &self.field {
            text.push_str(" · field: ");
            text.push_str(field);
        }
        Some(SharedString::from(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_CODES: [ErrorCode; 8] = [
        ErrorCode::InvalidConfig,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::PortConflict,
        ErrorCode::PortRangeExhausted,
        ErrorCode::ConfigMismatch,
        ErrorCode::StoreFailure,
        ErrorCode::InternalError,
    ];

    #[test]
    fn every_contract_code_has_a_chinese_hint() {
        // match 已经是穷尽的（加码不编译），这条钉的是"每句话都非空且不只是重复码名"。
        for code in ALL_CODES {
            let text = hint(code);
            assert!(!text.is_empty(), "{code} 没有中文结论");
            assert!(!text.is_ascii(), "{code} 的中文结论里没有中文");
            assert_ne!(text, code.as_str(), "{code} 的结论不能只是码名");
        }
    }

    #[test]
    fn summary_reads_as_verb_object_failure() {
        let error = Error::new(ErrorCode::PortConflict, "port 8080 already in use");
        let notice = Notice::from_error(Action::StartEnvironment, Some("dev"), &error);
        assert_eq!(
            notice.summary.as_ref(),
            "启动 dev 失败：端口已被其他程序占用"
        );
    }

    #[test]
    fn summary_without_subject_uses_the_standalone_phrase() {
        let error = Error::new(ErrorCode::InternalError, "boom");
        let notice = Notice::from_error(Action::LoadSnapshot, None, &error);
        assert_eq!(
            notice.summary.as_ref(),
            "加载环境列表失败：内部错误，具体原因见下一行原文"
        );
        // 反例：不能拼出「加载环境列表环境」这种把名词重复一遍的句子。
        assert!(!notice.summary.contains("列表环境"));
    }

    #[test]
    fn success_lines_cover_writes_and_skip_reads() {
        // 写操作：动词 + 对象，成功与失败同一份动词表。
        assert_eq!(
            Action::StartEnvironment.success(Some("dev")).as_deref(),
            Some("已启动 dev")
        );
        assert_eq!(
            Action::DeleteEnvironment.success(Some("beta")).as_deref(),
            Some("已删除 beta")
        );
        assert_eq!(
            Action::ClearCapture.success(None).as_deref(),
            Some("已清空记录")
        );
        // 读操作没有"成功"可报：数据出现就是回执。
        assert!(Action::LoadSnapshot.success(None).is_none());
        assert!(Action::LoadRules.success(None).is_none());
        assert!(Action::ReadRules.success(None).is_none());
        assert!(Action::LoadProxies.success(None).is_none());
    }

    #[test]
    fn detail_keeps_code_message_and_field() {
        let error = Error::at(
            ErrorCode::InvalidConfig,
            "environment.listen.port",
            "port 0 is invalid",
        );
        let notice = Notice::from_error(Action::CreateEnvironment, None, &error);
        assert_eq!(notice.code, Some("invalid_config"));
        assert_eq!(
            notice.detail_text().unwrap().as_ref(),
            "[invalid_config] port 0 is invalid · field: environment.listen.port"
        );
    }

    #[test]
    fn detail_without_field_has_no_trailing_separator() {
        let error = Error::new(ErrorCode::StoreFailure, "cannot write ledger");
        let notice = Notice::from_error(Action::LoadSnapshot, None, &error);
        assert_eq!(
            notice.detail_text().unwrap().as_ref(),
            "[store_failure] cannot write ledger"
        );
    }

    #[test]
    fn plain_notice_has_no_second_line() {
        let notice = Notice::plain("调试会话已失效：目标环境不在运行");
        assert_eq!(notice.detail_text(), None);
    }
}
