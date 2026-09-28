#!/usr/bin/env python3
"""SFT 微调验收评测（docs/training-handover.md 6.3 三项指标）。

用法:
  教师参考基线（不跑模型，直接评 held-out 教师输出）:
    python3 scripts/eval_sft_mlx.py --data sft_data/valid.jsonl --teacher
  底座 zero-shot 对照:
    python3 scripts/eval_sft_mlx.py --data sft_data/valid.jsonl --model mlx-community/Qwen3-0.6B-bf16
  微调模型:
    python3 scripts/eval_sft_mlx.py --data sft_data/valid.jsonl --model mlx-community/Qwen3-0.6B-bf16 \
      --adapter adapters/qwen3-06b-sft-v1

JSON 解析口径对齐 scripts/parse_tianhun_labels.py：剥 code fence、json.loads、
失败后取第一个平衡花括号片段重试。assistant 生成可能带 <think> 块，先剥离再解析。

指标（held-out 5%）:
  json_valid_rate         输出可解析为 dict 且含 actions 键（硬门槛 >=0.95）
  action_type_valid_rate  action_type ∈ actions.yaml 动作集（硬门槛 >=0.90;
                          注意存量教师 trace 可能含 actions.yaml 改名前的旧动作名,
                          以报告中的 teacher_action_type_valid_rate 为参照系）
  entity_match_rate       逐位置 (action_type + target_agent_id/item_id/target_location)
                          与教师输出一致（首期观测基线, 阈值待定）
"""

import argparse
import json
import random
import re
import sys
from collections import Counter
from pathlib import Path

import yaml

FENCE_RE = re.compile(r"^```(?:json)?\s*|\s*```")
THINK_RE = re.compile(r"<think>.*?</think>", re.DOTALL)
ENTITY_KEYS = ("target_agent_id", "item_id", "target_location")

# 阈值来自 docs/training-handover.md 6.3
JSON_VALID_THRESHOLD = 0.95
ACTION_VALID_THRESHOLD = 0.90


def strip_think(text: str) -> str:
    s = THINK_RE.sub("", text or "")
    # 未闭合的 think 块: 从 <think> 起全部视为思考, 无正文
    if "<think>" in s and "</think>" not in s:
        return ""
    return s.strip()


def lenient_parse(text: str):
    """宽松解析, 口径对齐 parse_tianhun_labels.extract_label 的前两级。

    返回 (obj, mode): mode ∈ strict / balanced / failed。
    """
    s = (text or "").strip()
    if not s:
        return None, "failed"
    candidate = FENCE_RE.sub("", s).strip()
    try:
        return json.loads(candidate), "strict"
    except (json.JSONDecodeError, ValueError):
        pass
    start = candidate.find("{")
    if start >= 0:
        depth = 0
        for i in range(start, len(candidate)):
            if candidate[i] == "{":
                depth += 1
            elif candidate[i] == "}":
                depth -= 1
                if depth == 0:
                    frag = candidate[start : i + 1]
                    try:
                        return json.loads(frag), "balanced"
                    except (json.JSONDecodeError, ValueError):
                        return None, "failed"
    return None, "failed"


def is_json_valid(obj) -> bool:
    return isinstance(obj, dict) and isinstance(obj.get("actions"), list)


def action_type_valid(obj, valid_actions: set) -> tuple[int, int]:
    """返回 (合法动作数, 总动作数)。"""
    if not is_json_valid(obj):
        return 0, 0
    acts = obj["actions"]
    ok = sum(
        1
        for a in acts
        if isinstance(a, dict) and a.get("action_type") in valid_actions
    )
    return ok, len(acts)


def entity_signature(action: dict) -> tuple:
    if not isinstance(action, dict):
        return ("<invalid>",)
    at = action.get("action_type")
    return (at,) + tuple(
        action.get("action_data", {}).get(k)
        for k in ENTITY_KEYS
        if isinstance(action.get("action_data"), dict)
    )


def entity_match(model_obj, teacher_obj) -> bool:
    """双方 actions 数量一致且逐位置 (action_type + 实体键) 一致。"""
    if not (is_json_valid(model_obj) and is_json_valid(teacher_obj)):
        return False
    m, t = model_obj["actions"], teacher_obj["actions"]
    if len(m) != len(t):
        return False
    return all(entity_signature(a) == entity_signature(b) for a, b in zip(m, t))


def load_actions(path: Path) -> set:
    data = yaml.safe_load(path.read_text(encoding="utf-8"))["data"]
    return set(data.keys())


def build_prompt(tokenizer, messages: list) -> str:
    chat = [m for m in messages if m["role"] != "assistant"]
    try:
        return tokenizer.apply_chat_template(
            chat,
            add_generation_prompt=True,
            tokenize=False,
            enable_thinking=False,
        )
    except TypeError:
        # 模板不接受 enable_thinking 时退回默认行为
        return tokenizer.apply_chat_template(
            chat, add_generation_prompt=True, tokenize=False
        )


def evaluate_teacher(records, valid_actions):
    n = len(records)
    json_ok = 0
    act_ok = act_total = 0
    invalid_types = Counter()
    for obj, _mode in records:
        if is_json_valid(obj):
            json_ok += 1
        ok, total = action_type_valid(obj, valid_actions)
        act_ok += ok
        act_total += total
        if is_json_valid(obj):
            for a in obj["actions"]:
                if (
                    isinstance(a, dict)
                    and a.get("action_type") not in valid_actions
                ):
                    invalid_types[a.get("action_type")] += 1
    return {
        "n": n,
        "json_valid_rate": round(json_ok / n, 4) if n else None,
        "action_type_valid_rate": round(act_ok / act_total, 4)
        if act_total
        else None,
        "total_actions": act_total,
        "invalid_action_types": dict(invalid_types.most_common()),
    }


def main():
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument(
        "--data", required=True, help="held-out jsonl (messages 格式)"
    )
    ap.add_argument(
        "--actions-yaml", default="crates/server/config/actions.yaml"
    )
    ap.add_argument(
        "--teacher", action="store_true", help="只评教师参考输出, 不跑模型"
    )
    ap.add_argument("--model", help="底座模型 (HF id 或本地路径)")
    ap.add_argument("--adapter", help="LoRA adapter 路径 (可选)")
    ap.add_argument("--max-tokens", type=int, default=2048)
    ap.add_argument("--limit", type=int, default=0, help="只评前 N 条 (调试)")
    ap.add_argument(
        "--sample",
        type=int,
        default=0,
        help="随机抽 N 条调试 (种子 17)",
    )
    ap.add_argument("--out", help="报告写入文件 (JSON), 默认只打印")
    args = ap.parse_args()

    if not args.teacher and not args.model:
        ap.error("需要 --teacher 或 --model 之一")

    valid_actions = load_actions(Path(args.actions_yaml))

    lines = [
        l
        for l in Path(args.data).read_text(encoding="utf-8").splitlines()
        if l.strip()
    ]
    if args.sample:
        rng = random.Random(17)
        idx = sorted(
            rng.sample(range(len(lines)), min(args.sample, len(lines)))
        )
        lines = [lines[i] for i in idx]
    if args.limit > 0:
        lines = lines[: args.limit]

    records = []  # (teacher_obj, messages)
    for l in lines:
        msgs = json.loads(l)["messages"]
        asst = next(m["content"] for m in msgs if m["role"] == "assistant")
        obj, _ = lenient_parse(asst)
        records.append((obj, msgs))

    teacher_metrics = evaluate_teacher(records, valid_actions)
    report = {
        "data": args.data,
        "n_samples": len(records),
        "actions_yaml_valid_set_size": len(valid_actions),
        "teacher": teacher_metrics,
        "thresholds": {
            "json_valid": JSON_VALID_THRESHOLD,
            "action_type_valid": ACTION_VALID_THRESHOLD,
        },
    }

    if args.teacher:
        report["mode"] = "teacher"
    else:
        import mlx_lm
        from mlx_lm.sample_utils import make_sampler

        model, tokenizer = mlx_lm.load(args.model, adapter_path=args.adapter)
        sampler = make_sampler(0.0)

        n = len(records)
        json_ok = 0
        act_ok = act_total = 0
        entity_ok = 0
        both_parsed = 0
        parse_fail_samples = []
        for i, (teacher_obj, msgs) in enumerate(records):
            prompt = build_prompt(tokenizer, msgs)
            out = mlx_lm.generate(
                model,
                tokenizer,
                prompt=prompt,
                max_tokens=args.max_tokens,
                sampler=sampler,
                verbose=False,
            )
            obj, mode = lenient_parse(strip_think(out))
            if is_json_valid(obj):
                json_ok += 1
            ok, total = action_type_valid(obj, valid_actions)
            act_ok += ok
            act_total += total
            if is_json_valid(obj) and is_json_valid(teacher_obj):
                both_parsed += 1
                if entity_match(obj, teacher_obj):
                    entity_ok += 1
            if not is_json_valid(obj) and len(parse_fail_samples) < 5:
                parse_fail_samples.append({"index": i, "raw_head": out[:300]})
            if (i + 1) % 25 == 0:
                print(
                    f"[{i + 1}/{n}] json_valid={json_ok}",
                    file=sys.stderr,
                    flush=True,
                )

        report["mode"] = "finetuned" if args.adapter else "base_zeroshot"
        report["model"] = args.model
        report["adapter"] = args.adapter
        report["model_metrics"] = {
            "json_valid_rate": round(json_ok / n, 4) if n else None,
            "action_type_valid_rate": round(act_ok / act_total, 4)
            if act_total
            else None,
            "total_actions": act_total,
            "entity_match_rate_all": round(entity_ok / n, 4) if n else None,
            "entity_match_rate_both_parsed": round(entity_ok / both_parsed, 4)
            if both_parsed
            else None,
            "parse_fail_samples": parse_fail_samples,
        }

    text = json.dumps(report, ensure_ascii=False, indent=2)
    if args.out:
        Path(args.out).write_text(text, encoding="utf-8")
        print(f"report -> {args.out}", file=sys.stderr)
    print(text)


if __name__ == "__main__":
    main()
