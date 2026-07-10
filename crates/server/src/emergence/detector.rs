// ============================================================================
// 涌现检测纯函数层（零 DB 依赖，可单测）
// ============================================================================
//
// 移植自 scripts/detect_emergence.py 的纯函数。两阶段检测：
//   阶段 1 形态筛选：按 (tick_id, node_id) 聚类时空簇，阈值判定候选事件。
//   阶段 2 因果验证：验证 agent 间"感知→处理→定向回应"闭环，
//                    区分 causal_emergence（真因果）与 co_occurrence（仅共现/存疑）。
//
// 核心不变量：同输入 → 同输出。无随机/LLM/时钟依赖。所有输出 sorted。
//
// 设计原则：判定逻辑零硬编码动作名——全部来自 EmergenceConfig（emergence.yaml）。
// ============================================================================

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::{CausalConfig, CategoryRule, DetectionConfig, EmergenceConfig};

// ============================================================================
// 数据模型
// ============================================================================

/// agent_action_logs 单行投影（经 SQL 查询提取，见 loader.rs）
#[derive(Debug, Clone)]
pub struct ActionRow {
    pub tick_id: i64,
    pub agent_id: Uuid,
    pub pipe_seq: i32,
    pub action_type: String,
    pub result: String,
    pub action_data: serde_json::Value,
    /// COALESCE(顶层 thought_log, metadata 嵌套 thought_log)
    pub thought_text: Option<String>,
    /// LEFT JOIN agent_states，可能为 None（缺快照）
    pub node_id: Option<String>,
}

/// 一个检测到的事件链
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergenceEvent {
    /// "causal_emergence" | "co_occurrence"
    pub category: String,
    pub tick_start: i64,
    pub tick_end: i64,
    /// 参与者 agent_id（已排序、去重）
    pub participants: Vec<Uuid>,
    pub action_count: usize,
    /// 覆盖的社会类别（已排序）
    pub categories_covered: Vec<String>,
    /// 验证通过的因果边
    pub causal_edges: Vec<CausalEdge>,
    /// 构成事件的动作摘要（已排序 by tick_id, agent_id, pipe_seq）
    pub actions: Vec<ActionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalEdge {
    pub from_agent: Uuid,
    pub to_agent: Uuid,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSummary {
    pub tick_id: i64,
    pub agent_id: Uuid,
    pub action_type: String,
    pub result: String,
}

// ============================================================================
// 阶段 1：形态筛选
// ============================================================================

/// 将单条动作投射到社会类别。返回类别名或 None（非社会动作）。
///
/// 判定逻辑完全由 `category_rules` 配置驱动，零硬编码动作名。
pub fn classify_category(
    action_type: &str,
    result: &str,
    action_data: &serde_json::Value,
    category_rules: &BTreeMap<String, CategoryRule>,
) -> Option<String> {
    for (cat_name, rule) in category_rules {
        match rule {
            CategoryRule::Actions {
                actions,
                require_success,
                require_target,
            } => {
                if !actions.iter().any(|a| a == action_type) {
                    continue;
                }
                if *require_success && result != "success" {
                    continue;
                }
                if *require_target {
                    let tgt = action_data.get("target_agent_id").and_then(|v| v.as_str());
                    if tgt.is_none() || tgt == Some("") || tgt == Some("null") {
                        continue;
                    }
                }
                return Some(cat_name.clone());
            }
            CategoryRule::TransferActions { transfer_actions } => {
                let mut matched = false;
                for spec in transfer_actions {
                    if action_type != spec.action {
                        continue;
                    }
                    let field_val = action_data
                        .get(&spec.direction_field)
                        .and_then(|v| v.as_str());
                    if field_val == Some(spec.direction_value.as_str()) {
                        matched = true;
                        break;
                    }
                }
                if matched {
                    return Some(cat_name.clone());
                }
            }
        }
    }
    None
}

/// 提取该动作的社会伙伴 agent_id（target/recipient/source），用于因果边判定。
///
/// 通用探测，不硬编码动作语义。
pub fn extract_partner_id(action_data: &serde_json::Value) -> Option<Uuid> {
    for key in &["target_agent_id", "recipient_id", "source_id"] {
        let val = match action_data.get(*key).and_then(|v| v.as_str()) {
            Some(v) if !v.is_empty() && v != "null" => v,
            _ => continue,
        };
        if let Ok(uuid) = Uuid::parse_str(val) {
            return Some(uuid);
        }
    }
    None
}

/// 按 (tick_id, node_id) 聚类。node_id 为 None 的行不参与聚类。
///
/// 返回 BTreeMap 保证 key 有序（确定性）。
pub fn cluster_by_spacetime(rows: &[ActionRow]) -> BTreeMap<(i64, String), Vec<ActionRow>> {
    let mut clusters: BTreeMap<(i64, String), Vec<ActionRow>> = BTreeMap::new();
    for r in rows {
        if let Some(node) = &r.node_id {
            clusters.entry((r.tick_id, node.clone())).or_default().push(r.clone());
        }
    }
    clusters
}

/// 判定一个时空簇是否通过 MVP §6.1.3 阈值。
///
/// agent 去重用 DISTINCT（处理同 agent 多 pipe_seq）。
pub fn cluster_passes_threshold(
    cluster_rows: &[ActionRow],
    category_rules: &BTreeMap<String, CategoryRule>,
    min_agents: usize,
    min_actions: usize,
    min_categories: usize,
) -> (bool, Vec<Uuid>, Vec<String>) {
    let distinct_agents: BTreeSet<Uuid> = cluster_rows.iter().map(|r| r.agent_id).collect();
    let distinct_sorted: Vec<Uuid> = distinct_agents.iter().copied().collect();

    if distinct_sorted.len() < min_agents {
        return (false, distinct_sorted, vec![]);
    }
    if cluster_rows.len() < min_actions {
        return (false, distinct_sorted, vec![]);
    }

    let mut categories: BTreeSet<String> = BTreeSet::new();
    for r in cluster_rows {
        if let Some(cat) = classify_category(
            &r.action_type,
            &r.result,
            &r.action_data,
            category_rules,
        ) {
            categories.insert(cat);
        }
    }
    let cats_sorted: Vec<String> = categories.iter().cloned().collect();
    if cats_sorted.len() < min_categories {
        return (false, distinct_sorted, cats_sorted);
    }
    (true, distinct_sorted, cats_sorted)
}

/// 合格簇（用于链合并的中间结构）
struct QualifiedCluster {
    tick_id: i64,
    rows: Vec<ActionRow>,
    participants: Vec<Uuid>,
    categories: Vec<String>,
}

/// 相邻 tick（间隔 ≤ chain_gap_ticks）的合格簇合并为事件链。
fn merge_chains(qualified: Vec<QualifiedCluster>, chain_gap_ticks: i64) -> Vec<Chain> {
    if qualified.is_empty() {
        return vec![];
    }
    // 按 tick_id 排序保证确定性（qualified 已按 BTreeMap 顺序，但显式排序更安全）
    let mut sorted = qualified;
    sorted.sort_by_key(|c| c.tick_id);

    let mut chains: Vec<Chain> = vec![];
    let mut current: Option<Chain> = None;
    for cluster in sorted {
        if let Some(cur) = current.as_mut() {
            if cluster.tick_id - cur.tick_end <= chain_gap_ticks {
                // 合并
                cur.rows.extend(cluster.rows);
                let mut parts: BTreeSet<Uuid> = cur.participants.iter().copied().collect();
                parts.extend(cluster.participants);
                cur.participants = parts.into_iter().collect();
                let mut cats: BTreeSet<String> = cur.categories.iter().cloned().collect();
                cats.extend(cluster.categories);
                cur.categories = cats.into_iter().collect();
                cur.tick_end = cluster.tick_id;
                continue;
            }
            chains.push(current.unwrap());
        }
        current = Some(Chain {
            tick_start: cluster.tick_id,
            tick_end: cluster.tick_id,
            rows: cluster.rows,
            participants: cluster.participants,
            categories: cluster.categories,
        });
    }
    if let Some(c) = current {
        chains.push(c);
    }
    chains
}

struct Chain {
    tick_start: i64,
    tick_end: i64,
    rows: Vec<ActionRow>,
    participants: Vec<Uuid>,
    categories: Vec<String>,
}

// ============================================================================
// 阶段 2：因果验证
// ============================================================================

/// 构造在 thought_text 中匹配该 agent 的 token 列表。
fn match_tokens(name: Option<&str>, agent_id: Uuid, short_uuid_len: usize) -> Vec<String> {
    let mut tokens: Vec<String> = vec![];
    if let Some(n) = name.filter(|s| !s.is_empty()) {
        tokens.push(n.to_string());
    }
    let id_str = agent_id.to_string();
    // UUID 前 N 位（含连字符前段）
    tokens.push(id_str.chars().take(short_uuid_len).collect());
    // 去连字符前 N 位
    let no_hyphen: String = id_str.chars().filter(|c| *c != '-').collect();
    tokens.push(no_hyphen.chars().take(short_uuid_len).collect());
    tokens
}

/// 验证链内 agent 间的因果边（感知→处理→定向回应闭环）。
///
/// 感知=同簇（已由聚类保证同 node）；处理=thought_text 匹配对方；回应=partner_id 指向对方。
fn verify_causal_edges(
    chain: &Chain,
    agent_names: &HashMap<Uuid, String>,
    match_by: &[String],
    short_uuid_len: usize,
) -> Vec<CausalEdge> {
    let participants: BTreeSet<Uuid> = chain.participants.iter().copied().collect();

    // 索引：每个 agent 在该链的动作
    let mut agent_actions: BTreeMap<Uuid, Vec<&ActionRow>> = BTreeMap::new();
    for r in &chain.rows {
        agent_actions.entry(r.agent_id).or_default().push(r);
    }

    let do_match = match_by.iter().any(|m| m == "name" || m == "short_uuid");
    if !do_match {
        return vec![];
    }

    let mut edges: Vec<CausalEdge> = vec![];
    for actor_b in participants.iter() {
        let b_actions = match agent_actions.get(actor_b) {
            Some(a) => a,
            None => continue,
        };
        // 排序保证确定性
        let mut sorted_actions: Vec<&ActionRow> = b_actions.clone();
        sorted_actions.sort_by_key(|r| (r.tick_id, r.agent_id, r.pipe_seq));

        for r_b in sorted_actions {
            let thought = match &r_b.thought_text {
                Some(t) if !t.is_empty() => t,
                _ => continue, // thought 为空 → 因果无法验证，跳过
            };
            let partner = match extract_partner_id(&r_b.action_data) {
                Some(p) => p,
                None => continue,
            };
            if partner == *actor_b || !participants.contains(&partner) {
                continue;
            }
            // B 的动作指向 partner(A)，且 B 的 thought 提及 A
            let tokens = match_tokens(agent_names.get(&partner).map(|s| s.as_str()), partner, short_uuid_len);
            let matched = tokens.iter().any(|tok| !tok.is_empty() && thought.contains(tok));
            if matched {
                edges.push(CausalEdge {
                    from_agent: *actor_b,
                    to_agent: partner,
                    evidence: "thought提及+target指向".to_string(),
                });
            }
        }
    }
    // 去重（同 from→to 多条动作只记代表）
    let mut seen: BTreeSet<(Uuid, Uuid)> = BTreeSet::new();
    let mut unique: Vec<CausalEdge> = vec![];
    for e in edges {
        let key = (e.from_agent, e.to_agent);
        if seen.insert(key) {
            unique.push(e);
        }
    }
    unique
}

// ============================================================================
// 主检测入口
// ============================================================================

/// 两阶段检测主逻辑（纯函数，不碰 DB）。
///
/// 返回 (事件列表, 候选总数)。
pub fn run_detection(
    rows: &[ActionRow],
    agent_names: &HashMap<Uuid, String>,
    cfg: &EmergenceConfig,
) -> (Vec<EmergenceEvent>, usize) {
    let det: &DetectionConfig = &cfg.detection;
    let cat_rules = &det.category_rules;
    let causal_cfg: &CausalConfig = &cfg.causal;

    // 阶段 1：形态筛选
    let clusters = cluster_by_spacetime(rows);
    let mut qualified: Vec<QualifiedCluster> = vec![];
    for ((tick_id, _node_id), c_rows) in &clusters {
        let (ok, parts, cats) = cluster_passes_threshold(
            c_rows,
            cat_rules,
            det.min_agents,
            det.min_actions,
            det.min_categories,
        );
        if ok {
            qualified.push(QualifiedCluster {
                tick_id: *tick_id,
                rows: c_rows.clone(),
                participants: parts,
                categories: cats,
            });
        }
    }
    let candidate_count = qualified.len();
    let chains = merge_chains(qualified, det.chain_gap_ticks);

    // 阶段 2：因果验证
    let mut events: Vec<EmergenceEvent> = chains
        .into_iter()
        .map(|chain| {
            let edges = verify_causal_edges(
                &chain,
                agent_names,
                &causal_cfg.match_by,
                causal_cfg.short_uuid_len,
            );
            let is_causal = edges.len() >= causal_cfg.min_causal_edges;
            let mut chain_rows = chain.rows;
            chain_rows.sort_by(|a, b| {
                (a.tick_id, a.agent_id, a.pipe_seq).cmp(&(b.tick_id, b.agent_id, b.pipe_seq))
            });
            let actions: Vec<ActionSummary> = chain_rows
                .iter()
                .map(|r| ActionSummary {
                    tick_id: r.tick_id,
                    agent_id: r.agent_id,
                    action_type: r.action_type.clone(),
                    result: r.result.clone(),
                })
                .collect();
            EmergenceEvent {
                category: if is_causal {
                    "causal_emergence"
                } else {
                    "co_occurrence"
                }
                .to_string(),
                tick_start: chain.tick_start,
                tick_end: chain.tick_end,
                participants: chain.participants,
                action_count: actions.len(),
                categories_covered: chain.categories,
                causal_edges: edges,
                actions,
            }
        })
        .collect();

    // 排序：causal 优先，再按 tick_start；限量
    events.sort_by(|a, b| {
        let ra = if a.category == "causal_emergence" { 0 } else { 1 };
        let rb = if b.category == "causal_emergence" { 0 } else { 1 };
        (ra, a.tick_start).cmp(&(rb, b.tick_start))
    });
    events.truncate(det.max_events);
    (events, candidate_count)
}

// ============================================================================
// 单元测试（移植自 scripts/test_detect_emergence.py）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emergence::config::{
        CausalConfig, CategoryRule, DetectionConfig, EmergenceConfig, HealthConfig, TransferSpec,
    };
    use serde_json::json;

    fn test_config() -> EmergenceConfig {
        let mut category_rules = BTreeMap::new();
        category_rules.insert(
            "conflict".to_string(),
            CategoryRule::Actions {
                actions: vec!["攻击".to_string()],
                require_success: true,
                require_target: true,
            },
        );
        category_rules.insert(
            "trade".to_string(),
            CategoryRule::TransferActions {
                transfer_actions: vec![
                    TransferSpec {
                        action: "予".to_string(),
                        direction_field: "recipient_type".to_string(),
                        direction_value: "agent".to_string(),
                    },
                    TransferSpec {
                        action: "取".to_string(),
                        direction_field: "source_type".to_string(),
                        direction_value: "agent".to_string(),
                    },
                ],
            },
        );
        category_rules.insert(
            "communication".to_string(),
            CategoryRule::Actions {
                actions: vec!["说话".to_string()],
                require_success: true,
                require_target: false,
            },
        );
        EmergenceConfig {
            version: "1.0".to_string(),
            detection: DetectionConfig {
                category_rules,
                min_agents: 2,
                min_actions: 3,
                min_categories: 2,
                chain_gap_ticks: 5,
                max_events: 50,
            },
            causal: CausalConfig {
                min_causal_edges: 1,
                match_by: vec!["name".to_string(), "short_uuid".to_string()],
                short_uuid_len: 8,
            },
            health: HealthConfig {
                supply_actions: vec!["用".to_string()],
                min_survivors: 3,
                min_supply_count: 3,
            },
        }
    }

    fn agent_a() -> Uuid {
        Uuid::parse_str("aaaaaaaa-0000-0000-0000-000000000001").unwrap()
    }
    fn agent_b() -> Uuid {
        Uuid::parse_str("bbbbbbbb-0000-0000-0000-000000000002").unwrap()
    }

    fn row(
        tick: i64,
        agent: Uuid,
        action: &str,
        data: serde_json::Value,
        thought: Option<&str>,
    ) -> ActionRow {
        ActionRow {
            tick_id: tick,
            agent_id: agent,
            pipe_seq: 0,
            action_type: action.to_string(),
            result: "success".to_string(),
            action_data: data,
            thought_text: thought.map(|s| s.to_string()),
            node_id: Some("龙门大堂".to_string()),
        }
    }

    fn names() -> HashMap<Uuid, String> {
        let mut m = HashMap::new();
        m.insert(agent_a(), "燕无归".to_string());
        m.insert(agent_b(), "钱三通".to_string());
        m
    }

    // AC3: 因果涌现判定
    #[test]
    fn test_causal_emergence_detected() {
        let rows = vec![
            row(100, agent_a(), "攻击", json!({"target_agent_id": agent_b()}), None),
            row(100, agent_b(), "攻击", json!({"target_agent_id": agent_a()}), Some("燕无归此人伤我，当还以颜色")),
            row(100, agent_a(), "予", json!({"recipient_type":"agent","recipient_id":agent_b(),"item_id":"x","quantity":1}), None),
        ];
        let cfg = test_config();
        let (events, cand) = run_detection(&rows, &names(), &cfg);
        assert_eq!(cand, 1);
        let causal: Vec<_> = events.iter().filter(|e| e.category == "causal_emergence").collect();
        assert_eq!(causal.len(), 1);
        assert!(!causal[0].causal_edges.is_empty());
    }

    // AC4: 虚假叙事排除
    #[test]
    fn test_co_occurrence_when_no_causal() {
        let rows = vec![
            row(100, agent_a(), "攻击", json!({"target_agent_id": agent_b()}), None),
            row(100, agent_b(), "说话", json!({}), Some("今天天气不错")),
            row(100, agent_a(), "予", json!({"recipient_type":"agent","recipient_id":agent_b(),"item_id":"x","quantity":1}), None),
        ];
        let cfg = test_config();
        let (events, cand) = run_detection(&rows, &names(), &cfg);
        assert_eq!(cand, 1);
        let causal: Vec<_> = events.iter().filter(|e| e.category == "causal_emergence").collect();
        let co: Vec<_> = events.iter().filter(|e| e.category == "co_occurrence").collect();
        assert!(causal.is_empty());
        assert_eq!(co.len(), 1);
    }

    // AC5: 确定性不变量
    #[test]
    fn test_deterministic() {
        let rows = vec![
            row(100, agent_a(), "攻击", json!({"target_agent_id": agent_b()}), None),
            row(100, agent_b(), "攻击", json!({"target_agent_id": agent_a()}), Some("燕无归伤我")),
            row(101, agent_a(), "说话", json!({}), Some("钱三通此人")),
        ];
        let cfg = test_config();
        let (e1, _) = run_detection(&rows, &names(), &cfg);
        let (e2, _) = run_detection(&rows, &names(), &cfg);
        assert_eq!(e1.len(), e2.len());
        for (a, b) in e1.iter().zip(e2.iter()) {
            assert_eq!(a.category, b.category);
            assert_eq!(a.tick_start, b.tick_start);
            assert_eq!(a.participants, b.participants);
            assert_eq!(a.causal_edges.len(), b.causal_edges.len());
        }
    }

    // AC11: thought 为空降级 co_occurrence
    #[test]
    fn test_empty_thought_degrades_to_co_occurrence() {
        let rows = vec![
            row(100, agent_a(), "攻击", json!({"target_agent_id": agent_b()}), None),
            row(100, agent_b(), "攻击", json!({"target_agent_id": agent_a()}), None),
            row(100, agent_a(), "予", json!({"recipient_type":"agent","recipient_id":agent_b(),"item_id":"x","quantity":1}), None),
        ];
        let cfg = test_config();
        let (events, _) = run_detection(&rows, &names(), &cfg);
        let causal: Vec<_> = events.iter().filter(|e| e.category == "causal_emergence").collect();
        assert!(causal.is_empty());
    }

    // AC12: node_id 缺失排除
    #[test]
    fn test_missing_node_excluded() {
        let mut r = row(100, agent_a(), "攻击", json!({"target_agent_id": agent_b()}), None);
        r.node_id = None;
        let rows = vec![
            r,
            row(100, agent_b(), "说话", json!({}), Some("test")),
        ];
        let cfg = test_config();
        let (events, cand) = run_detection(&rows, &names(), &cfg);
        assert_eq!(cand, 0);
        assert!(events.is_empty());
    }

    // AC13: pipe_seq 多意图去重
    #[test]
    fn test_pipe_seq_dedup() {
        let mut r0 = row(100, agent_a(), "攻击", json!({"target_agent_id": agent_b()}), None);
        r0.pipe_seq = 0;
        let mut r1 = row(100, agent_a(), "予", json!({"recipient_type":"agent","recipient_id":agent_b(),"item_id":"x","quantity":1}), None);
        r1.pipe_seq = 1;
        let mut r2 = row(100, agent_a(), "说话", json!({}), None);
        r2.pipe_seq = 2;
        let rows = vec![r0, r1, r2];
        let cfg = test_config();
        let (_events, cand) = run_detection(&rows, &names(), &cfg);
        // 单 agent 多 pipe_seq 去重为 1，min_agents=2 不满足
        assert_eq!(cand, 0);
    }

    // 配置驱动验证
    #[test]
    fn test_classify_category_config_driven() {
        let cfg = test_config();
        let rules = &cfg.detection.category_rules;
        assert_eq!(
            classify_category("攻击", "success", &json!({"target_agent_id":"x"}), rules),
            Some("conflict".to_string())
        );
        // require_success
        assert_eq!(
            classify_category("攻击", "failed", &json!({"target_agent_id":"x"}), rules),
            None
        );
        // require_target
        assert_eq!(classify_category("攻击", "success", &json!({}), rules), None);
        // transfer
        assert_eq!(
            classify_category("予", "success", &json!({"recipient_type":"agent"}), rules),
            Some("trade".to_string())
        );
        assert_eq!(
            classify_category("予", "success", &json!({"recipient_type":"ground"}), rules),
            None
        );
        // 非社会动作
        assert_eq!(classify_category("用", "success", &json!({}), rules), None);
    }
}
