#!/usr/bin/env python3
"""
生成黄金对照基线.

复刻 scripts/build_sft_data.py:158-197 的 trace_to_sft_sample 逻辑
(--no-db-filter 模式, tianhun_result=None), 对固定输入 fixture 产出期望 SFT JSONL.

Rust 测试 (training_export_golden_test.rs) 读相同输入 + 对照本脚本产出,
验证 transform 纯函数与 Python 基线不漂移.

用法: python3 generate_golden.py
产出: input_traces.jsonl + expected_samples.jsonl
"""

import json
from pathlib import Path


def trace_to_sft_sample(trace, tianhun_result=None):
    """逐字复刻 build_sft_data.py:158-197 (--no-db-filter 模式)."""
    response = trace.get("response", "").strip()
    if not response or not trace.get("ok", True):
        return None

    user_prompt = trace.get("user_prompt", "")
    persona_name = trace.get("persona_name", "")
    persona_description = trace.get("persona_description", "")

    messages = []
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
            "tianhun_result": tianhun_result,  # None in --no-db-filter mode
            "trace_id": trace.get("trace_id", ""),
        },
    }


# 固定输入 fixture: 覆盖 4 种边界 + 正常路径
FIXTURES = [
    # 1. 正常: persona name + desc + ok + response
    {
        "trace_id": "golden-001",
        "agent_id": "00000000-0000-0000-0000-000000000001",
        "character_name": "张三",
        "tick_id": 100,
        "soul_stage": "Renhun",
        "attempt": 0,
        "provider": "openai",
        "model": "gpt-4",
        "persona_name": "张三",
        "persona_description": "一位侠客",
        "user_prompt": "今日如何?",
        "response": "  出门练剑。  ",
        "prompt_tokens": None,
        "completion_tokens": None,
        "ok": True,
        "wall_clock": None,
    },
    # 2. ok=false -> 跳过 (期望: 不产出)
    {
        "trace_id": "golden-002",
        "agent_id": "00000000-0000-0000-0000-000000000002",
        "character_name": "李四",
        "tick_id": 101,
        "soul_stage": "Renhun",
        "attempt": 0,
        "provider": "test",
        "model": "m",
        "persona_name": "李四",
        "persona_description": "描述",
        "user_prompt": "问",
        "response": "答",
        "prompt_tokens": None,
        "completion_tokens": None,
        "ok": False,
        "wall_clock": None,
    },
    # 3. response 空 -> 跳过 (期望: 不产出)
    {
        "trace_id": "golden-003",
        "agent_id": "00000000-0000-0000-0000-000000000003",
        "character_name": "王五",
        "tick_id": 102,
        "soul_stage": "Renhun",
        "attempt": 0,
        "provider": "test",
        "model": "m",
        "persona_name": "王五",
        "persona_description": "描述",
        "user_prompt": "问",
        "response": "   ",
        "prompt_tokens": None,
        "completion_tokens": None,
        "ok": True,
        "wall_clock": None,
    },
    # 4. persona name 有, desc 空 -> system 只有 "你是 {name}。"
    {
        "trace_id": "golden-004",
        "agent_id": "00000000-0000-0000-0000-000000000004",
        "character_name": "赵六",
        "tick_id": 103,
        "soul_stage": "Renhun",
        "attempt": 1,
        "provider": "anthropic",
        "model": "claude",
        "persona_name": "赵六",
        "persona_description": "",
        "user_prompt": "做什么?",
        "response": "读书",
        "prompt_tokens": None,
        "completion_tokens": None,
        "ok": True,
        "wall_clock": None,
    },
    # 5. persona 双空 -> 无 system, messages 只含 user+assistant (不跳过)
    {
        "trace_id": "golden-005",
        "agent_id": "00000000-0000-0000-0000-000000000005",
        "character_name": "Agent",
        "tick_id": 104,
        "soul_stage": "Renhun",
        "attempt": 0,
        "provider": "test",
        "model": "m",
        "persona_name": "",
        "persona_description": "",
        "user_prompt": "go",
        "response": "ok",
        "prompt_tokens": None,
        "completion_tokens": None,
        "ok": True,
        "wall_clock": None,
    },
]


def main():
    out_dir = Path(__file__).parent

    # 写输入 fixture
    with open(out_dir / "input_traces.jsonl", "w", encoding="utf-8") as f:
        f.writelines(
            json.dumps(trace, ensure_ascii=False) + "\n" for trace in FIXTURES
        )

    # 产出期望样本 (--no-db-filter 模式, tianhun_result=None)
    with open(out_dir / "expected_samples.jsonl", "w", encoding="utf-8") as f:
        for trace in FIXTURES:
            sample = trace_to_sft_sample(trace, tianhun_result=None)
            if sample is not None:
                f.write(json.dumps(sample, ensure_ascii=False) + "\n")

    print(f"生成 {len(FIXTURES)} 条输入, 期望产出见 expected_samples.jsonl")


if __name__ == "__main__":
    main()
