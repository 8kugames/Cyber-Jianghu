#!/usr/bin/env python3
"""
离线社会结构分析 —— 江湖涌现观察工具（观察 ≠ 干预）

哲学定位：
  关系/声望是众生主观认知，不进天道 reward（Phase 1 已定）。
  本脚本是研究者的"望远镜"——一次性读取所有 agent 的关系快照，
  做恩怨分图 PageRank，观察"谁被公认是大侠/魔头"。
  只读 SQLite，不写回任何 agent 状态，不进运行时。

数据源：
  遍历 <data_dir>/relationships_{agent_id}.db（每 agent 一个 SQLite）
  读取 relationships 表的 (target_agent_id, favorability)

算法：
  恩图：favorability > 0 的边，权重 = favorability / 100
  怨图：favorability < 0 的边，权重 = -favorability / 100
  两图分别跑 PageRank（damping=0.85）

用法：
  python scripts/analyze_social_structure.py <data_dir>
  python scripts/analyze_social_structure.py ~/.cyber-jianghu

输出（stdout）：
  - 恩望 PageRank Top 10（"公认正派领袖"）
  - 恶名 PageRank Top 10（"武林公敌"）
  - 不对称关系统计（单向仰慕、双向仇恨等）
"""

from __future__ import annotations

import argparse
import sqlite3
import sys
from collections import defaultdict
from pathlib import Path
from typing import Dict, List, Tuple

Graph = Dict[str, Dict[str, float]]  # source -> {target: weight}


def load_all_edges(data_dir: Path) -> List[Tuple[str, str, int]]:
    """遍历所有 relationships_{agent_id}.db，收集有向边 (source, target, favorability)。"""
    edges: List[Tuple[str, str, int]] = []
    db_files = sorted(data_dir.glob("relationships_*.db"))

    if not db_files:
        print(f"[警告] 未在 {data_dir} 找到 relationships_*.db 文件", file=sys.stderr)
        return edges

    for db_path in db_files:
        # 从文件名提取 source agent_id
        source_id = db_path.stem.replace("relationships_", "")
        try:
            conn = sqlite3.connect(str(db_path))
            conn.row_factory = sqlite3.Row
            cursor = conn.execute(
                "SELECT target_agent_id, favorability FROM relationships"
            )
            for row in cursor:
                target = row["target_agent_id"]
                fav = row["favorability"]
                if fav != 0:  # 忽略中性边
                    edges.append((source_id, target, fav))
            conn.close()
        except sqlite3.Error as e:
            print(f"[警告] 读取 {db_path} 失败: {e}", file=sys.stderr)

    return edges


def build_favor_ire_graphs(
    edges: List[Tuple[str, str, int]]
) -> Tuple[Graph, Graph]:
    """构建恩图（正 favorability）和怨图（负 favorability）。"""
    favor_graph: Graph = defaultdict(dict)
    ire_graph: Graph = defaultdict(dict)

    for source, target, fav in edges:
        if fav > 0:
            favor_graph[source][target] = fav / 100.0
        elif fav < 0:
            ire_graph[source][target] = (-fav) / 100.0

    return dict(favor_graph), dict(ire_graph)


def pagerank(
    graph: Graph, damping: float = 0.85, max_iter: int = 100, tol: float = 1e-6
) -> Dict[str, float]:
    """带权有向图 PageRank。返回归一化到 [0,1] 的分数。"""
    nodes: set[str] = set(graph.keys())
    for targets in graph.values():
        nodes.update(targets.keys())
    nodes_sorted = sorted(nodes)
    n = len(nodes_sorted)
    if n == 0:
        return {}

    # 归一化出边权重
    out_weights: Graph = {}
    for src, targets in graph.items():
        total = sum(targets.values())
        if total > 0:
            out_weights[src] = {t: w / total for t, w in targets.items()}

    scores = {node: 1.0 / n for node in nodes_sorted}

    for _ in range(max_iter):
        new_scores = {node: (1.0 - damping) / n for node in nodes_sorted}
        for src in nodes_sorted:
            outs = out_weights.get(src)
            if not outs:
                # 悬挂节点：均分给所有
                for node in nodes_sorted:
                    new_scores[node] += damping * scores[src] / n
                continue
            for tgt, weight in outs.items():
                new_scores[tgt] += damping * scores[src] * weight

        diff = sum(abs(new_scores[node] - scores[node]) for node in nodes_sorted)
        scores = new_scores
        if diff < tol:
            break

    return scores


def analyze_asymmetry(edges: List[Tuple[str, str, int]]) -> Dict[str, int]:
    """统计关系不对称模式：单向仰慕、双向仇恨、双向好感等。"""
    # source -> {target: favorability} 的正向查找
    edge_map: Dict[str, Dict[str, int]] = defaultdict(dict)
    for s, t, f in edges:
        edge_map[s][t] = f

    mutual_favor = 0  # 互为好感
    mutual_ire = 0  # 互为仇恨
    one_way_favor = 0  # 单向好感
    one_way_ire = 0  # 单向仇恨
    mixed = 0  # 一方好感一方仇恨（爱恨交织）

    seen = set()
    for s, t, f in edges:
        key = tuple(sorted([s, t]))
        if key in seen:
            continue
        seen.add(key)

        reverse = edge_map.get(t, {}).get(s)
        if reverse is None:
            # 单向
            if f > 0:
                one_way_favor += 1
            else:
                one_way_ire += 1
        else:
            # 双向
            if f > 0 and reverse > 0:
                mutual_favor += 1
            elif f < 0 and reverse < 0:
                mutual_ire += 1
            else:
                mixed += 1

    return {
        "互为好感（知己/挚友）": mutual_favor,
        "互为仇恨（宿敌）": mutual_ire,
        "单向好感（仰慕）": one_way_favor,
        "单向仇恨（单方面记仇）": one_way_ire,
        "爱恨交织（一方恩一方怨）": mixed,
    }


def main() -> None:
    parser = argparse.ArgumentParser(
        description="离线社会结构分析 —— 江湖涌现观察工具（只读，不写回 agent 状态）"
    )
    parser.add_argument(
        "data_dir",
        type=str,
        help="agent 数据目录（含 relationships_*.db），如 ~/.cyber-jianghu",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=10,
        help="排行显示前 N 名（默认 10）",
    )
    args = parser.parse_args()

    data_dir = Path(args.data_dir).expanduser()
    if not data_dir.is_dir():
        print(f"[错误] 数据目录不存在: {data_dir}", file=sys.stderr)
        sys.exit(1)

    print(f"=== 江湖社会结构分析 ===")
    print(f"数据目录: {data_dir}")
    print()

    # 1. 加载所有边
    edges = load_all_edges(data_dir)
    print(f"收集到 {len(edges)} 条关系边")
    if not edges:
        print("无关系数据，分析结束。")
        return
    print()

    # 2. 恩怨分图
    favor_graph, ire_graph = build_favor_ire_graphs(edges)
    print(f"恩图: {sum(len(v) for v in favor_graph.values())} 条边")
    print(f"怨图: {sum(len(v) for v in ire_graph.values())} 条边")
    print()

    # 3. PageRank
    favor_scores = pagerank(favor_graph)
    ire_scores = pagerank(ire_graph)

    # 4. 恩望排行（公认正派）
    print(f"--- 恩望 PageRank Top {args.top}（公认正派领袖 / 德高望重者）---")
    favor_ranked = sorted(favor_scores.items(), key=lambda x: x[1], reverse=True)
    for i, (agent_id, score) in enumerate(favor_ranked[: args.top], 1):
        print(f"  {i:>2}. {agent_id[:8]}...  恩望={score:.4f}")
    print()

    # 5. 恶名排行（武林公敌）
    print(f"--- 恶名 PageRank Top {args.top}（武林公敌 / 恶名昭著者）---")
    ire_ranked = sorted(ire_scores.items(), key=lambda x: x[1], reverse=True)
    for i, (agent_id, score) in enumerate(ire_ranked[: args.top], 1):
        print(f"  {i:>2}. {agent_id[:8]}...  恶名={score:.4f}")
    print()

    # 6. 不对称关系统计
    print("--- 关系不对称模式统计 ---")
    asym = analyze_asymmetry(edges)
    for pattern, count in asym.items():
        print(f"  {pattern}: {count}")
    print()

    print("=== 分析完成 ===")
    print("注：本脚本为观察工具，未写回任何 agent 状态。")


if __name__ == "__main__":
    main()
