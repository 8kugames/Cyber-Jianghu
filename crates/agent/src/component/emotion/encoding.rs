use crate::component::emotion::config::EncodingConfig;

/// 情绪编码门控：arousal 调制 memory importance
/// 支持函数类型：linear | exponential
pub fn apply_emotional_encoding(base_score: f32, arousal: f32, cfg: &EncodingConfig) -> f32 {
    let boost = match cfg.function.as_str() {
        "linear" => cfg.intercept + arousal * cfg.slope,
        "exponential" => (arousal * cfg.exponent).exp(),
        _ => match cfg.unknown_function_fallback.as_str() {
            "panic" => panic!("Unknown encoding function: {}", cfg.function),
            _ => {
                tracing::warn!(
                    "Unknown encoding function '{}', falling back to intercept",
                    cfg.function
                );
                cfg.intercept
            }
        },
    };
    let mut score = base_score * boost;
    if arousal >= cfg.flashbulb.threshold {
        score *= cfg.flashbulb.multiplier;
    }
    score.clamp(cfg.output_range[0], cfg.output_range[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::emotion::config::EncodingConfig;

    fn default_config() -> EncodingConfig {
        EncodingConfig::default()
    }

    #[test]
    fn test_linear_boost_zero_arousal() {
        let score = apply_emotional_encoding(0.5, 0.0, &default_config());
        assert!(
            (score - 0.5).abs() < 0.001,
            "arousal=0 应无增强，got {}",
            score
        );
    }

    #[test]
    fn test_linear_boost_high_arousal() {
        let score = apply_emotional_encoding(0.5, 0.6, &default_config());
        // boost = 1.0 + 0.6 * 0.5 = 1.3; score = 0.5 * 1.3 = 0.65; no flashbulb (< 0.8)
        assert!((score - 0.65).abs() < 0.001, "got {}", score);
    }

    #[test]
    fn test_flashbulb() {
        let score = apply_emotional_encoding(0.5, 0.9, &default_config());
        // boost = 1.0 + 0.9 * 0.5 = 1.45; * 0.5 = 0.725; * 1.5 flashbulb = 1.0875; clamp 1.0
        assert!(
            (score - 1.0).abs() < 0.001,
            "flashbulb 应 clamp 到 1.0，got {}",
            score
        );
    }

    #[test]
    fn test_unknown_function_warn() {
        let mut cfg = default_config();
        cfg.function = "nonexistent".to_string();
        let score = apply_emotional_encoding(0.5, 0.5, &cfg);
        assert!(
            (score - 0.5).abs() < 0.001,
            "unknown function fallback，got {}",
            score
        );
    }

    #[test]
    #[should_panic(expected = "Unknown encoding function")]
    fn test_unknown_function_panic() {
        let mut cfg = default_config();
        cfg.function = "nonexistent".to_string();
        cfg.unknown_function_fallback = "panic".to_string();
        apply_emotional_encoding(0.5, 0.5, &cfg);
    }
}
