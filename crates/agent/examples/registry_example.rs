// ============================================================================
// 全局记忆注册表示例
// ============================================================================
//
// 演示如何使用 GlobalMemoryRegistry 获取所有 Agent 的历史记忆
// ============================================================================

use cyber_jianghu_agent::GlobalMemoryRegistry;

fn main() {
    println!("全局记忆注册表演示");
    println!("{}", "=".repeat(60));

    // 创建注册表（使用默认目录 ~/.cyber-jianghu/）
    let mut registry = GlobalMemoryRegistry::with_default_dir();

    // 扫描所有 Agent 数据库
    println!("\n扫描 ~/.cyber-jianghu/ 目录...");
    match registry.scan() {
        Ok(count) => {
            println!("发现 {} 个 Agent 数据库\n", count);

            if count == 0 {
                println!("暂无历史数据，请先运行 basic 示例创建 Agent");
                return;
            }

            // 显示每个 Agent 的信息
            println!("Agent 生涯列表:");
            println!("{}", "-".repeat(60));
            for (i, agent) in registry.agents().iter().enumerate() {
                println!("{}. Agent ID: {}", i + 1, agent.agent_id);
                println!("   记忆数量: {}", agent.memory_count);
                println!(
                    "   Tick 范围: {:?} ~ {:?}",
                    agent.earliest_tick, agent.latest_tick
                );
                println!("{}", "-".repeat(60));
            }

            // 显示统计
            println!("\n统计汇总:");
            println!("   Agent 总数: {}", registry.agent_count());
            println!("   记忆总数: {}", registry.total_memory_count());

            // 获取最重要的记忆
            println!("\n所有 Agent 最重要的记忆 (Top 5):");
            match registry.get_top_memories(5) {
                Ok(memories) => {
                    for (i, mem) in memories.iter().enumerate() {
                        println!(
                            "   {}. [Agent {}] {} (重要性: {:.2})",
                            i + 1,
                            &mem.agent_id.to_string()[..8],
                            mem.content,
                            mem.importance_score
                        );
                    }
                }
                Err(e) => println!("   获取失败: {}", e),
            }

            // 生成全局报告
            println!("\n生成全局记忆报告...");
            match registry.generate_report(100, 24, 10) {
                Ok(report) => {
                    println!("   报告生成成功");
                    println!("   - Agent 总数: {}", report.total_agents);
                    println!("   - 记忆总数: {}", report.total_memories);
                    println!("   - 时间范围内记忆: {}", report.memories_in_range.len());
                    println!("   - Top 记忆数: {}", report.top_memories.len());

                    // 导出为 JSON
                    println!("\nJSON 格式报告:");
                    match serde_json::to_string_pretty(&report) {
                        Ok(json) => println!("{}", json),
                        Err(e) => println!("   JSON 序列化失败: {}", e),
                    }
                }
                Err(e) => println!("   生成报告失败: {}", e),
            }
        }
        Err(e) => {
            println!("扫描失败: {}", e);
        }
    }

    println!("\n演示完成");
}
