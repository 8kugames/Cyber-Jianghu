use crate::component::emotion::config::RetrievalConfig;

/// 效价一致性检索偏置
/// 受 Bower (1981) 启发的工程近似
pub fn compute_valence_bonus(
    encoding_valence: Option<f32>,
    current_valence: f32,
    cfg: &RetrievalConfig,
) -> f32 {
    match encoding_valence {
        Some(ev) => {
            let congruence = 1.0 - (ev - current_valence).abs() / cfg.valence_range;
            cfg.valence_bias_weight * congruence.max(0.0)
        }
        None => cfg.null_encoding_bonus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::emotion::config::RetrievalConfig;

    #[test]
    fn test_congruent_valence() {
        let cfg = RetrievalConfig::default();
        let bonus = compute_valence_bonus(Some(-0.5), -0.5, &cfg);
        // congruence = 1 - 0/2 = 1.0; bonus = 0.3 * 1.0 = 0.3
        assert!((bonus - 0.3).abs() < 0.001, "got {}", bonus);
    }

    #[test]
    fn test_opposite_valence() {
        let cfg = RetrievalConfig::default();
        let bonus = compute_valence_bonus(Some(0.9), -0.9, &cfg);
        // congruence = 1 - 1.8/2.0 = 0.1; bonus = 0.3 * 0.1 = 0.03
        assert!((bonus - 0.03).abs() < 0.001, "got {}", bonus);
    }

    #[test]
    fn test_null_encoding() {
        let cfg = RetrievalConfig::default();
        let bonus = compute_valence_bonus(None, 0.5, &cfg);
        assert!((bonus - 0.0).abs() < 0.001, "NULL encoding 应返回 0，got {}", bonus);
    }
}
