//! 令牌对账：`tokens.rs` 里那 24 个角色名，主题 JSON 的浅/深两套都必须有。
//!
//! 这条测试守的是**最贵的错**：`tokens.rs` 里名字拼错一个字母，`theme().color()`
//! 解不出来、回退成透明 —— 界面上表现为一个洞，不表现为一个错误颜色，肉眼很难抓。
//! 解析器是 `serde_json`，不是 `nsh_base::Theme`：用被测代码的解析器做对账，
//! 主题解析坏了测试会一起坏。

use std::collections::BTreeSet;

use envboard_ui::tokens::ROLES;

/// NSH 内置控件（输入框、滚动条、菜单等）用的组件层键，共 14 个。
const CONTROL_KEYS: [&str; 14] = [
    "background",
    "foreground",
    "card",
    "popover",
    "primary",
    "primary-foreground",
    "secondary",
    "muted",
    "muted-foreground",
    "accent-foreground",
    "destructive",
    "border",
    "input",
    "ring",
];

fn theme() -> serde_json::Value {
    serde_json::from_str(envboard_ui::THEME_JSON).expect("主题 JSON 可解析")
}

fn set<'a>(v: &'a serde_json::Value, appearance: &str) -> &'a serde_json::Map<String, serde_json::Value> {
    v["cssVars"][appearance]
        .as_object()
        .unwrap_or_else(|| panic!("cssVars.{appearance} 缺失"))
}

#[test]
fn every_semantic_role_exists_in_both_appearances() {
    let v = theme();
    for appearance in ["light", "dark"] {
        let map = set(&v, appearance);
        for role in ROLES {
            assert!(
                map.contains_key(role),
                "{appearance} 缺语义键 `{role}`（拼错的名字会解成透明，界面上只会是个洞）"
            );
        }
    }
}

#[test]
fn every_component_key_exists_in_both_appearances() {
    let v = theme();
    for appearance in ["light", "dark"] {
        let map = set(&v, appearance);
        for key in CONTROL_KEYS {
            assert!(
                map.contains_key(key),
                "{appearance} 缺组件键 `{key}`（NSH 的输入框与滚动条会退回默认色）"
            );
        }
    }
}

#[test]
fn role_list_is_the_whole_set_and_has_no_duplicates() {
    let mut seen = BTreeSet::new();
    for role in ROLES {
        assert!(seen.insert(role), "角色 `{role}` 出现了两次");
    }
    assert_eq!(seen.len(), 24, "语义角色是 24 个，改数量前先改设计系统 §2");
    assert_eq!(seen.len() + CONTROL_KEYS.len(), 38, "一套令牌是 38 键");
}

#[test]
fn radius_has_exactly_one_source_of_truth() {
    // 圆角只有一个真值：`metrics::RADIUS`。主题 JSON 里若也写 radius，
    // 两份数就会漂，且 Godot 式的“改哪个才对”没人答得上来。
    let v = theme();
    let theme_block = v["cssVars"]["theme"].as_object();
    assert!(
        theme_block.is_none_or(|m| !m.contains_key("radius")),
        "cssVars.theme.radius 必须删掉：圆角只在 crates/ui/src/metrics.rs 里定义"
    );
}
