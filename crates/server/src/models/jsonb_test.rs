// ============================================================================
// JSONB 动态属性序列化测试
// ============================================================================
//
// 验证完全动态的属性系统在数据库层正确工作
// 使用 serde_json::Value 作为统一的值类型（数据驱动）

#[cfg(test)]
mod tests {
    
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn test_attribute_value_serialization() {
        // 测试 serde_json::Value 序列化到 JSON
        let hp = json!(100);
        assert_eq!(hp, 100);

        let stamina = json!(50);
        assert_eq!(stamina, 50);

        let name = json!("test");
        assert_eq!(name, "test");
    }

    #[test]
    fn test_attributes_to_jsonb() {
        // 测试完整的 attributes HashMap 序列化
        let mut attributes: HashMap<String, serde_json::Value> = HashMap::new();
        attributes.insert("hp".to_string(), json!(100));
        attributes.insert("stamina".to_string(), json!(100));
        attributes.insert("hunger".to_string(), json!(50));
        attributes.insert("thirst".to_string(), json!(50));
        attributes.insert("sanity".to_string(), json!(100));
        attributes.insert("reputation".to_string(), json!(0));
        attributes.insert("qi".to_string(), json!(0));

        let json_val = serde_json::to_value(&attributes).unwrap();

        // 验证 JSON 结构
        let obj = json_val.as_object().unwrap();
        assert_eq!(obj.get("hp").unwrap(), &100);
        assert_eq!(obj.get("stamina").unwrap(), &100);
        assert_eq!(obj.get("hunger").unwrap(), &50);
        assert_eq!(obj.get("thirst").unwrap(), &50);
        assert_eq!(obj.get("sanity").unwrap(), &100);
        assert_eq!(obj.get("reputation").unwrap(), &0);
        assert_eq!(obj.get("qi").unwrap(), &0);
    }

    #[test]
    fn test_attributes_from_jsonb() {
        // 测试从 JSON 反序列化 attributes
        let json_val = json!({
            "hp": 100,
            "stamina": 100,
            "hunger": 50,
            "thirst": 50,
            "sanity": 100,
            "reputation": 0,
            "qi": 0
        });

        // 直接使用 serde_json::Value
        let attributes: HashMap<String, serde_json::Value> =
            serde_json::from_value(json_val).unwrap();

        assert_eq!(attributes.get("hp").unwrap().as_i64().unwrap(), 100);
        assert_eq!(attributes.get("stamina").unwrap().as_i64().unwrap(), 100);
        assert_eq!(attributes.get("hunger").unwrap().as_i64().unwrap(), 50);
        assert_eq!(attributes.get("thirst").unwrap().as_i64().unwrap(), 50);
    }

    #[test]
    fn test_dynamic_attribute_extension() {
        // 测试添加新属性无需代码修改（数据驱动）
        let json_val = json!({
            "hp": 100,
            "stamina": 100,
            "hunger": 50,
            "thirst": 50,
            // 新属性，无需修改 Rust 代码
            "new_attr": 999,
            "another_attr": "test_value"
        });

        let attributes: HashMap<String, serde_json::Value> =
            serde_json::from_value(json_val).unwrap();

        // 新属性自动可用
        assert_eq!(attributes.get("new_attr").unwrap().as_i64().unwrap(), 999);
        assert_eq!(attributes.get("another_attr").unwrap(), "test_value");
    }
}
