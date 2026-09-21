//! 域：规则账本（rules library）—— 导入、清单、读取、删除与对账。
//!
//! 收什么：`import_rules` / `rules_list` / `rules_read` / `rules_delete` /
//! `reconcile_rules`，以及本域私有的物化与对账机制（ensure_rules_file、
//! reconcile_rules_locked、environments_bound_locked）。
//!
//! 账本是唯一真相：`state.json` 里的 rendered 是权威文本，物化文件
//! （rules_dir/<name>.rules）只是它的投影，缺了或坏了由对账自愈。

use std::path::PathBuf;

use envboard_engine::domain::Environment;
use envboard_engine::{Error, ErrorCode, LogLevel};

use crate::manager::Manager;
use crate::state::{PersistedState, StoredRules};
use envboard_events::ControlEvent;

impl Manager {
    /// 导入规则：解析 → 规范化渲染 → 记入账本 → 物化落盘（0600）→ 热应用。
    ///
    /// 账本是唯一真相：同名再导入就是覆盖；绑定它且在跑的环境当场换快照
    /// （v2 靠注入器轮询文件达成，v3 是同步 apply —— 这就是"规则内容热生效"）。
    /// 非法内容不让整次导入失败是有意的：一个笔误不该让其余几百条一起报废。
    pub fn import_rules(&self, name: &str, source_text: &str) -> Result<PathBuf, Error> {
        envboard_engine::domain::validate_rules_name(name)?;
        let import = envboard_engine::rules::parse_hosts_text(source_text);
        let rendered = envboard_engine::rules::render_rules(&import.entries, name)?;

        let entry = StoredRules {
            name: name.to_string(),
            source: None,
            rendered: rendered.clone(),
            entries: import.accepted(),
            skipped: import.skipped.len(),
            conflicts: import.conflicts.len(),
            imported_at: self.clock.now_unix(),
        };

        let mut state = self.state.lock().unwrap();
        match state
            .rules
            .iter_mut()
            .find(|existing| existing.name == name)
        {
            Some(existing) => *existing = entry,
            None => state.rules.push(entry),
        }
        state
            .rules
            .sort_by(|left, right| left.name.cmp(&right.name));
        let path = self.config.rules_path(name);
        crate::agent::write_private(&path, rendered.as_bytes())?;
        self.commit(
            &state,
            ControlEvent::RulesImported {
                rules_name: name.to_string(),
                rules_sha256: envboard_engine::sha256::hex(rendered.as_bytes()),
            },
        )?;
        let affected = self.environments_bound_locked(&state, name);
        drop(state);

        self.logger.log(
            LogLevel::Info,
            &format!(
                "imported rules {name}: {} entries, {} skipped, {} conflict(s)",
                import.accepted(),
                import.skipped.len(),
                import.conflicts.len()
            ),
        );
        for env in affected {
            self.hot_update_engine(&env);
        }
        Ok(path)
    }

    /// 规则库清单（从账本读，不依赖物化文件是否存在）。
    pub fn rules_list(&self) -> Result<Vec<String>, Error> {
        let state = self.state.lock().unwrap();
        Ok(state.rules.iter().map(|entry| entry.name.clone()).collect())
    }

    /// 读规则正文：账本里的 rendered 就是权威文本，所以物化文件被删也读得到。
    pub fn rules_read(&self, name: &str) -> Result<String, Error> {
        envboard_engine::domain::validate_rules_name(name)?;
        let state = self.state.lock().unwrap();
        if let Some(entry) = state.rules_entry(name) {
            return Ok(entry.rendered.clone());
        }
        // 账本里没有但文件在（例如有人手工放了文件而启动对账还没跑）：读文件。
        let path = self.config.rules_path(name);
        if self.files.exists(&path) {
            return self.files.read_to_string(&path);
        }
        Err(Error::at(
            ErrorCode::NotFound,
            "rules",
            format!("rules {name:?} is not in the rules library"),
        ))
    }

    /// 删除规则。被任何环境绑定时拒绝 —— 否则那个环境会静默失去覆盖。
    pub fn rules_delete(&self, name: &str) -> Result<(), Error> {
        envboard_engine::domain::validate_rules_name(name)?;
        let mut state = self.state.lock().unwrap();
        for raw in &state.environments {
            let environment = Environment::from_json(raw)?;
            if environment.rules() == Some(name) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!(
                        "rules {name:?} is bound to environment {:?}; unbind it first",
                        environment.name()
                    ),
                ));
            }
        }
        state.rules.retain(|entry| entry.name != name);
        self.commit(
            &state,
            ControlEvent::RulesDeleted {
                rules_name: name.to_string(),
            },
        )?;
        drop(state);

        let path = self.config.rules_path(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// 规则库对账（幂等）：升级迁移与自愈。软链时代已随注入器退场。
    pub fn reconcile_rules(&self) -> Result<Vec<String>, Error> {
        let mut state = self.state.lock().unwrap();
        let messages = self.reconcile_rules_locked(&mut state)?;
        self.commit(
            &state,
            ControlEvent::Custom {
                kind: "manager/rules-reconciled".to_string(),
                payload: serde_json::json!({ "repaired": messages.len() }),
            },
        )?;
        Ok(messages)
    }

    /// 把账本里的规则物化到 rules_dir/<name>.rules（缺了或内容不一致才写）。
    fn ensure_rules_file(&self, state: &PersistedState, name: &str) -> Result<bool, Error> {
        let Some(entry) = state.rules_entry(name) else {
            return Ok(false);
        };
        let path = self.config.rules_path(name);
        let up_to_date = std::fs::read(&path)
            .map(|existing| existing == entry.rendered.as_bytes())
            .unwrap_or(false);
        if up_to_date {
            return Ok(false);
        }
        crate::agent::write_private(&path, entry.rendered.as_bytes())?;
        Ok(true)
    }

    /// 绑定某条规则的全部环境名（热应用的影响面）。
    fn environments_bound_locked(&self, state: &PersistedState, rules: &str) -> Vec<String> {
        state
            .environments
            .iter()
            .filter_map(|raw| Environment::from_json(raw).ok())
            .filter(|environment| environment.rules() == Some(rules))
            .map(|environment| environment.name().to_string())
            .collect()
    }

    fn reconcile_rules_locked(&self, state: &mut PersistedState) -> Result<Vec<String>, Error> {
        let mut messages = Vec::new();

        // 1) 回填：物化目录里有、账本里没有的规则（升级迁移）。
        if let Ok(entries) = std::fs::read_dir(&self.config.rules_dir) {
            let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "rules")
                    && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
                    && envboard_engine::domain::validate_rules_name(stem).is_ok()
                    && state.rules_entry(stem).is_none()
                {
                    found.push((stem.to_string(), path));
                }
            }
            found.sort();
            for (name, path) in found {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let import = envboard_engine::rules::parse_hosts_text(&text);
                state.rules.push(StoredRules {
                    name: name.clone(),
                    source: None,
                    rendered: text,
                    entries: import.accepted(),
                    skipped: import.skipped.len(),
                    conflicts: import.conflicts.len(),
                    imported_at: self.clock.now_unix(),
                });
                messages.push(format!(
                    "rules ledger backfilled from {}: {name}",
                    path.display()
                ));
            }
            state
                .rules
                .sort_by(|left, right| left.name.cmp(&right.name));
        }

        // 2) 自愈：账本里有、物化文件缺失或被改坏。
        let names: Vec<String> = state.rules.iter().map(|entry| entry.name.clone()).collect();
        for name in names {
            if self.ensure_rules_file(state, &name)? {
                messages.push(format!("rules {name} was rebuilt from the ledger"));
            }
        }

        Ok(messages)
    }
}
