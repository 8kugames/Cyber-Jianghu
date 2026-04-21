//! 规则注册表
//!
//! 管理所有验证规则的注册和查询。

use super::types::{Rule, RuleType};
use std::sync::Arc;
use tokio::sync::RwLock;

/// 规则集合
///
/// 按规则类型组织的一组规则
#[derive(Debug, Clone)]
pub struct RuleSet {
    /// 动作冷却规则
    pub cooldown_rules: Vec<Rule>,
    /// 资源约束规则
    pub resource_rules: Vec<Rule>,
    /// 状态限制规则
    pub state_rules: Vec<Rule>,
    /// 特质一致性规则
    pub trait_rules: Vec<Rule>,
    /// 数值范围规则
    pub range_rules: Vec<Rule>,
    /// 自定义规则
    pub custom_rules: Vec<Rule>,
}

impl RuleSet {
    /// 创建空的规则集合
    pub fn new() -> Self {
        Self {
            cooldown_rules: Vec::new(),
            resource_rules: Vec::new(),
            state_rules: Vec::new(),
            trait_rules: Vec::new(),
            range_rules: Vec::new(),
            custom_rules: Vec::new(),
        }
    }

    /// 添加规则
    pub fn add_rule(&mut self, rule: Rule) {
        match rule.rule_type {
            RuleType::ActionCooldown => self.cooldown_rules.push(rule),
            RuleType::ResourceConstraint => self.resource_rules.push(rule),
            RuleType::StateRestriction => self.state_rules.push(rule),
            RuleType::TraitConsistency => self.trait_rules.push(rule),
            RuleType::ValueRange => self.range_rules.push(rule),
            RuleType::Custom => self.custom_rules.push(rule),
        }
    }

    /// 获取指定类型的所有规则
    pub fn get_rules_by_type(&self, rule_type: RuleType) -> &[Rule] {
        match rule_type {
            RuleType::ActionCooldown => &self.cooldown_rules,
            RuleType::ResourceConstraint => &self.resource_rules,
            RuleType::StateRestriction => &self.state_rules,
            RuleType::TraitConsistency => &self.trait_rules,
            RuleType::ValueRange => &self.range_rules,
            RuleType::Custom => &self.custom_rules,
        }
    }

    /// 获取所有启用的规则
    pub fn all_enabled(&self) -> Vec<&Rule> {
        let rule_arrays = [
            &self.cooldown_rules,
            &self.resource_rules,
            &self.state_rules,
            &self.trait_rules,
            &self.range_rules,
            &self.custom_rules,
        ];

        rule_arrays
            .iter()
            .flat_map(|rules| rules.iter().filter(|r| r.enabled))
            .collect()
    }
}

impl Default for RuleSet {
    fn default() -> Self {
        Self::new()
    }
}

/// 规则注册表
///
/// 线程安全的规则存储和查询
pub struct RuleRegistry {
    inner: Arc<RwLock<RuleSet>>,
}

impl RuleRegistry {
    /// 创建新的规则注册表
    pub fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(RuleSet::new())),
        }
    }

    /// 从已有的规则集合创建注册表
    pub fn from_rule_set(rule_set: RuleSet) -> Self {
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(rule_set)),
        }
    }

    /// 注册规则
    pub async fn register(&self, rule: Rule) {
        let mut set = self.inner.write().await;
        set.add_rule(rule);
    }

    /// 批量注册规则
    pub async fn register_all(&self, rules: Vec<Rule>) {
        let mut set = self.inner.write().await;
        for rule in rules {
            set.add_rule(rule);
        }
    }

    /// 获取指定类型的规则
    pub async fn get_by_type(&self, rule_type: RuleType) -> Vec<Rule> {
        let set = self.inner.read().await;
        set.get_rules_by_type(rule_type).to_vec()
    }

    /// 获取所有启用的规则
    pub async fn all_enabled(&self) -> Vec<Rule> {
        let set = self.inner.read().await;
        set.all_enabled().into_iter().cloned().collect()
    }

    /// 从配置加载规则（未来扩展）
    pub async fn load_from_config(&self, _path: &std::path::Path) -> anyhow::Result<()> {
        // Phase 2: 实现 YAML 配置加载
        // 当前使用硬编码规则
        tracing::warn!("load_from_config 尚未实现，使用硬编码规则");
        Ok(())
    }
}

impl Default for RuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Clone 实现（因为 Arc<RwLock> 不直接支持 Clone）
impl Clone for RuleRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::RuleCondition;
    use super::*;

    #[tokio::test]
    async fn test_registry_register() {
        let registry = RuleRegistry::new();
        let rule = Rule::new(
            "test1".to_string(),
            "测试规则".to_string(),
            RuleType::ActionCooldown,
            RuleCondition::Equals("action".to_string(), serde_json::json!("说话")),
            "测试错误".to_string(),
        );

        registry.register(rule).await;
        let rules = registry.all_enabled().await;
        assert_eq!(rules.len(), 1);
    }

    #[tokio::test]
    async fn test_registry_get_by_type() {
        let registry = RuleRegistry::new();
        let rule = Rule::new(
            "test1".to_string(),
            "测试规则".to_string(),
            RuleType::ActionCooldown,
            RuleCondition::Equals("action".to_string(), serde_json::json!("说话")),
            "测试错误".to_string(),
        );

        registry.register(rule).await;
        let rules = registry.get_by_type(RuleType::ActionCooldown).await;
        assert_eq!(rules.len(), 1);
    }
}
