// ============================================================================
// 决策提示词渲染 — startlux_decision/jevfmt.py 的逐字移植
// ============================================================================
//
// 训练时模型的输入分布由此格式唯一决定，任何一字偏差都会把读出推离分布。
// 移植对齐口径（与 Python 参考实现逐字一致）：
//   - system 固定一行判据指令；user = Evidence / Question / Options 三段；
//   - 选项行 "{letter}) {选项名}: {判据}"，选项顺序 = 请求给出顺序，letter = A..Z；
//   - "裸字母选项隐藏判据"：choice 题所有选项 id 都是裸字母/数字且都有判据时，
//     选项行只显示判据（避免答案字母与选项名歧义）；
//   - noul 题 true/false 显示为 yes/no；
//   - 判据为空或与选项名相同（归一后）时只显示选项名；
//   - chat 模板以 enable_thinking=false 渲染：Qwen im_start/im_end 三段结构，
//     assistant 段后接 "<think>\n\n</think>\n\n"（训练时校验过的 thinking-off 前缀）。
//
// 状态文本恒为字符串（生产由认知摘要块 + 精简世界状态拼装），jevfmt 的
// annotate_indices（长数组下标标注）与 score 题型在本管线中不使用，未移植。

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// 固定 system 指令（jevfmt.SYSTEM 逐字）
pub const SYSTEM: &str = "Apply the criterion to the evidence. Choose exactly one listed option. Answer with its letter only.";

/// thinking-off assistant 前缀（jevfmt.THINK_OFF_SUFFIX 逐字）。
/// chat 模板渲染结果必须以此结尾（渲染正确性的硬校验）。
pub const THINK_OFF_SUFFIX: &str = "<think>\n\n</think>\n\n";

/// 选项字母表 A..Z（jevfmt.LETTERS）
pub const LETTERS: [char; 26] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

/// 单选项最大数（jevfmt.MAX_OPTIONS）
pub const MAX_OPTIONS: usize = 26;

/// 裸字母/数字选项 id 判定（jevfmt._BARE，case-insensitive）
const BARE_ID_RE: &str =
    r"^(?:\(?[A-Za-z][\).]?|\(?\d{1,3}[\).]?|option[_ ]?\d{1,3}|opt[_ ]?\d{1,3})$";

/// 题型（本管线使用 choice 与 noul；score 为推理包能力，未接入）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QType {
    Choice,
    Noul,
}

impl QType {
    /// decision_config.json temperature_by_type 的键名
    pub fn config_key(self) -> &'static str {
        match self {
            QType::Choice => "choice",
            QType::Noul => "noul",
        }
    }
}

/// 单个选项：id 为答案值（回填 action_data 的原样字符串），criterion 为判据说明
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionSpec {
    pub id: String,
    /// None 或空串表示无判据（只显示选项名）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<String>,
}

impl OptionSpec {
    pub fn bare(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            criterion: None,
        }
    }

    pub fn with_criterion(id: impl Into<String>, criterion: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            criterion: Some(criterion.into()),
        }
    }
}

/// 单个问题（jevfmt 的 row；state 由调用方统一携带，不进 QuestionSpec）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionSpec {
    pub qtype: QType,
    /// 问题指令（non-empty，渲染前 strip）
    pub instructions: String,
    /// 选项列表（顺序即渲染顺序 = letter 顺序）
    pub options: Vec<OptionSpec>,
}

/// 校验问题规格（jevfmt.validate 同口径）
pub fn validate(q: &QuestionSpec) -> Result<()> {
    if q.instructions.trim().is_empty() {
        bail!("instructions 不能为空");
    }
    if q.options.len() < 2 || q.options.len() > MAX_OPTIONS {
        bail!(
            "选项数需在 2..{} 之间，实际 {}",
            MAX_OPTIONS,
            q.options.len()
        );
    }
    let mut seen = std::collections::HashSet::new();
    for o in &q.options {
        let id = o.id.as_str();
        if id.trim().is_empty() || id.contains('\n') || id.contains('\r') || id.len() > 200 {
            bail!("非法选项 id: {id:?}");
        }
        if !seen.insert(id.to_string()) {
            bail!("选项 id 重复: {id:?}");
        }
    }
    if q.qtype == QType::Noul {
        let ids: std::collections::HashSet<_> = q.options.iter().map(|o| o.id.as_str()).collect();
        if ids != std::collections::HashSet::from(["true", "false"]) {
            bail!("noul 题选项 id 必须为 true/false");
        }
    }
    Ok(())
}

/// id 归一（jevfmt._norm）：空白/下划线/连字符序列压成单空格，转小写
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_sep = false;
    for c in s.chars() {
        if c.is_whitespace() || c == '_' || c == '-' {
            in_sep = true;
        } else {
            if in_sep {
                out.push(' ');
                in_sep = false;
            }
            out.push(c.to_ascii_lowercase());
        }
    }
    out.trim().to_string()
}

/// 裸字母/数字 id 判定（jevfmt._BARE）
fn is_bare_id(id: &str) -> bool {
    regex::Regex::new(BARE_ID_RE)
        .expect("内置正则必合法")
        .is_match(id)
}

/// 选项行渲染（jevfmt.option_lines 逐字）
///
/// 返回 "A) {body}" 形式的行列表；`order` 为渲染顺序（选项 id 序列），
/// 供调用方对齐 letter → option id 的映射。
pub fn option_lines(q: &QuestionSpec, order: &[String]) -> Result<Vec<String>> {
    let crit_of = |id: &str| -> Option<&str> {
        q.options
            .iter()
            .find(|o| o.id == *id)
            .and_then(|o| o.criterion.as_deref())
    };
    let hide = q.qtype == QType::Choice
        && order.iter().all(|i| is_bare_id(i))
        && order
            .iter()
            .all(|i| crit_of(i).map(|c| !c.trim().is_empty()).unwrap_or(false));

    let mut lines = Vec::with_capacity(order.len());
    for (k, id) in order.iter().enumerate() {
        let name = match q.qtype {
            QType::Noul => {
                if id == "true" {
                    "yes".to_string()
                } else {
                    "no".to_string()
                }
            }
            QType::Choice => id.clone(),
        };
        let c = crit_of(id);
        let body = if hide {
            c.context("隐藏判据模式下所有选项必须有判据")?
                .trim()
                .to_string()
        } else if c.is_none()
            || c.unwrap_or_default().trim().is_empty()
            || norm(c.unwrap_or_default()) == norm(&name)
        {
            name
        } else {
            format!("{name}: {}", c.context("criterion 存在")?.trim())
        };
        lines.push(format!("{}) {body}", LETTERS[k]));
    }
    Ok(lines)
}

/// 渲染 chat messages（jevfmt.messages 逐字）
///
/// 返回 (system 内容, user 内容)；user 内容 = Evidence / Question / Options 三段。
pub fn messages(q: &QuestionSpec, state: &str) -> Result<(String, String)> {
    validate(q)?;
    let order: Vec<String> = q.options.iter().map(|o| o.id.clone()).collect();
    let state = if state.trim().is_empty() {
        "(none)"
    } else {
        state
    };
    let content = format!(
        "Evidence:\n{}\n\nQuestion: {}\nOptions:\n{}",
        state,
        q.instructions.trim(),
        option_lines(q, &order)?.join("\n")
    );
    Ok((SYSTEM.to_string(), content))
}

/// 以 Qwen3.5 chat template（enable_thinking=false）渲染完整 prompt 文本
///
/// 模板形态已用 HF tokenizer 逐字校验（见 tests::golden_render_matches_python）：
/// <|im_start|>system\n{sys}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n
/// <|im_start|>assistant\n<think>\n\n</think>\n\n
pub fn render_chat_text(system: &str, user: &str) -> String {
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n{THINK_OFF_SUFFIX}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 黄金样本 1：choice 题（与 Python jevfmt.messages + apply_chat_template 逐字对拍）
    #[test]
    fn golden_render_matches_python() {
        let q = QuestionSpec {
            qtype: QType::Choice,
            instructions: "选择你此刻最应该执行的第一个动作。".to_string(),
            options: vec![
                OptionSpec::with_criterion("吃", "摄入食物"),
                OptionSpec::with_criterion("休整", "静待时间流逝"),
                OptionSpec::with_criterion("移动", "移动到相邻位置"),
            ],
        };
        let (system, user) = messages(&q, "你是一名侠客。饥饿难耐。").expect("渲染");
        assert_eq!(
            system,
            "Apply the criterion to the evidence. Choose exactly one listed option. Answer with its letter only."
        );
        assert_eq!(
            user,
            "Evidence:\n你是一名侠客。饥饿难耐。\n\nQuestion: 选择你此刻最应该执行的第一个动作。\nOptions:\nA) 吃: 摄入食物\nB) 休整: 静待时间流逝\nC) 移动: 移动到相邻位置"
        );
        let text = render_chat_text(&system, &user);
        assert_eq!(
            text,
            "<|im_start|>system\nApply the criterion to the evidence. Choose exactly one listed option. Answer with its letter only.<|im_end|>\n<|im_start|>user\nEvidence:\n你是一名侠客。饥饿难耐。\n\nQuestion: 选择你此刻最应该执行的第一个动作。\nOptions:\nA) 吃: 摄入食物\nB) 休整: 静待时间流逝\nC) 移动: 移动到相邻位置<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        assert!(
            text.ends_with(THINK_OFF_SUFFIX),
            "渲染文本必须以 thinking-off 前缀结尾"
        );
    }

    /// 黄金样本 2：裸字母选项隐藏判据规则
    #[test]
    fn golden_bare_letter_hide() {
        let q = QuestionSpec {
            qtype: QType::Choice,
            instructions: "q".to_string(),
            options: vec![
                OptionSpec::with_criterion("A", "Paris is in France"),
                OptionSpec::with_criterion("B", "Paris is in Italy"),
            ],
        };
        let (_, user) = messages(&q, "s").expect("渲染");
        assert_eq!(
            user,
            "Evidence:\ns\n\nQuestion: q\nOptions:\nA) Paris is in France\nB) Paris is in Italy"
        );
    }

    /// 黄金样本 3：noul 题 true/false → yes/no
    #[test]
    fn golden_noul_yes_no() {
        let q = QuestionSpec {
            qtype: QType::Noul,
            instructions: "Is the sky blue?".to_string(),
            options: vec![OptionSpec::bare("true"), OptionSpec::bare("false")],
        };
        let (_, user) = messages(&q, "s").expect("渲染");
        assert_eq!(
            user,
            "Evidence:\ns\n\nQuestion: Is the sky blue?\nOptions:\nA) yes\nB) no"
        );
    }

    /// 黄金样本 4：无判据 / 判据与选项名相同 → 只显示选项名
    #[test]
    fn golden_criterion_display_rules() {
        let q = QuestionSpec {
            qtype: QType::Choice,
            instructions: "q".to_string(),
            options: vec![
                OptionSpec::bare("吃"),
                OptionSpec::with_criterion("喝", "喝"),
                OptionSpec::with_criterion("用", "消耗物品"),
            ],
        };
        let (_, user) = messages(&q, "s").expect("渲染");
        assert_eq!(
            user,
            "Evidence:\ns\n\nQuestion: q\nOptions:\nA) 吃\nB) 喝\nC) 用: 消耗物品"
        );
    }

    #[test]
    fn empty_state_becomes_none_placeholder() {
        let q = QuestionSpec {
            qtype: QType::Choice,
            instructions: "q".to_string(),
            options: vec![OptionSpec::bare("x"), OptionSpec::bare("y")],
        };
        let (_, user) = messages(&q, "  ").expect("渲染");
        assert!(user.starts_with("Evidence:\n(none)\n"));
    }

    #[test]
    fn validate_rejects_bad_specs() {
        let mut q = QuestionSpec {
            qtype: QType::Choice,
            instructions: "q".into(),
            options: vec![OptionSpec::bare("x"), OptionSpec::bare("x")],
        };
        assert!(validate(&q).is_err(), "重复 id 拒绝");
        q.options = vec![OptionSpec::bare("x")];
        assert!(validate(&q).is_err(), "单选项拒绝");
        q.instructions = "  ".into();
        q.options = vec![OptionSpec::bare("x"), OptionSpec::bare("y")];
        assert!(validate(&q).is_err(), "空 instructions 拒绝");
        let mut noul = QuestionSpec {
            qtype: QType::Noul,
            instructions: "q".into(),
            options: vec![OptionSpec::bare("true"), OptionSpec::bare("yes")],
        };
        assert!(validate(&noul).is_err(), "noul id 必须为 true/false");
        noul.options = vec![OptionSpec::bare("false"), OptionSpec::bare("true")];
        assert!(validate(&noul).is_ok());
    }

    #[test]
    fn bare_id_matcher_aligns_python() {
        // 与 jevfmt._BARE 对齐：裸字母/数字（含可选括号与标点）→ 裸
        for id in [
            "A", "a", "C", "(A)", "A.", "A)", "12", "(12)", "12.", "option_3", "opt 4",
        ] {
            assert!(is_bare_id(id), "{id:?} 应判为裸 id");
        }
        for id in [
            "吃",
            "馒头[a65df604]",
            "无",
            "gate_street",
            "ABC",
            "option_3a",
            "",
        ] {
            assert!(!is_bare_id(id), "{id:?} 不应判为裸 id");
        }
    }

    #[test]
    fn norm_aligns_python() {
        assert_eq!(norm("Gate-Street_1"), "gate street 1");
        assert_eq!(norm("吃"), "吃");
        assert_eq!(norm("  A  "), "a");
    }
}
