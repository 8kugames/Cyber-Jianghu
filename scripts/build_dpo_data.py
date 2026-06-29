#!/usr/bin/env python3
"""
DPO（直接偏好优化）训练数据导出脚本

利用三魂架构的天魂驳回→人魂重试机制，产出 (chosen, rejected) 偏好对。
这是本游戏独有的 DPO 优势——天魂驳回后的纠正样本天然构成偏好对。

数据源（server 端）：
  - traces/soul=renhun/agent=<id>/date=*.jsonl  （人魂 LLM 调用，含真实 attempt）
  - DB agent_action_logs.soul_cycle_metadata    （天魂审查结果 approved/rejected）

配对逻辑（基于修复后的 attempt 字段，精确可靠）：
  同一 (agent_id, tick_id) 下：
    - attempt=N 被天魂 rejected → attempt=N+1 被 approved
    - 输出 (chosen=approved response, rejected=rejected response)
  self-correct 样本（同 attempt 内的纠正）也产出偏好对。

输出格式（兼容 DPO trainer / Axolotl）：
  {"prompt": {"role":"user","content":"..."},
   "chosen": {"role":"assistant","content":"..."},   // approved
   "rejected": {"role":"assistant","content":"..."},  // rejected
   "metadata": {"agent_id":"...","tick_id":42,...}}

用法：
  DATABASE_URL=postgres://... python scripts/build_dpo_data.py <data_dir> -o dpo.jsonl
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections import defaultdict
from pathlib import Path
from typing import Dict, List, Optional, Tuple


def load_traces_by_tick(data_dir: Path) -> Dict[Tuple[str, int], List[dict]]:
    """加载所有 Renhun trace，按 (agent_id, tick_id) 分组，每组按 attempt 排序。

    trace 现在记录真实的外层 soul_cycle attempt（修复后），
    同一 (agent_id, tick_id) 的多条 trace 可按 attempt 精确排列。
    """
    traces_dir = data_dir / "traces" / "soul=renhun"
    if not traces_dir.exists():
        print(f"[警告] trace 目录不存在: {traces_dir}", file=sys.stderr)
        return {}

    grouped: Dict[Tuple[str, int], List[dict]] = defaultdict(list)
    for jsonl_path in sorted(traces_dir.rglob("*.jsonl")):
        with open(jsonl_path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                    # 只取 renhun trace
                    if entry.get("soul_stage") != "renhun":
                        continue
                    agent_id = entry.get("agent_id", "")
                    tick_id = entry.get("tick_id", 0)
                    grouped[(agent_id, tick_id)].append(entry)
                except json.JSONDecodeError as e:
                    print(f"[警告] 解析 trace 行失败 {jsonl_path}: {e}", file=sys.stderr)

    # 每组按 attempt 排序（修复后 attempt 字段可靠）
    for key in grouped:
        grouped[key].sort(key=lambda t: (t.get("attempt", 0), t.get("wall_clock", "")))

    return grouped


def load_tianhun_results(database_url: str) -> Dict[Tuple[str, int], List[Tuple[int, str]]]:
    """从 DB 查询每个 (agent_id, tick_id) 的所有 attempt 审查结果。

    soul_cycle_metadata 按 (agent_id, tick_id, pipe_seq) keyed，
    解析 cycles 数组，返回 {(agent_id, tick_id): [(attempt, result), ...]}。

    返回的列表按 attempt 排序。
    """
    try:
        import psycopg2
    except ImportError:
        print("[错误] 需要 psycopg2：pip install psycopg2-binary", file=sys.stderr)
        sys.exit(1)

    conn = psycopg2.connect(database_url)
    conn.set_session(readonly=True)
    cursor = conn.cursor()

    cursor.execute(
        """
        SELECT agent_id::text, tick_id, soul_cycle_metadata
        FROM agent_action_logs
        WHERE soul_cycle_metadata IS NOT NULL
        """
    )

    results: Dict[Tuple[str, int], List[Tuple[int, str]]] = defaultdict(list)
    for agent_id, tick_id, metadata in cursor:
        if metadata is None:
            continue
        try:
            if isinstance(metadata, str):
                metadata = json.loads(metadata)
            cycles = metadata.get("cycles", [])
            for cycle in cycles:
                attempt = cycle.get("attempt", 0)
                tianhun = cycle.get("tianhun", {})
                result = tianhun.get("result", "unknown")
                results[(agent_id, tick_id)].append((attempt, result))
        except (json.JSONDecodeError, KeyError, TypeError) as e:
            print(f"[警告] 解析 metadata 失败 agent={agent_id} tick={tick_id}: {e}", file=sys.stderr)

    conn.close()

    # 按 attempt 排序
    for key in results:
        results[key].sort(key=lambda x: x[0])

    return results


def build_dpo_pairs(
    traces: Dict[Tuple[str, int], List[dict]],
    tianhun_map: Dict[Tuple[str, int], List[Tuple[int, str]]],
) -> List[dict]:
    """构建 DPO (chosen, rejected) 偏好对。

    配对规则：
    1. 同 (agent_id, tick_id) 下，按 attempt 序列
    2. 找连续的 (rejected_N, approved_{N+1}) 对
    3. rejected 的 response = rejected trace 的 response
       approved 的 response = approved trace 的 response
    4. prompt 共享（同一 tick 的 user_prompt 基本相同）

    注意：trace 的 attempt 现在是真实外层 attempt（修复后可靠）。
    """
    dpo_samples: List[dict] = []
    stats: Dict[str, int] = defaultdict(int)

    for (agent_id, tick_id), tick_traces in traces.items():
        # 按 attempt 分组 trace（同 attempt 可能有多条——内层重试/self-correct）
        # 取每个 attempt 的最后一条 trace（最终输出）
        attempt_traces: Dict[int, dict] = {}
        for trace in tick_traces:
            attempt = trace.get("attempt", 0)
            attempt_traces[attempt] = trace  # 后出现的覆盖（取最后一条）

        # 获取该 tick 的天魂审查序列
        tianhun_seq = tianhun_map.get((agent_id, tick_id), [])

        # 建 attempt → result 映射
        attempt_results: Dict[int, str] = {att: res for att, res in tianhun_seq}

        # 找连续的 rejected → approved 对
        sorted_attempts = sorted(attempt_traces.keys())
        for i in range(len(sorted_attempts) - 1):
            att_n = sorted_attempts[i]
            att_n1 = sorted_attempts[i + 1]

            result_n = attempt_results.get(att_n, "unknown")
            result_n1 = attempt_results.get(att_n1, "unknown")

            # 配对：N rejected → N+1 approved
            if result_n == "rejected" and result_n1 in ("approved", "chaos_fallback"):
                rejected_trace = attempt_traces[att_n]
                chosen_trace = attempt_traces[att_n1]

                # 跳过无 response 的
                rejected_resp = rejected_trace.get("response", "").strip()
                chosen_resp = chosen_trace.get("response", "").strip()
                if not rejected_resp or not chosen_resp:
                    stats["跳过(无response)"] += 1
                    continue

                # prompt：用 chosen（纠正后）的 user_prompt，因含驳回反馈
                user_prompt = chosen_trace.get("user_prompt", "")
                persona_name = chosen_trace.get("persona_name", "")
                persona_description = chosen_trace.get("persona_description", "")

                # DPO prompt 是 messages 数组（含 system persona + user），与 SFT 对齐
                prompt_messages: List[dict] = []
                if persona_name:
                    system_content = f"你是 {persona_name}。"
                    if persona_description:
                        system_content += f"\n{persona_description}"
                    prompt_messages.append({"role": "system", "content": system_content})
                prompt_messages.append({"role": "user", "content": user_prompt})

                sample = {
                    "prompt": prompt_messages,
                    "chosen": {"role": "assistant", "content": chosen_resp},
                    "rejected": {"role": "assistant", "content": rejected_resp},
                    "metadata": {
                        "agent_id": agent_id,
                        "tick_id": tick_id,
                        "rejected_attempt": att_n,
                        "chosen_attempt": att_n1,
                        "rejected_tianhun": result_n,
                        "chosen_tianhun": result_n1,
                    },
                }
                dpo_samples.append(sample)
                stats["配对成功"] += 1
            else:
                stats[f"跳过({result_n}→{result_n1})"] += 1

    return dpo_samples, stats


def main() -> None:
    parser = argparse.ArgumentParser(
        description="DPO 训练数据导出（天魂 reject→approve 偏好对）"
    )
    parser.add_argument("data_dir", type=str, help="server 数据目录（含 traces/）")
    parser.add_argument("-o", "--output", type=str, default="dpo_dataset.jsonl")
    args = parser.parse_args()

    data_dir = Path(args.data_dir).expanduser()
    if not data_dir.is_dir():
        print(f"[错误] 数据目录不存在: {data_dir}", file=sys.stderr)
        sys.exit(1)

    print("=== DPO 数据导出 ===")
    print(f"数据目录: {data_dir}")
    print()

    # 1. 加载 trace（按 tick 分组）
    traces = load_traces_by_tick(data_dir)
    total_ticks = len(traces)
    total_traces = sum(len(v) for v in traces.values())
    print(f"加载人魂 trace: {total_traces} 条，覆盖 {total_ticks} 个 (agent,tick)")
    if not traces:
        print("无 trace 数据，导出结束。")
        return

    # 2. 加载天魂审查结果
    db_url = os.environ.get("DATABASE_URL")
    if not db_url:
        print("[错误] 需要 DATABASE_URL 查天魂审查结果", file=sys.stderr)
        sys.exit(1)
    tianhun_map = load_tianhun_results(db_url)
    print(f"加载天魂审查结果: {len(tianhun_map)} 个 (agent,tick) 映射")

    # 3. 构建 DPO 配对
    dpo_samples, stats = build_dpo_pairs(traces, tianhun_map)

    # 4. 输出
    output_path = Path(args.output)
    with open(output_path, "w", encoding="utf-8") as f:
        for sample in dpo_samples:
            f.write(json.dumps(sample, ensure_ascii=False) + "\n")

    # 5. 统计
    print()
    print("=== 导出完成 ===")
    print(f"输出文件: {output_path} ({len(dpo_samples)} 条偏好对)")
    print()
    print("--- 配对统计 ---")
    for key, count in sorted(stats.items()):
        print(f"  {key}: {count}")

    # agent 分布
    agent_dist: Dict[str, int] = defaultdict(int)
    for s in dpo_samples:
        agent_dist[s["metadata"]["agent_id"][:8]] += 1
    print()
    print("--- Agent 分布（top 10）---")
    for aid, count in sorted(agent_dist.items(), key=lambda x: x[1], reverse=True)[:10]:
        print(f"  {aid}...: {count} 对")

    print()
    print("注：本脚本为离线导出工具，未写回任何 agent 状态。")


if __name__ == "__main__":
    main()
