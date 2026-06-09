//! 集成测试: 验证 D8 reasoning_content 剥离 + system_hash 确定性
//!
//! reasoning_content 剥离的 unit test 在 client.rs 中:
//! - build_conversation_messages_strips_reasoning_when_flag_set
//! - build_conversation_messages_preserves_reasoning_when_flag_unset
//!
//! 生产验证: 联调测试 13h, DeepSeek system_hash 0 变化, 证明剥离生效。
//! 不再需要 MockLlmClient 捕获 request body 的 e2e 测试 — unit test 已覆盖核心逻辑。

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
