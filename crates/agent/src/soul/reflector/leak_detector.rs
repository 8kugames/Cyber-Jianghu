//! 数值泄露检测器
//!
//! 检测 NarrativeContext 中是否存在数值泄露（LLM 意外暴露原始游戏数值）。
//! 使用评分制：可疑关键词 + 高风险正则模式累加分数，超过阈值触发重试。

use cyber_jianghu_protocol::NarrativeContext;
use regex::Regex;
use std::sync::LazyLock;

/// 高风险正则模式
static HIGH_RISK_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // 百分比
        Regex::new(r"\d+%").unwrap(),
        // 量词+数字
        Regex::new(r"\d+\s*(步|米|个|件|名|人|两|斤)").unwrap(),
        // 序数
        Regex::new(r"第\d+").unwrap(),
        // 分数
        Regex::new(r"\d+/\d+").unwrap(),
    ]
});

/// 可疑关键词（这些词出现在叙事中暗示数值泄露）
const SUSPICION_KEYWORDS: &[&str] = &[
    "HP",
    "血量",
    "生命值",
    "体力值",
    "耐力",
    "饥饿度",
    "口渴度",
    "经验值",
    "等级",
    "攻击力",
    "防御力",
];

/// 泄露检测报告
#[derive(Debug)]
pub struct LeakReport {
    /// 风险分数（>=threshold 视为高风险）
    pub score: u8,
    /// 具体证据
    pub evidences: Vec<String>,
}

impl LeakReport {
    /// 是否为高风险
    pub fn is_high_risk(&self, threshold: u8) -> bool {
        self.score >= threshold
    }
}

/// 对抗性泄露检测器
pub struct LeakDetector {
    /// 高风险阈值（从配置读取）
    suspicion_threshold: u8,
}

impl LeakDetector {
    pub fn new(suspicion_threshold: u8) -> Self {
        Self {
            suspicion_threshold,
        }
    }

    /// 默认检测器（阈值 100）
    pub fn default_detector() -> Self {
        Self::new(100)
    }

    /// 对抗性检测 NarrativeContext 中的数值泄露
    pub fn detect_leaks(&self, ctx: &NarrativeContext) -> LeakReport {
        let mut score = 0u8;
        let mut evidences = Vec::new();

        let text_fields = [
            &ctx.self_perception.status_summary,
            &ctx.environment.location_description,
            &ctx.environment.ambient_features,
            &ctx.self_perception.inventory_narrative,
        ];

        for field in &text_fields {
            // 检测可疑关键词
            for keyword in SUSPICION_KEYWORDS {
                if field.contains(keyword) {
                    score = score.saturating_add(20);
                    evidences.push(format!("包含可疑关键词: {}", keyword));
                }
            }

            // 检测高风险正则模式
            for pattern in HIGH_RISK_PATTERNS.iter() {
                if let Some(mat) = pattern.find(field) {
                    score = score.saturating_add(30);
                    evidences.push(format!("匹配高风险模式: {}", mat.as_str()));
                }
            }
        }

        LeakReport { score, evidences }
    }

    /// 获取阈值
    pub fn threshold(&self) -> u8 {
        self.suspicion_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::*;

    fn make_ctx(status: &str, location: &str) -> NarrativeContext {
        NarrativeContext {
            tick_id: 1,
            self_perception: SelfPerception {
                status_summary: status.to_string(),
                notable_attributes: vec![],
                inventory_narrative: String::new(),
            },
            environment: EnvironmentPerception {
                location_description: location.to_string(),
                ambient_features: String::new(),
                interactive_elements: vec![],
                reachable_locations: vec![],
            },
            nearby_agents: vec![],
            recent_memories: vec![],
            last_outcome: None,
        }
    }

    #[test]
    fn test_no_leak() {
        let ctx = make_ctx("你感到有些饿了", "你身处龙门客栈");
        let detector = LeakDetector::new(100);
        let report = detector.detect_leaks(&ctx);
        assert!(!report.is_high_risk(100));
    }

    #[test]
    fn test_keyword_leak() {
        // 单个关键词 = 20 分，低于 100 阈值
        let ctx = make_ctx("你的饥饿度很低", "你身处客栈");
        let detector = LeakDetector::new(100);
        let report = detector.detect_leaks(&ctx);
        assert!(!report.is_high_risk(100), "单个关键词不应触发高风险: score={}", report.score);
        assert!(report.score > 0, "但应检测到泄露: score={}", report.score);
    }

    #[test]
    fn test_percentage_leak() {
        // "HP"(20) + "99%"(30) = 50 分，低于 100
        let ctx = make_ctx("你状态还行", "HP 99%");
        let detector = LeakDetector::new(100);
        let report = detector.detect_leaks(&ctx);
        assert!(!report.is_high_risk(100), "HP+百分比不应触发高风险: score={}", report.score);
        assert!(report.score >= 50, "应检测到多个泄露: score={}", report.score);
    }

    #[test]
    fn test_multi_leak_high_risk() {
        // 多个关键词 + 模式组合触发高风险
        let ctx = make_ctx(
            "你的饥饿度很高，体力值很低，经验值不足",
            "HP 30%，防御力只有 50/100",
        );
        let detector = LeakDetector::new(100);
        let report = detector.detect_leaks(&ctx);
        assert!(report.is_high_risk(100), "多泄露组合应触发高风险: score={}, evidences={:?}", report.score, report.evidences);
    }
}
