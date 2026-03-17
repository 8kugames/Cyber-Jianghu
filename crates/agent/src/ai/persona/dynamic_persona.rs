// ============================================================================
// 动态人设核心
// ============================================================================
//
// 实现运行时可修改的人设系统
//
// 核心功能:
// - 从静态配置加载人设
// - 运行时修改特质值
// - 生成叙事化的人设描述
// - 追踪人设演化历史
// ============================================================================

use crate::ai::prompts::AgentPrompt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

use super::trait_types::{Trait, TraitChange, TraitType};

/// 人设状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaState {
    /// 当前情绪
    pub current_emotion: String,
    /// 当前目标
    pub current_goal: Option<String>,
    /// 当前压力值 (0-100)
    pub stress_level: u8,
    /// 上次更新时间
    pub last_updated: i64,
}

impl Default for PersonaState {
    fn default() -> Self {
        Self {
            current_emotion: "平静".to_string(),
            current_goal: None,
            stress_level: 0,
            last_updated: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        }
    }
}

/// 动态人设
///
/// 支持运行时修改的人设系统，特质可以随事件动态变化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicPersona {
    /// Agent ID
    pub agent_id: String,
    /// Agent 名称
    pub name: String,
    /// 基础描述（静态部分）
    pub base_description: String,
    /// 动态特质（可运行时修改）
    pub traits: HashMap<String, Trait>,
    /// 当前状态
    pub current_state: PersonaState,
    /// 人设版本（用于追踪演化）
    pub version: u32,
}

impl DynamicPersona {
    /// 从预设的 AgentPrompt 创建动态人设
    pub fn from_preset(agent_id: Uuid, preset: &AgentPrompt) -> Self {
        let traits = Trait::parse_from_prompt(preset.system_prompt, preset.name);

        Self {
            agent_id: agent_id.to_string(),
            name: preset.name.to_string(),
            base_description: preset.system_prompt.to_string(),
            traits,
            current_state: PersonaState::default(),
            version: 1,
        }
    }

    /// 使用默认配置创建
    pub fn new(agent_id: Uuid, name: &str, description: &str) -> Self {
        let traits = Trait::parse_from_prompt(description, name);

        Self {
            agent_id: agent_id.to_string(),
            name: name.to_string(),
            base_description: description.to_string(),
            traits,
            current_state: PersonaState::default(),
            version: 1,
        }
    }

    /// 应用特质变化
    ///
    /// # 参数
    /// - `trait_name`: 特质名称
    /// - `delta`: 变化量（正数增加，负数减少）
    /// - `reason`: 变化原因
    /// - `tick_id`: 当前 Tick ID
    pub fn apply_trait_change(
        &mut self,
        trait_name: &str,
        delta: i16,
        reason: String,
        tick_id: i64,
    ) {
        if let Some(trait_obj) = self.traits.get_mut(trait_name) {
            let change = TraitChange::new(trait_name.to_string(), delta, reason, tick_id);
            trait_obj.apply_change(change, tick_id);
            self.version += 1;
        } else {
            // 特质不存在时自动创建
            let mut new_trait = Trait::new(trait_name.to_string(), TraitType::Social, 50);
            let change = TraitChange::new(trait_name.to_string(), delta, reason, tick_id);
            new_trait.apply_change(change, tick_id);
            self.traits.insert(trait_name.to_string(), new_trait);
            self.version += 1;
        }
    }

    /// 获取特质值
    pub fn get_trait(&self, trait_name: &str) -> Option<u8> {
        self.traits.get(trait_name).map(|t| t.value())
    }

    /// 设置特质值（直接设置，不记录历史）
    pub fn set_trait(&mut self, trait_name: &str, value: u8) {
        if let Some(trait_obj) = self.traits.get_mut(trait_name) {
            trait_obj.value = value.clamp(0, 100);
            self.version += 1;
        } else {
            let trait_type = match trait_name {
                "信任" | "友善" | "攻击性" => TraitType::Social,
                "贪婪" | "诚实" | "正义感" => TraitType::Moral,
                "勇敢" | "智慧" | "机敏" => TraitType::Capability,
                "愤怒" | "恐惧" | "喜悦" => TraitType::Emotional,
                _ => TraitType::Survival,
            };
            let mut new_trait = Trait::new(trait_name.to_string(), trait_type, value);
            new_trait.value = value.clamp(0, 100);
            self.traits.insert(trait_name.to_string(), new_trait);
            self.version += 1;
        }
    }

    /// 生成叙事化的人设描述
    ///
    /// 结合静态描述和动态特质，生成当前人设的完整描述
    pub fn generate_description(&self) -> String {
        let mut description = format!("# {}\n\n", self.name);
        description.push_str(&self.base_description);
        description.push_str("\n\n");

        // 添加当前特质状态
        description.push_str("# 当前性格特质\n\n");

        for (name, trait_obj) in &self.traits {
            let narrative = trait_obj.narrative_description();
            description.push_str(&format!("- {}: {}\n", name, narrative));
        }

        // 添加当前状态
        description.push_str(&format!("\n# 当前状态\n\n"));
        description.push_str(&format!("情绪: {}\n", self.current_state.current_emotion));
        if let Some(ref goal) = self.current_state.current_goal {
            description.push_str(&format!("当前目标: {}\n", goal));
        }
        if self.current_state.stress_level > 50 {
            description.push_str(&format!("压力水平: 高 ({}%)\n", self.current_state.stress_level));
        }

        description
    }

    /// 更新情绪状态
    pub fn update_emotion(&mut self, emotion: String) {
        self.current_state.current_emotion = emotion;
        self.current_state.last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
    }

    /// 设置当前目标
    pub fn set_goal(&mut self, goal: String) {
        self.current_state.current_goal = Some(goal);
        self.current_state.last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
    }

    /// 应用所有特质的衰减（每 Tick 调用）
    pub fn apply_all_decay(&mut self) {
        for trait_obj in self.traits.values_mut() {
            trait_obj.apply_decay();
        }
        self.current_state.last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
    }

    /// 检查人设是否一致（用于测试）
    pub fn is_consistent(&self) -> bool {
        // 检查特质值是否在合理范围内
        for trait_obj in self.traits.values() {
            if trait_obj.value > 100 {
                return false;
            }
        }
        true
    }
}

/// 线程安全的动态人设包装
///
/// 使用 RwLock 支持多线程访问
#[derive(Debug, Clone)]
pub struct ThreadSafePersona {
    inner: Arc<RwLock<DynamicPersona>>,
}

impl ThreadSafePersona {
    /// 从动态人设创建线程安全版本
    pub fn new(persona: DynamicPersona) -> Self {
        Self {
            inner: Arc::new(RwLock::new(persona)),
        }
    }

    /// 读取人设
    pub fn read<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&DynamicPersona) -> R,
    {
        let guard = self.inner.read().unwrap();
        f(&*guard)
    }

    /// 修改人设
    pub fn write<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut DynamicPersona) -> R,
    {
        let mut guard = self.inner.write().unwrap();
        f(&mut *guard)
    }

    /// 获取克隆的人设
    pub fn clone_persona(&self) -> DynamicPersona {
        let guard = self.inner.read().unwrap();
        (*guard).clone()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts;

    #[test]
    fn test_persona_from_preset() {
        let agent_id = Uuid::new_v4();
        let preset = prompts::liu_yunnang();
        let persona = DynamicPersona::from_preset(agent_id, &preset);

        assert_eq!(persona.name, "柳云娘");
        assert!(persona.traits.contains_key("贪婪"));
        assert!(persona.traits.contains_key("信誉"));
    }

    #[test]
    fn test_trait_change() {
        let agent_id = Uuid::new_v4();
        let mut persona = DynamicPersona::new(
            agent_id,
            "测试角色",
            "这是一个测试角色",
        );

        // 设置初始值
        persona.set_trait("信任", 50);
        assert_eq!(persona.get_trait("信任"), Some(50));

        // 应用变化
        persona.apply_trait_change("信任", -10, "被骗了".to_string(), 100);
        assert_eq!(persona.get_trait("信任"), Some(40));

        // 验证人设版本增加
        assert_eq!(persona.version, 3); // new + set_trait + apply_trait_change
    }

    #[test]
    fn test_narrative_description() {
        let agent_id = Uuid::new_v4();
        let mut persona = DynamicPersona::new(
            agent_id,
            "测试角色",
            "基础描述",
        );

        persona.set_trait("勇敢", 75);
        persona.set_trait("贪婪", 30);

        let description = persona.generate_description();
        assert!(description.contains("测试角色"));
        assert!(description.contains("勇敢"));
        assert!(description.contains("贪婪"));
    }

    #[test]
    fn test_persona_consistency() {
        let agent_id = Uuid::new_v4();
        let mut persona = DynamicPersona::new(
            agent_id,
            "测试角色",
            "基础描述",
        );

        assert!(persona.is_consistent());

        // 人设应该是一致的
        persona.set_trait("测试", 50);
        assert!(persona.is_consistent());
    }

    #[test]
    fn test_thread_safe_persona() {
        let agent_id = Uuid::new_v4();
        let persona = DynamicPersona::new(
            agent_id,
            "测试角色",
            "基础描述",
        );

        let safe_persona = ThreadSafePersona::new(persona);

        // 测试读取
        let name = safe_persona.read(|p| p.name.clone());
        assert_eq!(name, "测试角色");

        // 测试写入
        safe_persona.write(|p| {
            p.set_trait("勇敢", 80);
        });

        let brave_value = safe_persona.read(|p| p.get_trait("勇敢"));
        assert_eq!(brave_value, Some(80));
    }
}
