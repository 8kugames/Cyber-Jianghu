#!/usr/bin/env python3
"""SFT 导出产物 → MLX LoRA 训练集准备。

用法: python3 scripts/prepare_sft_mlx.py <export.jsonl> <output_dir>

输入为 training_export 的 run=<ulid>.jsonl（{"messages":[...], "metadata":{...}}），
剥离 metadata（mlx-lm chat 格式只需 messages），固定种子 95/5 切分 train/valid。
"""

import json
import random
import sys
from pathlib import Path

VALID_RATIO = 0.05
SEED = 17


def main():
    src = Path(sys.argv[1])
    out_dir = Path(sys.argv[2])
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
                "median_record_chars": mid,
                "seed": SEED,
            },
            ensure_ascii=False,
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
