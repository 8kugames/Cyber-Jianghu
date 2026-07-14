// 天魂层展示名映射 handler —— 数据驱动，从 souls.yaml layer_display 读取

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use std::collections::HashMap;

use super::HttpApiState;

/// 天魂层展示名映射（数据驱动，从 souls.yaml layer_display 读取）
///
/// GET /api/dashboard/layer-display
pub(crate) async fn get_layer_display(
    State(_state): State<HttpApiState>,
) -> Result<Json<HashMap<String, String>>, StatusCode> {
    let yaml_path = crate::config::config_dir().join("souls.yaml");
    let content = std::fs::read_to_string(&yaml_path).map_err(|e| {
        tracing::error!("读取 souls.yaml 失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let parsed: serde_json::Value = serde_yaml::from_str(&content).map_err(|e| {
        tracing::error!("解析 souls.yaml 失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let map = parsed
        .get("data")
        .and_then(|d| d.get("tianhun"))
        .and_then(|t| t.get("layer_display"))
        .map(|v| {
            serde_json::from_value::<HashMap<String, String>>(v.clone()).unwrap_or_else(|e| {
                tracing::warn!("souls.yaml layer_display 字段解析失败: {}", e);
                HashMap::new()
            })
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| {
            // 向后兼容：若无配置，返回默认映射
            let mut m = HashMap::new();
            m.insert("layer1".to_string(), "动作审查".to_string());
            m.insert("layer2".to_string(), "规则校验".to_string());
            m.insert("layer3".to_string(), "意图审查".to_string());
            m
        });

    Ok(Json(map))
}
