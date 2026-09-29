#!/usr/bin/env python3
"""SFT 导出产物 → MLX LoRA 训练集准备。

用法: python3 scripts/prepare_sft_mlx.py <export.jsonl> <output_dir>

输入为 training_export 的 run=<ulid>.jsonl（{"messages":[...], "metadata":{...}}），
剥离 metadata（mlx-lm chat 格式只需 messages），固定种子 95/5 切分 train/valid。

合成增强动机：SFT 数据缺格式纠错反馈形态，模型在格式失败重试时分布外崩坏
（2026-09-28 归因实证）；--augment-fraction 对 normal 样本注入「[验证反馈]」
格式纠错前缀变体，补齐重试语境的分布覆盖。
"""

import argparse
import json
import random
from pathlib import Path

VALID_RATIO = 0.05
SEED = 17

# 格式纠错反馈的两种真实形态，与 crates/agent/src/runtime/decision.rs
# retry_feedback_for_error 一致：带 serde 错误细节（灰度期观测形态）与不带细节
# （现行格式解析错误形态，存量训练数据零覆盖）。外层包装 "[验证反馈]: {fb}\n"
# 与 crates/agent/src/soul/actor/engine_prompts.rs build_tick_message 一致。
FORMAT_ERROR_DETAIL_POOL = [
    "invalid type: null, expected a string at line 1 column {N}",
    "expected value at line 1 column {N}",
    "trailing characters at line 1 column {N}",
    "missing field `thought_process` at line 1 column {N}",
]


def augment_format_feedback_variants(records, fraction, rng):
    """对 normal 样本合成「[验证反馈]」格式纠错前缀变体。

    normal 定义：user 消息（messages[1]，导出恒为 system/user/assistant）
    不以 [验证反馈] 开头且不以 [解析反馈] 开头。随机抽 fraction 比例，
    每条变体仅在 user 消息前追加反馈前缀（带/不带错误细节两形态随机，
    error_detail 从池随机、N 为 1-400 随机整数），assistant/system 不变。
    返回 (原记录列表追加变体, 增强条数)。
    """
    if fraction <= 0:
        return records, 0
    normal_idx = [
        i
        for i, rec in enumerate(records)
        if not (
            rec["messages"][1]["content"].startswith("[验证反馈]")
            or rec["messages"][1]["content"].startswith("[解析反馈]")
        )
    ]
    k = int(len(normal_idx) * fraction)
    if k <= 0:
        return records, 0
    variants = []
    for i in rng.sample(normal_idx, k):
        msgs = records[i]["messages"]
        detail = rng.choice(FORMAT_ERROR_DETAIL_POOL).format(
            N=rng.randint(1, 400)
        )
        if rng.random() < 0.5:
            prefix = (
                "[验证反馈]: 系统提示：你上一次输出格式有误，"
                "请确保严格输出合法的JSON对象，不要在JSON外添加任何文本。\n"
            )
        else:
            prefix = (
                f"[验证反馈]: 系统提示：你上一次输出格式有误（{detail}），"
                "请确保严格输出合法的JSON对象，不要在JSON外添加任何文本。\n"
            )
        new_msgs = list(msgs)
        new_msgs[1] = {**msgs[1], "content": prefix + msgs[1]["content"]}
        variants.append({"messages": new_msgs})
    return records + variants, len(variants)


def main():
    parser = argparse.ArgumentParser(
        description="SFT 导出产物 → MLX LoRA 训练集准备"
    )
    parser.add_argument("src", help="training_export 的 run=<ulid>.jsonl")
    parser.add_argument("out_dir", help="输出目录")
    parser.add_argument(
        "--augment-fraction",
        type=float,
        default=0.15,
        help="对 normal 样本合成格式纠错反馈变体的比例（<=0 关闭）",
    )
    args = parser.parse_args()

    src = Path(args.src)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    records = []
    bad = 0
    with src.open(encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                bad += 1
                continue
            msgs = rec.get("messages")
            if not msgs or not isinstance(msgs, list):
                bad += 1
                continue
            records.append({"messages": msgs})

    rng = random.Random(SEED)
    records, n_augmented = augment_format_feedback_variants(
        records, args.augment_fraction, rng
    )
    rng.shuffle(records)
    n_valid = max(1, int(len(records) * VALID_RATIO)) if records else 0
    valid, train = records[:n_valid], records[n_valid:]

    for name, part in (("train.jsonl", train), ("valid.jsonl", valid)):
        path = out_dir / name
        with path.open("w", encoding="utf-8") as f:
            for rec in part:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")

    char_lens = sorted(len(json.dumps(r, ensure_ascii=False)) for r in records)
    mid = char_lens[len(char_lens) // 2] if char_lens else 0
    print(
        json.dumps(
            {
                "source": str(src),
                "total": len(records),
                "bad_skipped": bad,
                "train": len(train),
                "valid": len(valid),
                "augmented_format_feedback": n_augmented,
                "median_record_chars": mid,
                "seed": SEED,
            },
            ensure_ascii=False,
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
