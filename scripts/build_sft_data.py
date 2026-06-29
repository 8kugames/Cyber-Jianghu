#!/usr/bin/env python3
"""
SFT 训练数据导出脚本

将 server 端汇聚的 trace + soul_cycle 审查结果 join 成 SFT 训练集。
离线工具，手动触发，只读不写 agent 状态。

数据源（server 端）：
  - traces/soul=renhun/agent=<id>/date=*.jsonl  （人魂 LLM 调用，已脱敏）
  - DB agent_action_logs.soul_cycle_metadata    （天魂审查结果 approved/rejected）
  - rewards/lifetime/agent=<id>.jsonl           （可选：按 longevity 筛选）

筛选逻辑：
  - 主筛选：天魂 approved 的人魂 trace（合规样本）
  - 可选：--top-longevity 0.25 只取高 longevity agent

输出格式（messages，兼容 vLLM/Axolotl）：
  {"messages": [{"role":"system","content":"..."},{"role":"user","content":"..."},
                {"role":"assistant","content":"..."}],
   "metadata": {"agent_id":"...","tick_id":42,"tianhun_result":"approved",...}}

用法：
  DATABASE_URL=postgres://... python scripts/build_sft_data.py <data_dir> --output sft.jsonl
  DATABASE_URL=postgres://... python scripts/build_sft_data.py <data_dir> --top-longevity 0.25 -o sft.jsonl

  若无 DB，用 --no-db-filter 跳过天魂筛选（导出所有人魂 trace，不筛 approved）。
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3  # noqa: F401 (预留未来从 agent 端 db 读)
import sys
from collections import defaultdict
from pathlib import Path
from typing import Dict, List, Optional, Tuple

# TraceEntry JSON 结构（与 crates/protocol/src/messages.rs 对齐）


def load_traces(data_dir: Path) -> List[dict]:
    """遍历 traces/soul=renhun/ 读所有 TraceEntry。"""
    traces_dir = data_dir / "traces" / "soul=renhun"
    if not traces_dir.exists():
        print(f"[警告] trace 目录不存在: {traces_dir}", file=sys.stderr)
        return []

    traces: List[dict] = []
    for jsonl_path in sorted(traces_dir.rglob("*.jsonl")):
        with open(jsonl_path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                    traces.append(entry)
                except json.JSONDecodeError as e:
                    print(f"[警告] 解析 trace 行失败 {jsonl_path}: {e}", file=sys.stderr)

    return traces


def load_tianhun_results(database_url: str) -> Dict[Tuple[str, int], str]:
    """从 DB 查询每个 (agent_id, tick_id) 的天魂审查结果。

    soul_cycle_metadata 按 (agent_id, tick_id, pipe_seq) keyed，
    聚合语义：取该 tick 下最大 pipe_seq 的 tianhun.result（最后一次审查为准）。

    返回 {(agent_id, tick_id): "approved"/"rejected"/...}
    """
    try:
        import psycopg2
    except ImportError:
        print("[错误] 需要 psycopg2：pip install psycopg2-binary", file=sys.stderr)
        sys.exit(1)

    conn = psycopg2.connect(database_url)
    conn.set_session(readonly=True)  # 只读事务
    cursor = conn.cursor()

    # 查 soul_cycle_metadata，按 (agent_id, tick_id) 聚合取最后 pipe_seq
    cursor.execute(
        """
        SELECT DISTINCT ON (agent_id, tick_id)
               agent_id::text, tick_id, pipe_seq,
               soul_cycle_metadata
        FROM agent_action_logs
        WHERE soul_cycle_metadata IS NOT NULL
        ORDER BY agent_id, tick_id, pipe_seq DESC
        """
    )

    results: Dict[Tuple[str, int], str] = {}
    for agent_id, tick_id, _pipe_seq, metadata in cursor:
        if metadata is None:
            continue
        # 解析 metadata.cycles[last].tianhun.result
        try:
            if isinstance(metadata, str):
                metadata = json.loads(metadata)
            cycles = metadata.get("cycles", [])
            if not cycles:
                continue
            last_cycle = cycles[-1]
            tianhun = last_cycle.get("tianhun", {})
            result = tianhun.get("result")
            if result:
                results[(agent_id, tick_id)] = result
        except (json.JSONDecodeError, KeyError, TypeError) as e:
            print(f"[警告] 解析 metadata 失败 agent={agent_id} tick={tick_id}: {e}", file=sys.stderr)

    conn.close()
    return results


def load_longevity(data_dir: Path) -> Dict[str, int]:
    """读 rewards/lifetime/ 返回 {agent_id: longevity_days}。"""
    lifetime_dir = data_dir / "rewards" / "lifetime"
    if not lifetime_dir.exists():
        return {}

    longevity: Dict[str, int] = {}
    for jsonl_path in sorted(lifetime_dir.glob("agent=*.jsonl")):
        with open(jsonl_path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    record = json.loads(line)
                    agent_id = record.get("agent_id", "")
                    days = record.get("longevity_days", 0)
                    longevity[agent_id] = days
                except json.JSONDecodeError:
                    continue

    return longevity


def filter_top_longevity(
    traces: List[dict], longevity: Dict[str, int], top_ratio: float
) -> List[dict]:
    """只保留 top X% longevity agent 的 trace。"""
    if not longevity or top_ratio >= 1.0:
        return traces

    # 按 longevity 排序，取 top
    sorted_agents = sorted(longevity.items(), key=lambda x: x[1], reverse=True)
    n_top = max(1, int(len(sorted_agents) * top_ratio))
    top_agent_ids = {aid for aid, _ in sorted_agents[:n_top]}

    return [t for t in traces if t.get("agent_id", "") in top_agent_ids]


def trace_to_sft_sample(trace: dict, tianhun_result: Optional[str]) -> Optional[dict]:
    """将 TraceEntry 转换为 SFT messages 格式样本。

    跳过无 response 的 trace（ok=false 或 response 为空）。
    """
    response = trace.get("response", "").strip()
    if not response or not trace.get("ok", True):
        return None

    user_prompt = trace.get("user_prompt", "")

    # system_prompt 不在 trace 中存储（全局静态模板由配置复用）。
    # 训练时从 persona_name + persona_description 重建 persona 部分，
    # 静态部分（survival_rules/narrative/output_format）从 prompt_templates.yaml 渲染。
    persona_name = trace.get("persona_name", "")
    persona_description = trace.get("persona_description", "")

    # messages 格式：persona 信息作为 system 角色提示（训练时下游用完整模板补充）
    messages: List[dict] = []
    if persona_name:
        system_content = f"你是 {persona_name}。"
        if persona_description:
            system_content += f"\n{persona_description}"
        messages.append({"role": "system", "content": system_content})
    messages.append({"role": "user", "content": user_prompt})
    messages.append({"role": "assistant", "content": response})

    return {
        "messages": messages,
        "metadata": {
            "agent_id": trace.get("agent_id", ""),
            "tick_id": trace.get("tick_id", 0),
            "soul_stage": trace.get("soul_stage", ""),
            "attempt": trace.get("attempt", 0),
            "provider": trace.get("provider", ""),
            "model": trace.get("model", ""),
            "tianhun_result": tianhun_result,
            "trace_id": trace.get("trace_id", ""),
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser(
        description="SFT 训练数据导出（离线，只读，产出 messages JSONL）"
    )
    parser.add_argument(
        "data_dir", type=str, help="server 数据目录（含 traces/ + rewards/）"
    )
    parser.add_argument(
        "-o", "--output", type=str, default="sft_dataset.jsonl", help="输出文件路径"
    )
    parser.add_argument(
        "--top-longevity",
        type=float,
        default=1.0,
        help="只取 top X%% longevity agent（默认 1.0=全部，如 0.25=前25%%）",
    )
    parser.add_argument(
        "--no-db-filter",
        action="store_true",
        help="跳过天魂筛选（无 DB 时用，导出所有人魂 trace）",
    )
    args = parser.parse_args()

    data_dir = Path(args.data_dir).expanduser()
    if not data_dir.is_dir():
        print(f"[错误] 数据目录不存在: {data_dir}", file=sys.stderr)
        sys.exit(1)

    print(f"=== SFT 数据导出 ===")
    print(f"数据目录: {data_dir}")
    print()

    # 1. 加载所有人魂 trace
    traces = load_traces(data_dir)
    print(f"加载人魂 trace: {len(traces)} 条")
    if not traces:
        print("无 trace 数据，导出结束。")
        return

    # 2. 可选：按 longevity 筛选 top agent
    if args.top_longevity < 1.0:
        longevity = load_longevity(data_dir)
        if longevity:
            traces = filter_top_longevity(traces, longevity, args.top_longevity)
            print(f"top-longevity {args.top_longevity:.0%} 筛选后: {len(traces)} 条")
        else:
            print(f"[警告] 无 lifetime reward 数据，跳过 longevity 筛选")

    # 3. 天魂审查结果筛选
    tianhun_map: Dict[Tuple[str, int], str] = {}
    approved_only = True
    if args.no_db_filter:
        print("[模式] --no-db-filter：跳过天魂筛选，导出所有人魂 trace")
        approved_only = False
    else:
        db_url = os.environ.get("DATABASE_URL")
        if not db_url:
            print(
                "[警告] 未设置 DATABASE_URL，无法查天魂结果。用 --no-db-filter 跳过，或设置 DATABASE_URL。",
                file=sys.stderr,
            )
            sys.exit(1)
        tianhun_map = load_tianhun_results(db_url)
        print(f"加载天魂审查结果: {len(tianhun_map)} 条 (agent,tick) 映射")

    # 4. 转换为 SFT 样本 + 筛选
    samples: List[dict] = []
    stats: Dict[str, int] = defaultdict(int)
    for trace in traces:
        agent_id = trace.get("agent_id", "")
        tick_id = trace.get("tick_id", 0)
        tianhun_result = tianhun_map.get((agent_id, tick_id))

        # 筛选：approved（若启用天魂筛选）
        if approved_only and tianhun_result != "approved":
            stats[f"跳过(tianhun={tianhun_result or '无数据'})"] += 1
            continue

        sample = trace_to_sft_sample(trace, tianhun_result)
        if sample:
            samples.append(sample)
            stats["导出"] += 1
        else:
            stats["跳过(无response)"] += 1

    # 5. 输出
    output_path = Path(args.output)
    with open(output_path, "w", encoding="utf-8") as f:
        for sample in samples:
            f.write(json.dumps(sample, ensure_ascii=False) + "\n")

    # 6. 统计
    print()
    print(f"=== 导出完成 ===")
    print(f"输出文件: {output_path} ({len(samples)} 条样本)")
    print()
    print("--- 统计 ---")
    for key, count in sorted(stats.items()):
        print(f"  {key}: {count}")

    # agent 分布
    agent_dist: Dict[str, int] = defaultdict(int)
    for s in samples:
        agent_dist[s["metadata"]["agent_id"][:8]] += 1
    print()
    print(f"--- Agent 分布（top 10）---")
    for aid, count in sorted(agent_dist.items(), key=lambda x: x[1], reverse=True)[:10]:
        print(f"  {aid}...: {count} 条")

    print()
    print("注：本脚本为离线导出工具，未写回任何 agent 状态。")


if __name__ == "__main__":
    main()
