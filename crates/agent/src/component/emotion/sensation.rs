use crate::component::emotion::config::SensationConfig;
use std::collections::HashMap;

/// 渲染体感 prompt 文本
pub fn build_internal_sensation(
    valence: f32,
    arousal: f32,
    attributes: &HashMap<String, i32>,
    config: &SensationConfig,
) -> String {
    let valence_label = find_label(valence, &config.valence_labels);
    let arousal_label = find_label(arousal, &config.arousal_labels);
    let distress_hint = build_distress_hint(
        attributes,
        &config.distress_template,
        config.distress_threshold,
    );
    let hint_line = if distress_hint.is_empty() {
        String::new()
    } else {
        format!("- {}", distress_hint)
    };
    config
        .template
        .replace("{valence_label}", &valence_label)
        .replace("{valence:.2}", &format!("{:.2}", valence))
        .replace("{arousal_label}", &arousal_label)
        .replace("{arousal:.2}", &format!("{:.2}", arousal))
        .replace("{distress_hint}", &hint_line)
}

fn find_label(value: f32, labels: &[crate::component::emotion::config::SensationLabel]) -> String {
    for (i, label) in labels.iter().enumerate() {
        let is_last = i == labels.len() - 1;
        if value >= label.lo && (value < label.hi || is_last) {
            return label.label.clone();
        }
    }
    crate::component::emotion::config::default_fallback_label()
}

fn build_distress_hint(
    attributes: &HashMap<String, i32>,
    template: &str,
    distress_threshold: i32,
) -> String {
    let worst = attributes
        .iter()
        .filter(|&(_, &v)| v < distress_threshold)
        .min_by_key(|&(_, &v)| v);
    match worst {
        Some((name, _)) => template.replace("{attribute_name}", name),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::emotion::config::SensationConfig;

    #[test]
    fn test_build_sensation_positive() {
        let config = SensationConfig::default();
        let attrs = HashMap::from([("hunger".into(), 60), ("hp".into(), 80)]);
        let result = build_internal_sensation(0.5, 0.3, &attrs, &config);
        assert!(result.contains("明显的愉悦"), "got: {}", result);
        assert!(result.contains("平静清醒"), "got: {}", result);
    }

    #[test]
    fn test_build_sensation_with_distress() {
        let config = SensationConfig::default();
        let attrs = HashMap::from([("hunger".into(), 10), ("hp".into(), 80)]);
        let result = build_internal_sensation(-0.5, 0.7, &attrs, &config);
        assert!(
            result.contains("hunger"),
            "应包含 distress hint，got: {}",
            result
        );
    }
}
