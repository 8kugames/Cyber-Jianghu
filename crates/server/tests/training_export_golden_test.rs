//! 黄金对照测试
//!
//! 对照基准: crates/server/tests/sft_golden/expected_samples.jsonl
//! 该基准由 generate_golden.py 产出, 逐字复刻 scripts/build_sft_data.py:158-197
//! 的 trace_to_sft_sample 逻辑 (--no-db-filter 模式, tianhun_result=None).
//!
//! 本测试读 input_traces.jsonl 的每条 TraceEntry, 用 Rust transform_entry 转换,
//! 与 expected_samples.jsonl 逐条比对. 任何 persona 拼装/ok 过滤/response 空过滤
//! 的漂移都会在此暴露.

use cyber_jianghu_protocol::TraceEntry;
use cyber_jianghu_server::training_export::sft_transform::{TransformInput, transform_entry};

const INPUT_TRACES: &str = include_str!("sft_golden/input_traces.jsonl");
const EXPECTED_SAMPLES: &str = include_str!("sft_golden/expected_samples.jsonl");

/// 解析 JSONL 每行为强类型 TraceEntry
fn parse_input_traces() -> Vec<TraceEntry> {
    INPUT_TRACES
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| serde_json::from_str::<TraceEntry>(line).expect("input trace 解析失败"))
        .collect()
}

/// 解析 JSONL 每行为 serde_json::Value (期望样本, 不强类型化以容忍字段顺序)
fn parse_expected_samples() -> Vec<serde_json::Value> {
    EXPECTED_SAMPLES
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("expected 解析失败"))
        .collect()
}

#[test]
fn test_rust_transform_matches_python_golden() {
    let inputs = parse_input_traces();
    let expected = parse_expected_samples();

    // Rust transform 每条输入 (--no-db-filter 模式: tianhun_result=None)
    let mut actual: Vec<serde_json::Value> = Vec::new();
    for entry in &inputs {
        if let Some(sample) = transform_entry(TransformInput {
            entry,
            tianhun_result: None,
        }) {
            let json = serde_json::to_value(&sample).expect("序列化失败");
            actual.push(json);
        }
    }

    // 条数必须一致 (ok=false / response 空 的跳过行为对齐)
    assert_eq!(
        actual.len(),
        expected.len(),
        "产出条数漂移: Rust={} Python={} (ok/response 过滤行为不一致)",
        actual.len(),
        expected.len()
    );

    // 逐条比对 (字段顺序无关, 用 Value 等价)
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a, e, "第 {} 条样本与 Python 黄金基线不一致", i);
    }
}

#[test]
fn test_golden_fixtures_cover_all_four_boundaries() {
    // 确认 fixture 真的覆盖了四种边界, 避免 fixture 退化
    let inputs = parse_input_traces();
    assert!(
        inputs.len() >= 5,
        "fixture 应至少 5 条, 实际 {}",
        inputs.len()
    );

    // 必须有 ok=false 的 fixture
    assert!(inputs.iter().any(|t| !t.ok), "fixture 缺少 ok=false 用例");
    // 必须有 response 空白-only 的 fixture
    assert!(
        inputs.iter().any(|t| t.response.trim().is_empty()),
        "fixture 缺少 response 空用例"
    );
    // 必须有 persona 双空的 fixture
    assert!(
        inputs
            .iter()
            .any(|t| t.persona_name.is_empty() && t.persona_description.is_empty()),
        "fixture 缺少 persona 双空用例"
    );
    // 必须有 persona name + desc 都有的 fixture
    assert!(
        inputs
            .iter()
            .any(|t| !t.persona_name.is_empty() && !t.persona_description.is_empty()),
        "fixture 缺少 persona name+desc 用例"
    );
}
