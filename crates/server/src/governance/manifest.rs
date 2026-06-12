use crate::game_data::registry::ActionRegistry;
use super::types::CapabilityEntry;

/// 能力注册表 — 从 ActionRegistry 投影
#[derive(Debug, Clone, Default)]
pub struct CapabilityManifest {
    entries: Vec<CapabilityEntry>,
}

impl CapabilityManifest {
    pub fn load() -> Self {
        let entries = ActionRegistry::all_action_names()
            .into_iter()
            .filter_map(|name| {
                let config = ActionRegistry::get(&name)?;
                Some(CapabilityEntry {
                    capability_id: name,
                    kind: "action".to_string(),
                    semantic_scope: config.category.clone(),
                })
            })
            .collect();
        Self { entries }
    }

    pub fn contains_effect_ref(&self, effect_ref: &str) -> bool {
        let action_name = effect_ref.split('.').next_back().unwrap_or(effect_ref);
        self.entries.iter().any(|e| e.capability_id == action_name)
    }

    pub fn all_ids(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.capability_id.clone()).collect()
    }

    pub fn entries(&self) -> &[CapabilityEntry] {
        &self.entries
    }
}
