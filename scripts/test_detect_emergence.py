#!/usr/bin/env python3
"""
detect_emergence.py 单元测试。

覆盖验收标准 AC3/AC4/AC5/AC11/AC12/AC13。
纯函数测试，不依赖 DB（构造 ActionRow fixture 直接喂 run_detection）。

运行：python scripts/test_detect_emergence.py
"""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

# 将脚本目录加入 path
sys.path.insert(0, str(Path(__file__).resolve().parent))
from detect_emergence import (  # noqa: E402
    ActionRow,
    classify_category,
    cluster_by_spacetime,
    cluster_passes_threshold,
    extract_partner_id,
    merge_chains,
    run_detection,
    verify_causal_edges,
)

# ============================================================================
# 测试用配置（与 emergence.yaml 同结构）
# ============================================================================

TEST_CFG = {
    "detection": {
        "category_rules": {
            "conflict": {"actions": ["攻击"], "require_target": True, "require_success": True},
            "trade": {"transfer_actions": [
                {"action": "予", "direction_field": "recipient_type", "direction_value": "agent"},
                {"action": "取", "direction_field": "source_type", "direction_value": "agent"},
            ]},
            "cooperation": {"actions": ["教导"], "require_target": True},
            "communication": {"actions": ["说话"], "require_success": True},
        },
        "min_agents": 2,
        "min_actions": 3,
        "min_categories": 2,
        "chain_gap_ticks": 5,
        "max_events": 50,
    },
    "causal": {"min_causal_edges": 1, "match_by": ["name", "short_uuid"], "short_uuid_len": 8},
    "health": {"supply_actions": ["用", "吃", "喝"], "min_survivors": 3, "min_supply_count": 3},
}

AGENT_A = "aaaaaaaa-0000-0000-0000-000000000001"
AGENT_B = "bbbbbbbb-0000-0000-0000-000000000002"
NAMES = {AGENT_A: "燕无归", AGENT_B: "钱三通"}


def _row(
    tick: int, agent: str, action: str, result: str = "success",
    data: dict | None = None, thought: str | None = None, node: str = "龙门大堂",
    pipe_seq: int = 0,
) -> ActionRow:
    return ActionRow(
        tick_id=tick, agent_id=agent, pipe_seq=pipe_seq, action_type=action,
        result=result, action_data=data or {}, thought_text=thought, node_id=node,
    )


# ============================================================================
# AC3：因果涌现判定（2 agent / 3 动作 / 2 类别 / 因果边成立 → 1 causal_emergence）
# ============================================================================


def test_ac3_causal_emergence_detected():
    """A 攻击 B（conflict），B thought 提及 A 名字并反击 A（conflict），A 给 B 物品（trade）。
    → causal_emergence，因 B 的 thought 提及 A 且 target 指向 A。"""
    rows = [
        _row(100, AGENT_A, "攻击", data={"target_agent_id": AGENT_B}),
        _row(100, AGENT_B, "攻击", data={"target_agent_id": AGENT_A},
             thought="燕无归此人伤我，当还以颜色"),
        _row(100, AGENT_A, "予", data={"recipient_type": "agent", "recipient_id": AGENT_B, "item_id": "x", "quantity": 1}),
    ]
    events, cand = run_detection(rows, NAMES, TEST_CFG, 100, 100)
    assert cand == 1, f"候选数应为1，实际{cand}"
    causal = [e for e in events if e.category == "causal_emergence"]
    assert len(causal) == 1, f"causal_emergence 应为1，实际{len(causal)}"
    assert len(causal[0].causal_edges) >= 1, "应至少1条因果边"
    print("AC3 ✅ causal_emergence 正确检测")


# ============================================================================
# AC4：虚假叙事排除（同形态，无因果 → co_occurrence 非 causal）
# ============================================================================


def test_ac4_co_occurrence_when_no_causal():
    """同形态共现，但 B 的 thought 不提及 A，且 B 动作不指向 A。
    → 0 causal_emergence，1 co_occurrence。"""
    rows = [
        _row(100, AGENT_A, "攻击", data={"target_agent_id": AGENT_B}),
        _row(100, AGENT_B, "说话", thought="今天天气不错"),  # thought 不提及 A，无 target
        _row(100, AGENT_A, "予", data={"recipient_type": "agent", "recipient_id": AGENT_B, "item_id": "x", "quantity": 1}),
    ]
    events, cand = run_detection(rows, NAMES, TEST_CFG, 100, 100)
    assert cand == 1
    causal = [e for e in events if e.category == "causal_emergence"]
    co = [e for e in events if e.category == "co_occurrence"]
    assert len(causal) == 0, "无因果边不应判 causal"
    assert len(co) == 1, f"应为1 co_occurrence，实际{len(co)}"
    print("AC4 ✅ 虚假叙事正确排除（降级 co_occurrence）")


# ============================================================================
# AC5：确定性不变量（同输入两次运行 deep-equal）
# ============================================================================


def test_ac5_deterministic():
    rows = [
        _row(100, AGENT_A, "攻击", data={"target_agent_id": AGENT_B}),
        _row(100, AGENT_B, "攻击", data={"target_agent_id": AGENT_A}, thought="燕无归伤我"),
        _row(101, AGENT_A, "说话", thought="钱三通此人"),
    ]
    e1, _ = run_detection(copy.deepcopy(rows), NAMES, copy.deepcopy(TEST_CFG), 100, 101)
    e2, _ = run_detection(copy.deepcopy(rows), NAMES, copy.deepcopy(TEST_CFG), 100, 101)
    # 序列化对比（含 sorted）
    j1 = json.dumps([e.__dict__ for e in e1], sort_keys=True, ensure_ascii=False)
    j2 = json.dumps([e.__dict__ for e in e2], sort_keys=True, ensure_ascii=False)
    assert j1 == j2, "两次运行结果不一致，违反确定性不变量"
    print("AC5 ✅ 确定性不变量通过")


# ============================================================================
# AC11：thought 为空时归 co_occurrence（不报错/不假阳性）
# ============================================================================


def test_ac11_empty_thought_degrades_to_co_occurrence():
    """B 的 thought 为 None，即使 target 指向 A，因果边也无法验证 → co_occurrence。"""
    rows = [
        _row(100, AGENT_A, "攻击", data={"target_agent_id": AGENT_B}),
        _row(100, AGENT_B, "攻击", data={"target_agent_id": AGENT_A}, thought=None),
        _row(100, AGENT_A, "予", data={"recipient_type": "agent", "recipient_id": AGENT_B, "item_id": "x", "quantity": 1}),
    ]
    events, _ = run_detection(rows, NAMES, TEST_CFG, 100, 100)
    causal = [e for e in events if e.category == "causal_emergence"]
    co = [e for e in events if e.category == "co_occurrence"]
    assert len(causal) == 0, "thought 为空不应判 causal"
    assert len(co) == 1
    print("AC11 ✅ thought 为空正确降级为 co_occurrence")


# ============================================================================
# AC12：agent_states 缺快照（node_id=None）→ 不参与聚类，不崩溃
# ============================================================================


def test_ac12_missing_node_excluded():
    """A 的 node_id=None（agent_states 无快照），不参与聚类。
    剩余 B 单独动作不足以成簇 → 0 候选，不崩溃。"""
    rows = [
        _row(100, AGENT_A, "攻击", data={"target_agent_id": AGENT_B}, node=None),
        _row(100, AGENT_B, "说话", thought="test"),
    ]
    events, cand = run_detection(rows, NAMES, TEST_CFG, 100, 100)
    assert cand == 0, f"node=None 的行应排除，候选应为0，实际{cand}"
    assert len(events) == 0
    print("AC12 ✅ agent_states 缺快照正确降级")


# ============================================================================
# AC13：同 agent 多 pipe_seq → DISTINCT 去重
# ============================================================================


def test_ac13_pipe_seq_dedup():
    """同一 agent 在同 tick 有 pipe_seq=0,1,2 三个动作，但只算 1 个 agent。
    min_agents=2 不满足 → 0 候选。"""
    rows = [
        _row(100, AGENT_A, "攻击", data={"target_agent_id": AGENT_B}, pipe_seq=0),
        _row(100, AGENT_A, "予", data={"recipient_type": "agent", "recipient_id": AGENT_B, "item_id": "x", "quantity": 1}, pipe_seq=1),
        _row(100, AGENT_A, "说话", pipe_seq=2),
    ]
    events, cand = run_detection(rows, NAMES, TEST_CFG, 100, 100)
    assert cand == 0, f"单 agent 多 pipe_seq 应去重为1，min_agents=2不满足，候选应0，实际{cand}"
    print("AC13 ✅ pipe_seq 多意图正确去重")


# ============================================================================
# 补充：classify_category 配置驱动验证（零硬编码）
# ============================================================================


def test_classify_category_config_driven():
    rules = TEST_CFG["detection"]["category_rules"]
    # conflict
    assert classify_category("攻击", "success", {"target_agent_id": "x"}, rules) == "conflict"
    assert classify_category("攻击", "failed", {"target_agent_id": "x"}, rules) is None  # require_success
    assert classify_category("攻击", "success", {}, rules) is None  # require_target
    # trade
    assert classify_category("予", "success", {"recipient_type": "agent"}, rules) == "trade"
    assert classify_category("取", "success", {"source_type": "agent"}, rules) == "trade"
    assert classify_category("予", "success", {"recipient_type": "ground"}, rules) is None
    # communication
    assert classify_category("说话", "success", {}, rules) == "communication"
    assert classify_category("说话", "failed", {}, rules) is None
    # 非社会动作
    assert classify_category("用", "success", {}, rules) is None
    print("classify_category ✅ 配置驱动判定正确（零硬编码）")


def test_extract_partner_id():
    assert extract_partner_id("攻击", {"target_agent_id": AGENT_B}, {}) == AGENT_B
    assert extract_partner_id("予", {"recipient_id": AGENT_B}, {}) == AGENT_B
    assert extract_partner_id("取", {"source_id": AGENT_A}, {}) == AGENT_A
    assert extract_partner_id("说话", {}, {}) is None
    print("extract_partner_id ✅")


if __name__ == "__main__":
    test_ac3_causal_emergence_detected()
    test_ac4_co_occurrence_when_no_causal()
    test_ac5_deterministic()
    test_ac11_empty_thought_degrades_to_co_occurrence()
    test_ac12_missing_node_excluded()
    test_ac13_pipe_seq_dedup()
    test_classify_category_config_driven()
    test_extract_partner_id()
    print("\n全部测试通过 ✅")
