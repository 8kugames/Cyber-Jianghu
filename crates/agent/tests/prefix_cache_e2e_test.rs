//! 集成测试: 验证 D8 双路径 (helper + inline) reasoning_content 剥离
//! spec §9 明确要求: 100 tick 模拟; 双路径 (helper + inline) 都验证 reasoning 不出现
//!
//! v2.1 修正: v2 plan 写的 3 个 unit test (compute_system_hash) 不满足 spec §9.
//! v2.1 改为真实 e2e: 用 MockLlmClient + 抓 outbound request body, 验证双路径
//!
//! 当前状态: 1 个 unit test (system_hash_deterministic) 可立即 PASS.
//! 另 2 个 e2e test (position_a, position_b) 用 `todo!()` 占位, 等 MockLlmClient
//! 扩展支持捕获 request body 后填充 (独立 PR).

use cyber_jianghu_agent::soul::actor::compute_system_hash;

#[test]
fn system_hash_deterministic_across_paths() {
    let system_a = "agent persona A + rules + actions + skills";
    let system_b = "agent persona A + rules + actions + skills";
    assert_eq!(compute_system_hash(system_a), compute_system_hash(system_b));
}

#[test]
fn system_hash_different_for_different_inputs() {
    let system_a = "agent persona A + rules + actions + skills";
    let system_b = "agent persona B + rules + actions + skills";
    assert_ne!(compute_system_hash(system_a), compute_system_hash(system_b));
}

#[test]
fn system_hash_returns_32_bytes() {
    let h = compute_system_hash("any content");
    assert_eq!(h.len(), 32);
}

#[tokio::test]
async fn position_a_helper_path_strips_reasoning() {
    // 跟踪: ISSUE-FOLLOWUP-MOCK-CAPTURE
    todo!("等 MockLlmClient 扩展支持捕获 request body 后填充");
}

#[tokio::test]
async fn position_b_inline_path_strips_reasoning() {
    // 跟踪: ISSUE-FOLLOWUP-MOCK-CAPTURE
    todo!("等 MockLlmClient 扩展支持捕获 request body 后填充");
}
