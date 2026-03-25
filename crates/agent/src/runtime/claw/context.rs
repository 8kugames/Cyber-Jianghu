// Context Builder - 构建 LLM 调用上下文
//
// 将 WorldState 转换为 LLM 可理解的 prompt
//
// KISS 原则：不生成完整 markdown，只生成简洁的结构化文本

use crate::models::WorldState;

// ============================================================================
// 常量
// ============================================================================

const INDENT: &str = "  ";
const MAX_ENTITIES_SHOWN: usize = 5;
const MAX_RECENT_EVENTS: usize = 3;

// ============================================================================
// Context Builder
// ============================================================================

pub struct ContextBuilder;

impl ContextBuilder {
    pub fn new() -> Self {
        Self
    }

    pub fn build(&self, state: &WorldState) -> String {
        let mut parts = Vec::new();

        // Header
        parts.push(format!(
            "Tick {} | {}",
            state.tick_id,
            self.format_time(&state.world_time)
        ));

        // Location
        parts.push(format!(
            "At: {} ({})",
            state.location.name, state.location.node_type
        ));

        // Self State
        parts.push(self.format_self_state(&state.self_state));

        // Nearby Entities (limited)
        if !state.entities.is_empty() {
            let shown = state.entities.len().min(MAX_ENTITIES_SHOWN);
            parts.push(format!("Nearby: {} entities visible", state.entities.len()));
            for entity in state.entities.iter().take(shown) {
                parts.push(format!("  - {} ({})", entity.name, entity.state));
            }
            if state.entities.len() > shown {
                parts.push(format!("  ... and {} more", state.entities.len() - shown));
            }
        }

        // Recent Events (limited)
        if !state.events_log.is_empty() {
            parts.push("Recent Events:".to_string());
            for event in state.events_log.iter().rev().take(MAX_RECENT_EVENTS).rev() {
                parts.push(format!("  - {}", event.description));
            }
        }

        // Available Actions
        if !state.available_actions.is_empty() {
            let actions: Vec<_> = state
                .available_actions
                .iter()
                .take(5)
                .map(|a| &a.action)
                .collect();
            parts.push(format!("Available: {}", actions.join(", ")));
        }

        parts.join("\n")
    }

    fn format_time(&self, time: &crate::models::WorldTime) -> String {
        format!(
            "Year {} Month {} Day {} Hour {}",
            time.year, time.month, time.day, time.hour
        )
    }

    fn format_self_state(&self, state: &crate::models::AgentSelfState) -> String {
        let mut parts = Vec::new();

        // Key attributes
        if let Some(&hp) = state.attributes.get("hp") {
            parts.push(format!("HP: {}", hp));
        }
        if let Some(&stamina) = state.attributes.get("stamina") {
            parts.push(format!("Stamina: {}", stamina));
        }
        if let Some(&hunger) = state.attributes.get("hunger") {
            parts.push(format!("Hunger: {}", hunger));
        }

        // Status effects
        if !state.status_effects.is_empty() {
            parts.push(format!("Effects: {}", state.status_effects.join(", ")));
        }

        // Inventory summary
        if !state.inventory.is_empty() {
            parts.push(format!("Items: {} items", state.inventory.len()));
        }

        parts.join(" | ")
    }
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_builder_basic() {
        let state = WorldState::default();
        let builder = ContextBuilder::new();

        let context = builder.build(&state);

        assert!(!context.is_empty());
        assert!(context.contains("Tick"));
    }

    #[test]
    fn test_max_entities_limit() {
        use crate::models::{Entity, Location, WorldTime};

        let mut state = WorldState::default();
        state.location = Location {
            node_id: "test".to_string(),
            name: "Test Location".to_string(),
            node_type: "indoor".to_string(),
            adjacent_nodes: vec![],
        };
        state.world_time = WorldTime {
            year: 1,
            month: 1,
            day: 1,
            hour: 12,
            minute: 0,
            second: 0,
            weather: "sunny".to_string(),
        };

        // Add many entities
        for i in 0..10 {
            state.entities.push(Entity {
                id: uuid::Uuid::new_v4(),
                name: format!("NPC {}", i),
                distance: 0,
                state: "alive".to_string(),
                hostile: false,
            });
        }

        let builder = ContextBuilder::new();
        let context = builder.build(&state);

        // Should show max 5 and mention "and 5 more"
        assert!(context.contains("Nearby: 10 entities visible"));
        assert!(context.contains("... and 5 more"));
    }
}
