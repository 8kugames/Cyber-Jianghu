#!/usr/bin/env python3
"""解析天魂 trace 的 L3 审查标签分布。

用法: python3 scripts/parse_tianhun_labels.py <extracted_traces_root> <output_dir>
  <extracted_traces_root>: 解包后的目录，含 agent-*/data/traces/soul=tianhun/...
  <output_dir>: 输出目录，产出 labels.jsonl + stats.json
"""

import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

RESULT_RE = re.compile(r'"result"\s*:\s*"(approved|rejected)"')
REJECT_TYPE_RE = re.compile(r'"rejection_type"\s*:\s*"([a-z_]*)"')
FENCE_RE = re.compile(r"^```(?:json)?\s*|\s*```")

# Rust Debug 格式: LlmValidationResponse { result: "..", reason: "..", ... }
DEBUG_RESULT_RE = re.compile(r'\bresult:\s*"(approved|rejected)"')
DEBUG_REJECT_TYPE_RE = re.compile(r'\brejection_type:\s*"([a-z_]*)"')
DEBUG_REASON_RE = re.compile(r'\breason:\s*"((?:[^"\\]|\\.)*)"')

REJECT_TYPES_KNOWN = {
    "era_violation",
    "power_system_violation",
    "out_of_character",
    "meta_gaming",
    "semantic_repeat",
    "other",
}


def extract_label(response: str):
    """返回 (label, rejection_type, reason, parse_mode)。"""
    s = (response or "").strip()
    if not s:
        return "empty", "", "", "empty"
    candidate = FENCE_RE.sub("", s).strip()

    obj = None
    try:
        obj = json.loads(candidate)
    except (json.JSONDecodeError, ValueError):
        # 容忍前后杂文：找第一个 { 起的平衡片段失败时退回正则
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
                            obj = json.loads(frag)
                        except (json.JSONDecodeError, ValueError):
                            obj = None
                        break
    if isinstance(obj, dict) and "result" in obj:
        result = str(obj.get("result", "")).lower()
        rtype = str(obj.get("rejection_type", "") or "").lower()
        reason = str(obj.get("reason", "") or "")
        if result in ("approved", "rejected"):
            label = (
                "approved"
                if result == "approved"
                else f"rejected:{rtype or 'unspecified'}"
            )
            return label, rtype, reason, "json"
        return "unknown_result", "", reason, "json_unknown"

    # Rust Debug 格式（存量 trace 的主要形态）
    if "LlmValidationResponse" in candidate or DEBUG_RESULT_RE.search(
        candidate
    ):
        m = DEBUG_RESULT_RE.search(candidate)
        if m:
            result = m.group(1)
            t = DEBUG_REJECT_TYPE_RE.search(candidate)
            rtype = t.group(1) if t else ""
            r = DEBUG_REASON_RE.search(candidate)
            reason = (
                r.group(1).replace('\\"', '"').replace("\\n", "\n") if r else ""
            )
            label = (
                "approved"
                if result == "approved"
                else f"rejected:{rtype or 'unspecified'}"
            )
            return label, rtype, reason, "rust_debug"

    # 正则兜底
    m = RESULT_RE.search(candidate)
    if m:
        result = m.group(1)
        t = REJECT_TYPE_RE.search(candidate)
        rtype = t.group(1) if t else ""
        label = (
            "approved"
            if result == "approved"
            else f"rejected:{rtype or 'unspecified'}"
        )
        return label, rtype, "", "regex"

    # 粗粒度关键词（信息性，不计入可靠标签）
    if "驳回" in candidate or "拒绝" in candidate:
        return "keyword_reject", "", "", "keyword"
    if "通过" in candidate or "approved" in candidate:
        return "keyword_pass", "", "", "keyword"
    return "unparsed", "", "", "failed"


def main():
    root = Path(sys.argv[1])
    out_dir = Path(sys.argv[2])
    out_dir.mkdir(parents=True, exist_ok=True)

    total = 0
    ok_true = 0
    ok_false = 0
    labels = Counter()
    parse_modes = Counter()
    by_month = defaultdict(Counter)
    by_agent_file = Counter()
    unparsed_samples = []
    rows = []

    for stage_dir in sorted(root.glob("*/data/traces/soul=tianhun")):
        for fpath in sorted(stage_dir.glob("agent=*/date=*.jsonl")):
            if fpath.name.startswith("._"):
                continue
            agent_dir = fpath.parent.name
            date_part = fpath.name.replace("date=", "").replace(".jsonl", "")
            month = date_part[:7]
            for line in fpath.read_text(
                encoding="utf-8", errors="replace"
            ).splitlines():
                line = line.strip()
                if not line:
                    continue
                total += 1
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    labels["bad_json_line"] += 1
                    continue
                if not rec.get("ok", False):
                    ok_false += 1
                    continue
                ok_true += 1
                label, rtype, reason, mode = extract_label(
                    rec.get("response", "")
                )
                labels[label] += 1
                parse_modes[mode] += 1
                by_month[month][label] += 1
                by_agent_file[agent_dir] += 1
                if label == "unparsed" and len(unparsed_samples) < 20:
                    unparsed_samples.append(
                        {
                            "file": str(fpath),
                            "response_head": rec.get("response", "")[:300],
                        }
                    )
                rows.append(
                    {
                        "trace_id": rec.get("trace_id"),
                        "agent_id": rec.get("agent_id"),
                        "character_name": rec.get("character_name"),
                        "tick_id": rec.get("tick_id"),
                        "attempt": rec.get("attempt"),
                        "date": date_part,
                        "label": label,
                        "rejection_type": rtype,
                        "reason": reason[:200],
                        "provider": rec.get("provider"),
                        "model": rec.get("model"),
                    }
                )

    labels_path = out_dir / "labels.jsonl"
    with labels_path.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")

    approved = sum(v for k, v in labels.items() if k == "approved")
    rejected = sum(v for k, v in labels.items() if k.startswith("rejected:"))
    reliable = approved + rejected
    stats = {
        "total_lines": total,
        "ok_true": ok_true,
        "ok_false": ok_false,
        "reliable_labels": reliable,
        "approved": approved,
        "rejected_total": rejected,
        "rejected_by_type": {
            k: v for k, v in labels.items() if k.startswith("rejected:")
        },
        "reject_rate": round(rejected / reliable, 4) if reliable else None,
        "parse_modes": dict(parse_modes),
        "label_distribution": dict(labels),
        "by_month": {m: dict(c) for m, c in sorted(by_month.items())},
        "traces_per_character_dir": dict(by_agent_file),
        "unparsed_samples": unparsed_samples,
    }
    (out_dir / "stats.json").write_text(
        json.dumps(stats, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(
        json.dumps(
            {k: v for k, v in stats.items() if k != "unparsed_samples"},
            ensure_ascii=False,
            indent=2,
        )
    )
    if unparsed_samples:
        print("\n== unparsed 样例（前 3）==")
        for s in unparsed_samples[:3]:
            print(s["file"], "->", s["response_head"][:120].replace("\n", " "))


if __name__ == "__main__":
    main()
