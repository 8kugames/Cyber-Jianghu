//! crates/server/tests/locations_graph_integrity_test.rs
//!
//! 地点图数据画像守卫（吸收自传灯录 test_di_tu 的守卫模式）
//!
//! 对 shipped locations.yaml 固化当前数据画像为断言：任何让地图结构性
//! 变差的配置改动（断头路、孤儿节点、连通性破坏、可达性预算超标）都必须
//! 显式更新本文件对应的画像常量，而非静默通过 CI。
//!
//! 运行: cargo test --test locations_graph_integrity_test
//! 预期: 全部通过（画像与 shipped 配置一致）

#[cfg(test)]
mod tests {
    use cyber_jianghu_server::game_data::loaders::{load_game_rules, load_locations};
    use cyber_jianghu_server::paths::get_config_dir;
    use std::collections::{HashMap, HashSet, VecDeque};

    /// 可达性预算上限：全图任意两点最短 travel_cost 之和不得超过此值。
    ///
    /// 画像依据（2026-09-11）：当前 shipped 图 worst = 3 tick
    /// （任意子场景 → 荒漠/酒泉：1 入龙门客栈 + 2 或经河西走廊 1+1+1）。
    /// 语义锚点：不超过半个游戏日（12 游戏小时 = 12 tick）的一半。
    /// 若新增远端地块使 worst 破 6，需重新评估此预算（而非静默接受）。
    const MAX_PAIRWISE_TRAVEL_BUDGET: u32 = 6;

    struct Graph {
        nodes: HashSet<String>,
        /// 邻接（含显式边 + 隐式 parent-child，取两者较小 cost）
        adj: HashMap<String, Vec<(String, u32)>>,
        /// 显式有向边集合（from, to）
        declared: HashSet<(String, String)>,
    }

    fn load_graph() -> Graph {
        let config_dir = get_config_dir();
        let config =
            load_locations(&config_dir).expect("shipped locations.yaml 应能加载并通过校验");
        let rules = load_game_rules(&config_dir).expect("shipped game_rules.yaml 应能加载");
        let implicit_cost = rules.data.agent_state.location.default_implicit_travel_cost;

        let mut nodes = HashSet::new();
        let mut parents: HashMap<String, String> = HashMap::new();
        for n in &config.data.nodes {
            nodes.insert(n.node_id.clone());
            if let Some(p) = &n.parent_id
                && !p.is_empty()
            {
                parents.insert(n.node_id.clone(), p.clone());
            }
        }

        let mut adj: HashMap<String, Vec<(String, u32)>> = HashMap::new();
        let mut declared = HashSet::new();
        let push =
            |adj: &mut HashMap<String, Vec<(String, u32)>>, from: &str, to: &str, cost: u32| {
                adj.entry(from.to_string())
                    .or_default()
                    .push((to.to_string(), cost));
            };

        for e in &config.data.edges {
            declared.insert((e.from.clone(), e.to.clone()));
            push(&mut adj, &e.from, &e.to, e.travel_cost.max(0) as u32);
        }
        // 隐式 parent-child（双向，与 LocationGraph::get_implicit_neighbors 同构）
        for (child, parent) in &parents {
            push(&mut adj, child, parent, implicit_cost);
            push(&mut adj, parent, child, implicit_cost);
        }

        Graph {
            nodes,
            adj,
            declared,
        }
    }

    /// Dijkstra 最短 travel_cost（小图 O(V^2) 足够，无需二叉堆）
    fn shortest_cost(g: &Graph, from: &str, to: &str) -> Option<u32> {
        if from == to {
            return Some(0);
        }
        let mut dist: HashMap<&str, u32> = HashMap::from([(from, 0)]);
        let mut visited: HashSet<&str> = HashSet::new();
        while let Some((&u, &du)) = dist
            .iter()
            .filter(|(k, _)| !visited.contains(*k))
            .min_by_key(|(_, d)| **d)
        {
            visited.insert(u);
            if u == to {
                return Some(du);
            }
            for (v, cost) in g.adj.get(u).into_iter().flatten() {
                let alt = du.saturating_add(*cost);
                let better = dist.get(v.as_str()).is_none_or(|d| alt < *d);
                if better {
                    dist.insert(v.as_str(), alt);
                }
            }
        }
        None
    }

    #[test]
    fn test_graph_is_fully_connected() {
        let g = load_graph();
        assert!(!g.nodes.is_empty(), "地图不应为空");

        let source = g.nodes.iter().next().unwrap().clone();
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue = VecDeque::from([source.clone()]);
        visited.insert(source.clone());
        while let Some(u) = queue.pop_front() {
            for (v, _) in g.adj.get(&u).into_iter().flatten() {
                if visited.insert(v.clone()) {
                    queue.push_back(v.clone());
                }
            }
        }

        let unreachable: Vec<&String> = g.nodes.difference(&visited).collect();
        assert!(
            unreachable.is_empty(),
            "存在从 {} 不可达的节点（孤儿/孤岛）: {:?}",
            source,
            unreachable
        );
    }

    #[test]
    fn test_asymmetric_edge_profile() {
        // 画像守卫：shipped 图当前所有显式边均为双向声明（cost 可不同，
        // 如 龙门客栈→荒漠=2 / 荒漠→龙门客栈=3 的“回程更累”）。
        // 若有意引入单向边，请把 (from, to) 断言在此处显式列出。
        let g = load_graph();
        let one_way: Vec<(&String, &String)> = g
            .declared
            .iter()
            .filter(|(from, to)| !g.declared.contains(&((*to).clone(), (*from).clone())))
            .map(|(f, t)| (f, t))
            .collect();
        assert!(
            one_way.is_empty(),
            "出现单向边 {:?}：若为有意设计（如断头路剧情），请在守卫中断言完整清单；否则补反向边",
            one_way
        );
    }

    #[test]
    fn test_all_explicit_travel_costs_positive() {
        let config_dir = get_config_dir();
        let config = load_locations(&config_dir).unwrap();
        for e in &config.data.edges {
            assert!(
                e.travel_cost >= 1,
                "边 {} → {} 的 travel_cost={} 应 >= 1（0 成本移动破坏体力语义）",
                e.from,
                e.to,
                e.travel_cost
            );
        }
    }

    #[test]
    fn test_pairwise_travel_budget() {
        let g = load_graph();
        let mut worst: u32 = 0;
        let mut worst_pair = String::new();
        let ids: Vec<String> = g.nodes.iter().cloned().collect();
        for from in &ids {
            for to in &ids {
                if let Some(cost) = shortest_cost(&g, from, to)
                    && cost > worst
                {
                    worst = cost;
                    worst_pair = format!("{} → {}", from, to);
                }
            }
        }
        assert!(
            worst <= MAX_PAIRWISE_TRAVEL_BUDGET,
            "全图最坏两点半径 worst={}（{}）破预算 {} tick：新增远端地块需重估预算或补通路",
            worst,
            worst_pair,
            MAX_PAIRWISE_TRAVEL_BUDGET
        );
    }

    #[test]
    fn test_spawn_location_exists_and_reachable() {
        let config_dir = get_config_dir();
        let config = load_locations(&config_dir).unwrap();
        let rules = load_game_rules(&config_dir).unwrap();
        let spawn = rules.data.agent_state.location.spawn_location.clone();
        assert!(!spawn.is_empty(), "spawn_location 不应为空");
        assert!(
            config.data.nodes.iter().any(|n| n.node_id == spawn),
            "game_rules.yaml 的 spawn_location '{}' 在 locations.yaml 中不存在",
            spawn
        );

        let g = load_graph();
        for node in &g.nodes {
            let cost = shortest_cost(&g, &spawn, node)
                .unwrap_or_else(|| panic!("出生点 {} 无法到达 {}", spawn, node));
            assert!(
                cost <= MAX_PAIRWISE_TRAVEL_BUDGET,
                "出生点 {} → {} 耗时 {} tick 破预算 {}",
                spawn,
                node,
                cost,
                MAX_PAIRWISE_TRAVEL_BUDGET
            );
        }
    }

    #[test]
    fn test_shipped_time_variants_well_formed() {
        // loader 已 fail-fast 校验 from<=to；此处固化 shipped 画像：
        // 当前无任何地点配置时代变体（机制就绪，内容待世界演化启用）。
        let config_dir = get_config_dir();
        let config = load_locations(&config_dir).unwrap();
        let with_variants: Vec<&str> = config
            .data
            .nodes
            .iter()
            .filter(|n| !n.time_variants.is_empty())
            .map(|n| n.node_id.as_str())
            .collect();
        assert!(
            with_variants.is_empty(),
            "shipped 图出现时代变体地点 {:?}：若有意启用世界演化，请更新此画像并确认对应可达性预算",
            with_variants
        );
    }
}
