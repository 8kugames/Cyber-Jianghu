#!/usr/bin/env python3
"""
因果涌现事件检测器（Causal Emergence Detector）

机器可验证地回答项目的核心假设："在生存压力下，AI 智能体会自发涌现
结盟、背叛、交易、厮杀等社会行为吗？"（白皮书 / MVP §6.1.3）

两阶段检测：
  阶段 1 形态筛选：按 tick+node 聚类时空簇，阈值判定候选事件（MVP §6.1.3）。
  阶段 2 因果验证：对候选簇验证 agent 间"感知→处理→定向回应"闭环，
                   区分 causal_emergence（真因果）与 co_occurrence（仅共现/存疑）。

离线工具，只读，不进 server 运行时（望远镜哲学，与 analyze_social_structure.py /
build_sft_data.py 同范式）。所有阈值与动作映射由 emergence.yaml 驱动，判定逻辑零硬编码。

注意：server 运行时的在线检测已迁移至 crates/server/src/emergence/（Rust 实现），
配置在 crates/server/config/emergence.yaml。本 Python 脚本保留为历史回填工具
（可对任意旧 tick 区间重算），其纯函数是 Rust 移植的参考基准。
两处的 emergence.yaml 内容应保持同步。

数据源（server Postgres）：
  - agent_action_logs：事件流主源（action_type/action_data/result/thought_log/...）
  - agent_states：位置 node_id + is_alive（LEFT JOIN，不保证每 tick 有快照）
  - agents：agent name
  - tick_logs：运行稳定性健康度

确定性不变量：同输入 → 同输出。无随机/LLM/时钟依赖。所有输出 sorted()。

用法：
  DATABASE_URL=postgres://... python scripts/detect_emergence.py [--config emergence.yaml]
                                        [--tick-start N] [--tick-end N]
                                        [--report docs/reports/emergence-baseline.md]
                                        [--no-report]
  不指定 tick 窗口时默认取最近 240 tick（MVP 观测窗口）。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

try:
    import psycopg2  # type: ignore
except ImportError:
    psycopg2 = None  # type: ignore

try:
    import yaml  # type: ignore
except ImportError:
    yaml = None  # type: ignore


# ============================================================================
# 数据模型
# ============================================================================


@dataclass
class ActionRow:
    """agent_action_logs 单行投影（经 SQL 查询提取）。"""

    tick_id: int
    agent_id: str
    pipe_seq: int
    action_type: str
    result: str
    action_data: Dict[str, Any]
    thought_text: Optional[str]  # COALESCE(顶层 thought_log, metadata thought_log)
    node_id: Optional[str]  # LEFT JOIN agent_states，可能为 None


@dataclass
class HealthMetrics:
    """MVP §6.1.1/§6.1.2 健康度。"""

    ticks_total: int = 0
    ticks_completed: int = 0
    ticks_failed: int = 0
    ticks_running: int = 0
    tick_completion_rate: float = 0.0
    continuous_run_seconds: float = 0.0
    agents_alive: int = 0
    min_survivors_required: int = 0
    survivors_pass: bool = False
    per_agent_supply: Dict[str, int] = field(default_factory=dict)
    min_supply_required: int = 0
    supply_pass: bool = False


@dataclass
class EmergenceEvent:
    """一个检测到的事件链。"""

    category: str  # "causal_emergence" | "co_occurrence"
    tick_start: int
    tick_end: int
    participants: List[str]  # sorted agent_ids
    action_count: int
    categories_covered: List[str]  # sorted
    causal_edges: List[Dict[str, str]]  # [{from, to, evidence: "thought提及+target指向"}, ...]
    actions: List[Dict[str, Any]]  # 构成事件的动作摘要（sorted by tick_id,agent_id,pipe_seq）


# ============================================================================
# 纯函数：阶段 1 形态筛选（可单测，不依赖 DB）
# ============================================================================


def classify_category(
    action_type: str,
    result: str,
    action_data: Dict[str, Any],
    category_rules: Dict[str, Any],
) -> Optional[str]:
    """将单条动作投射到社会类别。返回类别名或 None（非社会动作）。

    判定逻辑完全由 category_rules 配置驱动，零硬编码动作名。
    """
    for cat_name, rule in category_rules.items():
        # 形态 A：actions 列表
        if "actions" in rule:
            if action_type not in rule["actions"]:
                continue
            if rule.get("require_success") and result != "success":
                continue
            if rule.get("require_target"):
                tgt = action_data.get("target_agent_id")
                if tgt is None or tgt == "" or tgt == "null":
                    continue
            return cat_name
        # 形态 B：transfer_actions（予/取 方向字段判定）
        if "transfer_actions" in rule:
            for ta in rule["transfer_actions"]:
                if action_type != ta["action"]:
                    continue
                field_val = action_data.get(ta["direction_field"])
                if field_val == ta["direction_value"]:
                    return cat_name
    return None


def extract_partner_id(
    action_type: str, action_data: Dict[str, Any], category_rules: Dict[str, Any]
) -> Optional[str]:
    """提取该动作的社会伙伴 agent_id（target/recipient/source），用于因果边判定。

    通用策略：按 category_rules 中该动作命中的规则，读对应字段。
    兜底：常见字段名直接探测。全部来自 action_data，不硬编码动作语义。
    """
    # 通用探测（覆盖 target_agent_id / recipient_id / source_id）
    for key in ("target_agent_id", "recipient_id", "source_id"):
        val = action_data.get(key)
        if val is not None and val != "" and val != "null":
            return str(val)
    return None


def cluster_by_spacetime(rows: List[ActionRow]) -> Dict[Tuple[int, str], List[ActionRow]]:
    """按 (tick_id, node_id) 聚类。node_id 为 None 的行不参与聚类。"""
    clusters: Dict[Tuple[int, str], List[ActionRow]] = defaultdict(list)
    for r in rows:
        if r.node_id is None:
            continue
        clusters[(r.tick_id, r.node_id)].append(r)
    return clusters


def cluster_passes_threshold(
    cluster_rows: List[ActionRow],
    category_rules: Dict[str, Any],
    min_agents: int,
    min_actions: int,
    min_categories: int,
) -> Tuple[bool, List[str], List[str]]:
    """判定一个时空簇是否通过 MVP §6.1.3 阈值。

    返回 (通过, sorted参与者agent_ids, sorted覆盖类别)。
    agent 去重用 DISTINCT（处理同 agent 多 pipe_seq）。
    """
    distinct_agents = sorted({r.agent_id for r in cluster_rows})
    if len(distinct_agents) < min_agents:
        return False, distinct_agents, []
    if len(cluster_rows) < min_actions:
        return False, distinct_agents, []

    categories = set()
    for r in cluster_rows:
        cat = classify_category(r.action_type, r.result, r.action_data, category_rules)
        if cat:
            categories.add(cat)
    if len(categories) < min_categories:
        return False, distinct_agents, sorted(categories)

    return True, distinct_agents, sorted(categories)


def merge_chains(
    qualified_clusters: List[Tuple[int, str, List[ActionRow], List[str], List[str]]],
    chain_gap_ticks: int,
) -> List[Dict[str, Any]]:
    """相邻 tick（间隔 ≤ chain_gap_ticks）的合格簇合并为事件链。

    qualified_clusters 元素: (tick_id, node_id, rows, participants, categories)
    返回合并后的链列表，每链含合并的 rows/participants/categories/tick_range。
    """
    if not qualified_clusters:
        return []
    # 按 tick_id 排序保证确定性
    sorted_clusters = sorted(qualified_clusters, key=lambda c: c[0])
    chains: List[Dict[str, Any]] = []
    current: Optional[Dict[str, Any]] = None
    for tick_id, node_id, rows, parts, cats in sorted_clusters:
        if current is not None and tick_id - current["tick_end"] <= chain_gap_ticks:
            # 合并到当前链
            current["rows"].extend(rows)
            current["participants"] = sorted(set(current["participants"]) | set(parts))
            current["categories"] = sorted(set(current["categories"]) | set(cats))
            current["tick_end"] = tick_id
        else:
            if current is not None:
                chains.append(current)
            current = {
                "tick_start": tick_id,
                "tick_end": tick_id,
                "rows": list(rows),
                "participants": list(parts),
                "categories": list(cats),
            }
    if current is not None:
        chains.append(current)
    return chains


# ============================================================================
# 纯函数：阶段 2 因果验证（可单测）
# ============================================================================


def _match_tokens(name: Optional[str], agent_id: str, short_uuid_len: int) -> List[str]:
    """构造在 thought_text 中匹配该 agent 的 token 列表。"""
    tokens: List[str] = []
    if name:
        tokens.append(name)
    # UUID 前 N 位（去连字符取前 N，兼容 thought 中可能出现的短 id）
    short = agent_id.replace("-", "")[:short_uuid_len]
    if short:
        tokens.append(short)
    # 原始 UUID 前 N 位（含连字符前段）
    tokens.append(agent_id[:short_uuid_len])
    return tokens


def verify_causal_edges(
    chain: Dict[str, Any],
    agent_names: Dict[str, str],
    match_by: List[str],
    short_uuid_len: int,
) -> List[Dict[str, str]]:
    """验证链内 agent 间的因果边（感知→处理→定向回应闭环）。

    返回经验证的因果边列表 [{from, to, evidence}]。
    感知=同簇（已由聚类保证同 node）；处理=thought_text 匹配对方；回应=partner_id 指向对方。
    """
    edges: List[Dict[str, str]] = []
    rows = chain["rows"]
    # 索引：每个 agent 在该链的动作 + thought
    agent_actions: Dict[str, List[ActionRow]] = defaultdict(list)
    for r in rows:
        agent_actions[r.agent_id].append(r)

    participants = chain["participants"]
    for actor_b in sorted(participants):
        for r_b in sorted(agent_actions.get(actor_b, []), key=lambda x: (x.tick_id, x.agent_id, x.pipe_seq)):
            if not r_b.thought_text:
                continue  # thought 为空 → 因果无法验证，跳过
            partner = extract_partner_id(r_b.action_type, r_b.action_data, {})
            if partner is None or partner not in participants or partner == actor_b:
                continue
            # B 的动作指向 partner(A)，且 B 的 thought 提及 A
            tokens = _match_tokens(
                agent_names.get(partner),
                partner,
                short_uuid_len,
            )
            matched = any(tok and tok in r_b.thought_text for tok in tokens) if "name" in match_by or "short_uuid" in match_by else False
            if matched:
                edges.append(
                    {
                        "from": actor_b,
                        "to": partner,
                        "evidence": "thought提及+target指向",
                    }
                )
    # 去重（同 from→to 多条动作只记代表）
    seen = set()
    unique_edges: List[Dict[str, str]] = []
    for e in sorted(edges, key=lambda x: (x["from"], x["to"])):
        key = (e["from"], e["to"])
        if key not in seen:
            seen.add(key)
            unique_edges.append(e)
    return unique_edges


# ============================================================================
# DB 访问（与判定逻辑分离）
# ============================================================================


def load_config(config_path: Path) -> Dict[str, Any]:
    """加载 emergence.yaml。缺失/格式错 fail-fast。"""
    if yaml is None:
        sys.exit("[错误] 需要 PyYAML：pip install pyyaml")
    if not config_path.exists():
        sys.exit(f"[错误] 配置文件不存在：{config_path}")
    try:
        with open(config_path, "r", encoding="utf-8") as f:
            cfg = yaml.safe_load(f)
    except yaml.YAMLError as e:
        sys.exit(f"[错误] 配置文件格式错误：{e}")
    if not isinstance(cfg, dict):
        sys.exit("[错误] 配置根节点必须是 dict")
    return cfg


def fetch_window(
    conn: Any, tick_start: int, tick_end: int
) -> Tuple[List[ActionRow], Dict[str, str]]:
    """读取时间窗口内的动作流 + agent 名字映射。

    thought_text = COALESCE(顶层 thought_log, metadata 嵌套 thought_log)。
    node_id LEFT JOIN agent_states（不保证每 tick 有快照）。
    """
    cur = conn.cursor()
    cur.execute(
        """
        SELECT l.tick_id,
               l.agent_id::text,
               l.pipe_seq,
               l.action_type,
               l.result,
               l.action_data,
               COALESCE(l.thought_log,
                        l.soul_cycle_metadata->'cycles'->0->'renhun'->>'thought_log') AS thought_text,
               s.node_id
        FROM agent_action_logs l
        LEFT JOIN agent_states s
          ON s.agent_id = l.agent_id AND s.tick_id = l.tick_id
        WHERE l.tick_id BETWEEN %s AND %s
        ORDER BY l.tick_id, l.agent_id, l.pipe_seq
        """,
        (tick_start, tick_end),
    )
    rows: List[ActionRow] = []
    for tick_id, agent_id, pipe_seq, action_type, result, action_data, thought_text, node_id in cur.fetchall():
        ad = action_data if isinstance(action_data, dict) else (
            json.loads(action_data) if action_data else {}
        )
        rows.append(
            ActionRow(
                tick_id=int(tick_id),
                agent_id=str(agent_id),
                pipe_seq=int(pipe_seq or 0),
                action_type=str(action_type),
                result=str(result or ""),
                action_data=ad,
                thought_text=thought_text,
                node_id=node_id,
            )
        )

    # agent 名字
    cur.execute("SELECT agent_id::text, name FROM agents")
    agent_names = {str(aid): str(name) for aid, name in cur.fetchall()}
    cur.close()
    return rows, agent_names


def fetch_health(
    conn: Any,
    tick_start: int,
    tick_end: int,
    supply_actions: List[str],
    min_survivors: int,
    min_supply_count: int,
) -> HealthMetrics:
    """读取 MVP §6.1.1/§6.1.2 健康度。"""
    cur = conn.cursor()
    h = HealthMetrics(min_survivors_required=min_survivors, min_supply_required=min_supply_count)

    # tick 完成率
    cur.execute(
        """
        SELECT status, COUNT(*), EXTRACT(EPOCH FROM (COALESCE(MAX(completed_at), MAX(started_at))
              - MIN(started_at)))
        FROM tick_logs WHERE tick_id BETWEEN %s AND %s GROUP BY status
        """,
        (tick_start, tick_end),
    )
    for status, cnt, span in cur.fetchall():
        cnt = int(cnt or 0)
        h.ticks_total += cnt
        if status == "completed":
            h.ticks_completed = cnt
        elif status == "failed":
            h.ticks_failed = cnt
        elif status == "running":
            h.ticks_running = cnt
        if span:
            h.continuous_run_seconds = max(h.continuous_run_seconds, float(span))
    h.tick_completion_rate = (h.ticks_completed / h.ticks_total) if h.ticks_total else 0.0

    # 窗口末点存活数（取窗口内最大 tick 的快照）
    cur.execute(
        """
        SELECT s.agent_id::text, s.is_alive
        FROM agent_states s
        WHERE s.tick_id = (SELECT MAX(tick_id) FROM agent_states WHERE tick_id BETWEEN %s AND %s)
        """,
        (tick_start, tick_end),
    )
    alive = 0
    for _aid, is_alive in cur.fetchall():
        if is_alive:
            alive += 1
    h.agents_alive = alive
    h.survivors_pass = alive >= min_survivors

    # 每存活 agent 补给次数
    if supply_actions:
        placeholders = ",".join(["%s"] * len(supply_actions))
        cur.execute(
            f"""
            SELECT agent_id::text, COUNT(*)
            FROM agent_action_logs
            WHERE tick_id BETWEEN %s AND %s
              AND action_type IN ({placeholders})
              AND result = 'success'
            GROUP BY agent_id
            """,
            (tick_start, tick_end, *supply_actions),
        )
        supply_map: Dict[str, int] = {}
        for aid, cnt in cur.fetchall():
            supply_map[str(aid)] = int(cnt or 0)
        # 只统计存活 agent
        cur.execute(
            """
            SELECT DISTINCT s.agent_id::text FROM agent_states s
            WHERE s.tick_id = (SELECT MAX(tick_id) FROM agent_states WHERE tick_id BETWEEN %s AND %s)
              AND s.is_alive = true
            """,
            (tick_start, tick_end),
        )
        alive_ids = {str(r[0]) for r in cur.fetchall()}
        h.per_agent_supply = {aid: supply_map.get(aid, 0) for aid in sorted(alive_ids)}
        h.supply_pass = all(v >= min_supply_count for v in h.per_agent_supply.values()) if h.per_agent_supply else False

    cur.close()
    return h


# ============================================================================
# 报告生成
# ============================================================================


def generate_markdown_report(
    events: List[EmergenceEvent],
    health: HealthMetrics,
    tick_start: int,
    tick_end: int,
    agent_names: Dict[str, str],
    candidate_count: int,
    co_occurrence_count: int,
) -> str:
    """生成 baseline Markdown 报告。"""
    lines: List[str] = []
    lines.append("# 涌现验证 Baseline 报告")
    lines.append("")
    lines.append(f"> 观测窗口：tick {tick_start} – {tick_end}")
    lines.append("")
    lines.append("## MVP 验收 Checklist（§6.1）")
    lines.append("")
    lines.append(f"- **§6.1.1 运行稳定性**：tick 完成率 {health.tick_completion_rate:.1%}（{health.ticks_completed}/{health.ticks_total}）"
                 f"，连续运行 {health.continuous_run_seconds:.0f}s")
    lines.append(f"- **§6.1.2 生存能力**：存活 {health.agents_alive}/{health.min_survivors_required} "
                 f"{'✅' if health.survivors_pass else '❌'}；"
                 f"补给达标 {'✅' if health.supply_pass else '❌'} "
                 f"(每人≥{health.min_supply_required}: {health.per_agent_supply})")
    causal_count = sum(1 for e in events if e.category == "causal_emergence")
    lines.append(f"- **§6.1.3 复杂交互**：causal_emergence {causal_count} 次 "
                 f"({'✅ ≥1' if causal_count >= 1 else '❌ 未观测到因果涌现'})")
    lines.append("")

    # 零涌现诊断段
    lines.append("## 诊断")
    lines.append("")
    lines.append(f"- 候选事件（形态筛选通过）：{candidate_count}")
    lines.append(f"- 其中 causal_emergence（因果验证通过）：{causal_count}")
    lines.append(f"- 其中 co_occurrence（仅共现，无法证明因果）：{co_occurrence_count}")
    if candidate_count > 0 and causal_count == 0:
        lines.append("- **卡点**：形态共现存在，但因果验证未通过——agent 间无 evidence 支撑的"
                     "感知→处理→回应闭环。可能原因：thought_log 为空、agent 未互相定向、"
                     "或 LLM 叙事未提及对方。")
    elif candidate_count == 0:
        lines.append("- **卡点**：形态筛选未通过——无满足 ≥N动作/≥N agent/≥N类别 的时空簇。"
                     "可能原因：agent 活动稀疏、社会动作少、或窗口过小。")
    lines.append("")

    # causal_emergence 详情
    causal_events = sorted([e for e in events if e.category == "causal_emergence"],
                           key=lambda e: (e.tick_start,))
    if causal_events:
        lines.append("## Causal Emergence 事件（强信号）")
        lines.append("")
        for i, e in enumerate(causal_events, 1):
            lines.append(f"### 事件 {i}：tick {e.tick_start}–{e.tick_end}")
            names = ", ".join(agent_names.get(a, a[:8]) for a in e.participants)
            lines.append(f"- 参与者：{names}")
            lines.append(f"- 动作数：{e.action_count}；类别覆盖：{', '.join(e.categories_covered)}")
            for edge in e.causal_edges:
                fn = agent_names.get(edge["from"], edge["from"][:8])
                tn = agent_names.get(edge["to"], edge["to"][:8])
                lines.append(f"- 因果边：{fn} → {tn}（{edge['evidence']}）")
            lines.append("")

    co_events = sorted([e for e in events if e.category == "co_occurrence"],
                       key=lambda e: (e.tick_start,))
    if co_events:
        lines.append("## Co-occurrence 事件（弱信号/存疑）")
        lines.append("")
        lines.append("> 仅满足形态共现，未通过因果验证。无法证明 agent 间存在因果互动。")
        lines.append("")
        for i, e in enumerate(co_events, 1):
            names = ", ".join(agent_names.get(a, a[:8]) for a in e.participants)
            lines.append(f"- tick {e.tick_start}–{e.tick_end}：{names}，{e.action_count} 动作，类别 {', '.join(e.categories_covered)}")
        lines.append("")

    return "\n".join(lines)


# ============================================================================
# 主流程
# ============================================================================


def run_detection(
    rows: List[ActionRow],
    agent_names: Dict[str, str],
    cfg: Dict[str, Any],
    tick_start: int,
    tick_end: int,
) -> Tuple[List[EmergenceEvent], int]:
    """两阶段检测主逻辑（纯函数，不碰 DB，便于单测）。返回 (事件列表, 候选总数)。"""
    det = cfg["detection"]
    cat_rules = det["category_rules"]
    min_agents = det["min_agents"]
    min_actions = det["min_actions"]
    min_categories = det["min_categories"]
    chain_gap = det["chain_gap_ticks"]
    max_events = det.get("max_events", 50)

    causal_cfg = cfg.get("causal", {})
    min_causal_edges = causal_cfg.get("min_causal_edges", 1)
    match_by = causal_cfg.get("match_by", ["name", "short_uuid"])
    short_uuid_len = causal_cfg.get("short_uuid_len", 8)

    # 阶段 1：形态筛选
    clusters = cluster_by_spacetime(rows)
    qualified: List[Tuple[int, str, List[ActionRow], List[str], List[str]]] = []
    for (tick_id, node_id), c_rows in sorted(clusters.items()):
        ok, parts, cats = cluster_passes_threshold(
            c_rows, cat_rules, min_agents, min_actions, min_categories
        )
        if ok:
            qualified.append((tick_id, node_id, c_rows, parts, cats))
    candidate_count = len(qualified)
    chains = merge_chains(qualified, chain_gap)

    # 阶段 2：因果验证
    events: List[EmergenceEvent] = []
    for chain in sorted(chains, key=lambda c: c["tick_start"]):
        edges = verify_causal_edges(chain, agent_names, match_by, short_uuid_len)
        is_causal = len(edges) >= min_causal_edges
        chain_rows = sorted(chain["rows"], key=lambda r: (r.tick_id, r.agent_id, r.pipe_seq))
        action_summaries = [
            {
                "tick_id": r.tick_id,
                "agent_id": r.agent_id,
                "action_type": r.action_type,
                "result": r.result,
            }
            for r in chain_rows
        ]
        events.append(
            EmergenceEvent(
                category="causal_emergence" if is_causal else "co_occurrence",
                tick_start=chain["tick_start"],
                tick_end=chain["tick_end"],
                participants=sorted(chain["participants"]),
                action_count=len(chain_rows),
                categories_covered=sorted(chain["categories"]),
                causal_edges=edges,
                actions=action_summaries,
            )
        )

    # 限量 + 排序（causal 优先，再按 tick）
    events.sort(key=lambda e: (0 if e.category == "causal_emergence" else 1, e.tick_start))
    events = events[:max_events]
    return events, candidate_count


def main() -> None:
    parser = argparse.ArgumentParser(description="因果涌现事件检测器")
    parser.add_argument("--config", default=None, help="emergence.yaml 路径（默认脚本同目录）")
    parser.add_argument("--tick-start", type=int, default=None, help="窗口起始 tick_id")
    parser.add_argument("--tick-end", type=int, default=None, help="窗口结束 tick_id")
    parser.add_argument("--report", default=None, help="输出 Markdown 报告路径")
    parser.add_argument("--no-report", action="store_true", help="不生成 Markdown 报告")
    args = parser.parse_args()

    # AC1: DATABASE_URL 缺失 fail-fast
    db_url = os.environ.get("DATABASE_URL")
    if not db_url:
        sys.exit("[错误] 未设置 DATABASE_URL 环境变量")

    # AC2: 配置缺失 fail-fast
    config_path = Path(args.config) if args.config else Path(__file__).resolve().parent / "emergence.yaml"
    cfg = load_config(config_path)

    if psycopg2 is None:
        sys.exit("[错误] 需要 psycopg2：pip install psycopg2-binary")

    conn = psycopg2.connect(db_url)
    conn.set_session(readonly=True)  # 只读，望远镜哲学

    try:
        cur = conn.cursor()
        # 默认窗口：最近 240 tick（AC6）
        if args.tick_start is None or args.tick_end is None:
            cur.execute("SELECT MAX(tick_id) FROM agent_action_logs")
            max_tick = cur.fetchone()[0] or 0
            tick_end = args.tick_end if args.tick_end is not None else max_tick
            tick_start = args.tick_start if args.tick_start is not None else max(0, tick_end - 240)
        else:
            tick_start, tick_end = args.tick_start, args.tick_end
        cur.close()

        rows, agent_names = fetch_window(conn, tick_start, tick_end)
        events, candidate_count = run_detection(rows, agent_names, cfg, tick_start, tick_end)

        health_cfg = cfg.get("health", {})
        health = fetch_health(
            conn, tick_start, tick_end,
            health_cfg.get("supply_actions", []),
            health_cfg.get("min_survivors", 3),
            health_cfg.get("min_supply_count", 3),
        )

        # AC5 确定性输出
        output = {
            "tick_start": tick_start,
            "tick_end": tick_end,
            "candidate_count": candidate_count,
            "causal_emergence_count": sum(1 for e in events if e.category == "causal_emergence"),
            "co_occurrence_count": sum(1 for e in events if e.category == "co_occurrence"),
            "events": [
                {
                    "category": e.category,
                    "tick_start": e.tick_start,
                    "tick_end": e.tick_end,
                    "participants": e.participants,
                    "action_count": e.action_count,
                    "categories_covered": e.categories_covered,
                    "causal_edges": e.causal_edges,
                    "actions": e.actions,
                }
                for e in events
            ],
            "health": {
                "tick_completion_rate": round(health.tick_completion_rate, 4),
                "ticks_total": health.ticks_total,
                "ticks_completed": health.ticks_completed,
                "ticks_failed": health.ticks_failed,
                "continuous_run_seconds": round(health.continuous_run_seconds, 1),
                "agents_alive": health.agents_alive,
                "min_survivors_required": health.min_survivors_required,
                "survivors_pass": health.survivors_pass,
                "per_agent_supply": health.per_agent_supply,
                "min_supply_required": health.min_supply_required,
                "supply_pass": health.supply_pass,
            },
        }
        # sort_keys=True + ensure_ascii=False 确定性序列化（AC5）
        print(json.dumps(output, ensure_ascii=False, sort_keys=True, separators=(",", ":")))

        # AC8 报告
        if not args.no_report:
            report_path = Path(args.report) if args.report else (
                Path("docs/reports") / f"emergence-baseline-{tick_end}.md"
            )
            report_path.parent.mkdir(parents=True, exist_ok=True)
            md = generate_markdown_report(
                events, health, tick_start, tick_end, agent_names,
                candidate_count, output["co_occurrence_count"],
            )
            report_path.write_text(md, encoding="utf-8")
            print(f"[报告] 已生成：{report_path}", file=sys.stderr)
    finally:
        conn.close()


if __name__ == "__main__":
    main()
