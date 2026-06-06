// ============================================================================
// 26 规则 YAML 加载器
// ============================================================================
//
// 把 hardcoded 规则搬到 persona_event_rules.yaml,符合"零魔法值"原则。
//
// 失败模式:fail-fast — 文件缺失 / YAML 损坏 / 空 rules / schema 不匹配 / 规则数 != 26
// 都返回 Result::Err,启动失败(无静默 fallback,符合创世哲学"快速失败")。
// 见计划书 §十六。
// ============================================================================

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fs;
use std::path::Path;

use super::event_mapper::{EventTraitMapper, EventType, TraitMappingRule};

#[derive(Debug, Deserialize)]
pub struct RulesFile {
    pub rules: Vec<RuleYaml>,
}

#[derive(Debug, Deserialize)]
pub struct RuleYaml {
    pub event_type: EventType,
    pub trait_name: String,
    pub base_delta: i16,
    pub weight: f32,
}

const EXPECTED_RULE_COUNT: usize = 26;

pub fn load_event_trait_rules(path: &Path) -> Result<EventTraitMapper> {
    if !path.exists() {
        bail!(
            "persona_event_rules.yaml 不存在: {} — 创世 26 规则必须显式提供(无静默 fallback)",
            path.display()
        );
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("读取 persona_event_rules.yaml 失败: {}", path.display()))?;
    let parsed: RulesFile = serde_yaml::from_str(&content)
        .with_context(|| format!("解析 persona_event_rules.yaml 失败: {}", path.display()))?;

    if parsed.rules.is_empty() {
        bail!("persona_event_rules.yaml `rules: []` — 必须至少 1 条规则(创世要求 base 行为表)");
    }

    if parsed.rules.len() != EXPECTED_RULE_COUNT {
        bail!(
            "persona_event_rules.yaml 规则数 {} != {} — 26 条是创世基线,若需调整请同步更新计划书 §十六",
            parsed.rules.len(),
            EXPECTED_RULE_COUNT
        );
    }

    for (i, rule) in parsed.rules.iter().enumerate() {
        if rule.trait_name.trim().is_empty() {
            bail!("第 {} 条规则 trait_name 为空", i + 1);
        }
        if !(-100..=100).contains(&rule.base_delta) {
            bail!(
                "第 {} 条规则 base_delta={} 超出 [-100, +100] 范围(trait_name={})",
                i + 1,
                rule.base_delta,
                rule.trait_name
            );
        }
        if rule.weight <= 0.0 || !rule.weight.is_finite() {
            bail!(
                "第 {} 条规则 weight={} 非法,必须 > 0 且有限(trait_name={})",
                i + 1,
                rule.weight,
                rule.trait_name
            );
        }
    }

    let trait_rules: Vec<TraitMappingRule> = parsed
        .rules
        .into_iter()
        .map(|r| TraitMappingRule {
            event_type: r.event_type,
            trait_name: r.trait_name,
            base_delta: r.base_delta,
            condition: None,
            weight: r.weight,
        })
        .collect();

    Ok(EventTraitMapper::from_rules(trait_rules))
}
