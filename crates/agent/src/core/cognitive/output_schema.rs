// ============================================================================
// 结构化输出 Schema
// ============================================================================
//
// 为 LLM 响应定义结构化输出格式，确保输出可解析、可验证
//
// 核心设计:
// - JSON Schema 格式定义每个阶段的输出结构
// - 支持动态人设感知（DynamicPersona 集成）
// - 提供验证规则确保输出符合预期
// ============================================================================

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::ai::persona::DynamicPersona;

// ============================================================================
// Schema 定义
// ============================================================================

/// JSON Schema 结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchema {
    /// Schema 标题
    pub title: String,
    /// Schema 描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema 版本
    #[serde(rename = "$schema")]
    pub schema_version: String,
    /// 类型
    #[serde(rename = "type")]
    pub schema_type: String,
    /// 属性定义
    #[serde(default)]
    pub properties: HashMap<String, SchemaProperty>,
    /// 必填字段
    #[serde(default)]
    pub required: Vec<String>,
    /// 额外属性是否允许
    #[serde(rename = "additionalProperties")]
    #[serde(default = "default_false")]
    pub additional_properties: bool,
}

/// 默认值为 false 的函数
fn default_false() -> bool {
    false
}

impl JsonSchema {
    /// 创建新的 Schema
    pub fn new(title: String, description: Option<String>) -> Self {
        Self {
            title,
            description,
            schema_version: "http://json-schema.org/draft-07/schema#".to_string(),
            schema_type: "object".to_string(),
            properties: HashMap::new(),
            required: Vec::new(),
            additional_properties: false,
        }
    }

    /// 添加属性
    pub fn add_property(&mut self, name: String, property: SchemaProperty, required: bool) {
        if required {
            self.required.push(name.clone());
        }
        self.properties.insert(name, property);
    }

    /// 转换为 JSON Value
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(json!({}))
    }

    /// 获取 JSON 字符串
    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// Schema 属性类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaPropertyType {
    String,
    Number,
    Integer,
    Boolean,
    Array,
    Object,
}

/// Schema 属性
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaProperty {
    /// 类型
    #[serde(rename = "type")]
    pub property_type: SchemaPropertyType,
    /// 描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 枚举值（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    /// 数组项类型（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<SchemaProperty>>,
    /// 对象属性（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, SchemaProperty>>,
    /// 最小值（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    /// 最大值（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
}

impl SchemaProperty {
    /// 创建字符串属性
    pub fn string(description: Option<String>) -> Self {
        Self {
            property_type: SchemaPropertyType::String,
            description,
            enum_values: None,
            items: None,
            properties: None,
            minimum: None,
            maximum: None,
        }
    }

    /// 创建带枚举的字符串属性
    pub fn string_enum(description: Option<String>, enum_values: Vec<String>) -> Self {
        Self {
            property_type: SchemaPropertyType::String,
            description,
            enum_values: Some(enum_values),
            items: None,
            properties: None,
            minimum: None,
            maximum: None,
        }
    }

    /// 创建整数属性
    pub fn integer(description: Option<String>, min: Option<i64>, max: Option<i64>) -> Self {
        Self {
            property_type: SchemaPropertyType::Integer,
            description,
            enum_values: None,
            items: None,
            properties: None,
            minimum: min.map(|v| v as f64),
            maximum: max.map(|v| v as f64),
        }
    }

    /// 创建数字属性
    pub fn number(description: Option<String>, min: Option<f64>, max: Option<f64>) -> Self {
        Self {
            property_type: SchemaPropertyType::Number,
            description,
            enum_values: None,
            items: None,
            properties: None,
            minimum: min,
            maximum: max,
        }
    }

    /// 创建布尔属性
    pub fn boolean(description: Option<String>) -> Self {
        Self {
            property_type: SchemaPropertyType::Boolean,
            description,
            enum_values: None,
            items: None,
            properties: None,
            minimum: None,
            maximum: None,
        }
    }

    /// 创建数组属性
    pub fn array(description: Option<String>, items: SchemaProperty) -> Self {
        Self {
            property_type: SchemaPropertyType::Array,
            description,
            enum_values: None,
            items: Some(Box::new(items)),
            properties: None,
            minimum: None,
            maximum: None,
        }
    }
}

// ============================================================================
// 认知阶段 Schema 定义
// ============================================================================

/// 感知阶段 Schema
///
/// 输出：自身状态、环境观察、关键信息
pub fn perception_schema(_persona: &DynamicPersona) -> JsonSchema {
    let mut schema = JsonSchema::new(
        "感知阶段输出".to_string(),
        Some("理解当前世界状态，包括自身、环境和关键信息".to_string()),
    );

    schema.add_property(
        "self_status".to_string(),
        SchemaProperty::string(Some("自身状态摘要（身体、饥饿、口渴等）".to_string())),
        true,
    );

    schema.add_property(
        "environment".to_string(),
        SchemaProperty::string(Some("环境观察（周围有什么、天气、地点等）".to_string())),
        true,
    );

    schema.add_property(
        "key_observations".to_string(),
        SchemaProperty::array(
            Some("识别到的关键信息（如：有人靠近、发现物品等）".to_string()),
            SchemaProperty::string(Some("单个关键观察".to_string())),
        ),
        true,
    );

    schema
}

/// 动机阶段 Schema
///
/// 输出：主要驱动力、驱动强度、推理过程
pub fn motivation_schema(persona: &DynamicPersona) -> JsonSchema {
    let mut schema = JsonSchema::new(
        "动机阶段输出".to_string(),
        Some(format!(
            "基于人设「{}」生成内在驱动力，解释为什么想要做某事",
            persona.name
        )),
    );

    schema.add_property(
        "primary_drive".to_string(),
        SchemaProperty::string(Some("当前主要驱动力（如：饥饿、复仇、求知等）".to_string())),
        true,
    );

    schema.add_property(
        "drive_intensity".to_string(),
        SchemaProperty::integer(
            Some("驱动强度（1-10，10 为最强烈）".to_string()),
            Some(1),
            Some(10),
        ),
        true,
    );

    schema.add_property(
        "reasoning".to_string(),
        SchemaProperty::string(Some("为什么有这个动机（基于人设特质的推理）".to_string())),
        true,
    );

    schema
}

/// 规划阶段 Schema
///
/// 输出：计划步骤、优先级、预期结果
pub fn planning_schema(_persona: &DynamicPersona) -> JsonSchema {
    let mut schema = JsonSchema::new(
        "规划阶段输出".to_string(),
        Some("制定行动计划，包括具体步骤和优先级".to_string()),
    );

    schema.add_property(
        "steps".to_string(),
        SchemaProperty::array(
            Some("具体行动步骤".to_string()),
            SchemaProperty::string(Some("单个步骤描述".to_string())),
        ),
        true,
    );

    schema.add_property(
        "priority".to_string(),
        SchemaProperty::integer(
            Some("优先级（1-10，10 为最高）".to_string()),
            Some(1),
            Some(10),
        ),
        true,
    );

    schema.add_property(
        "expected_outcome".to_string(),
        SchemaProperty::string(Some("预期结果（如：获得食物、击败敌人等）".to_string())),
        true,
    );

    schema
}

/// 决策阶段 Schema
///
/// 输出：思考过程、动作、目标、额外数据
pub fn decision_schema(_persona: &DynamicPersona, available_actions: &[String]) -> JsonSchema {
    let mut schema = JsonSchema::new(
        "决策阶段输出".to_string(),
        Some("选择最终行动，基于前面的感知、动机、规划阶段".to_string()),
    );

    schema.add_property(
        "thought_process".to_string(),
        SchemaProperty::string(Some("思考过程（必须引用前面阶段的结论）".to_string())),
        true,
    );

    schema.add_property(
        "action".to_string(),
        SchemaProperty::string_enum(
            Some(format!(
                "选择的动作（可用动作: {}）",
                available_actions.join(", ")
            )),
            available_actions.to_vec(),
        ),
        true,
    );

    schema.add_property(
        "target".to_string(),
        SchemaProperty::string(Some(
            "目标（可选，如：其他 Agent 名称、物品 ID）".to_string(),
        )),
        false,
    );

    schema.add_property(
        "data".to_string(),
        SchemaProperty::string(Some("额外数据（可选，如：交易价格、对话内容）".to_string())),
        false,
    );

    schema.add_property(
        "confidence".to_string(),
        SchemaProperty::number(
            Some("决策置信度（0.0-1.0）".to_string()),
            Some(0.0),
            Some(1.0),
        ),
        false,
    );

    schema
}

// ============================================================================
// Schema 验证
// ============================================================================

/// Schema 验证错误
#[derive(Debug, Clone)]
pub enum SchemaValidationError {
    /// 缺少必填字段
    MissingRequiredField(String),
    /// 类型错误
    TypeMismatch(String),
    /// 值超出范围
    ValueOutOfRange(String),
    /// 枚举值不匹配
    InvalidEnumValue(String),
    /// JSON 解析错误
    JsonParseError(String),
}

impl std::fmt::Display for SchemaValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingRequiredField(field) => write!(f, "缺少必填字段: {}", field),
            Self::TypeMismatch(field) => write!(f, "类型错误: {}", field),
            Self::ValueOutOfRange(field) => write!(f, "值超出范围: {}", field),
            Self::InvalidEnumValue(field) => write!(f, "无效的枚举值: {}", field),
            Self::JsonParseError(msg) => write!(f, "JSON 解析错误: {}", msg),
        }
    }
}

impl std::error::Error for SchemaValidationError {}

/// Schema 验证器
pub struct SchemaValidator;

impl SchemaValidator {
    /// 验证 JSON 值是否符合 Schema
    pub fn validate(json: &Value, schema: &JsonSchema) -> Result<(), SchemaValidationError> {
        // 检查必填字段
        for required_field in &schema.required {
            if json.get(required_field).is_none() {
                return Err(SchemaValidationError::MissingRequiredField(
                    required_field.clone(),
                ));
            }
        }

        // 检查每个属性
        if let Some(obj) = json.as_object() {
            for (key, value) in obj {
                if let Some(property) = schema.properties.get(key) {
                    Self::validate_property(value, property)?;
                }
            }
        }

        Ok(())
    }

    /// 验证单个属性
    fn validate_property(
        value: &Value,
        property: &SchemaProperty,
    ) -> Result<(), SchemaValidationError> {
        match property.property_type {
            SchemaPropertyType::String => {
                if !value.is_string() {
                    return Err(SchemaValidationError::TypeMismatch(format!(
                        "期望 string, 实际 {:?}",
                        value
                    )));
                }

                // 检查枚举值
                if let Some(ref enum_values) = property.enum_values {
                    let str_val = value.as_str().unwrap();
                    if !enum_values.contains(&str_val.to_string()) {
                        return Err(SchemaValidationError::InvalidEnumValue(format!(
                            "值 '{}' 不在允许的枚举值中: {:?}",
                            str_val, enum_values
                        )));
                    }
                }
            }
            SchemaPropertyType::Integer | SchemaPropertyType::Number => {
                let is_valid = match property.property_type {
                    SchemaPropertyType::Integer => value.is_i64(),
                    SchemaPropertyType::Number => value.is_f64() || value.is_i64(),
                    _ => false,
                };

                if !is_valid {
                    return Err(SchemaValidationError::TypeMismatch(format!(
                        "期望 {:?}, 实际 {:?}",
                        property.property_type, value
                    )));
                }

                // 检查范围
                let num_val = value.as_f64().or_else(|| value.as_i64().map(|v| v as f64));
                if let Some(val) = num_val {
                    if let Some(min) = property.minimum
                        && val < min
                    {
                        return Err(SchemaValidationError::ValueOutOfRange(format!(
                            "值 {} 小于最小值 {}",
                            val, min
                        )));
                    }
                    if let Some(max) = property.maximum
                        && val > max
                    {
                        return Err(SchemaValidationError::ValueOutOfRange(format!(
                            "值 {} 大于最大值 {}",
                            val, max
                        )));
                    }
                }
            }
            SchemaPropertyType::Boolean => {
                if !value.is_boolean() {
                    return Err(SchemaValidationError::TypeMismatch(format!(
                        "期望 boolean, 实际 {:?}",
                        value
                    )));
                }
            }
            SchemaPropertyType::Array => {
                if !value.is_array() {
                    return Err(SchemaValidationError::TypeMismatch(format!(
                        "期望 array, 实际 {:?}",
                        value
                    )));
                }

                // 验证数组项
                if let Some(ref items) = property.items
                    && let Some(arr) = value.as_array()
                {
                    for item in arr {
                        Self::validate_property(item, items)?;
                    }
                }
            }
            SchemaPropertyType::Object => {
                if !value.is_object() {
                    return Err(SchemaValidationError::TypeMismatch(format!(
                        "期望 object, 实际 {:?}",
                        value
                    )));
                }
            }
        }

        Ok(())
    }

    /// 从 JSON 字符串解析并验证
    pub fn parse_and_validate(
        json_str: &str,
        schema: &JsonSchema,
    ) -> Result<Value, SchemaValidationError> {
        let json: Value = serde_json::from_str(json_str)
            .map_err(|e| SchemaValidationError::JsonParseError(e.to_string()))?;

        Self::validate(&json, schema)?;
        Ok(json)
    }
}

// ============================================================================
// Schema 生成工具
// ============================================================================

/// Schema 生成器
///
/// 根据当前上下文生成合适的 Schema
pub struct SchemaGenerator {
    /// 可用动作列表
    available_actions: Vec<String>,
}

impl SchemaGenerator {
    /// 创建新的 Schema 生成器
    pub fn new(available_actions: Vec<String>) -> Self {
        Self { available_actions }
    }

    /// 生成感知阶段 Schema
    pub fn perception(&self, persona: &DynamicPersona) -> JsonSchema {
        perception_schema(persona)
    }

    /// 生成动机阶段 Schema
    pub fn motivation(&self, persona: &DynamicPersona) -> JsonSchema {
        motivation_schema(persona)
    }

    /// 生成规划阶段 Schema
    pub fn planning(&self, persona: &DynamicPersona) -> JsonSchema {
        planning_schema(persona)
    }

    /// 生成决策阶段 Schema
    pub fn decision(&self, persona: &DynamicPersona) -> JsonSchema {
        decision_schema(persona, &self.available_actions)
    }

    /// 生成所有阶段的 Schema
    pub fn all_stages(&self, persona: &DynamicPersona) -> HashMap<String, JsonSchema> {
        let mut schemas = HashMap::new();
        schemas.insert("perception".to_string(), self.perception(persona));
        schemas.insert("motivation".to_string(), self.motivation(persona));
        schemas.insert("planning".to_string(), self.planning(persona));
        schemas.insert("decision".to_string(), self.decision(persona));
        schemas
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::persona::DynamicPersona;
    use uuid::Uuid;

    #[test]
    fn test_schema_creation() {
        let mut schema =
            JsonSchema::new("测试 Schema".to_string(), Some("这是一个测试".to_string()));

        schema.add_property(
            "name".to_string(),
            SchemaProperty::string(Some("名称".to_string())),
            true,
        );

        schema.add_property(
            "age".to_string(),
            SchemaProperty::integer(Some("年龄".to_string()), Some(0), Some(150)),
            true,
        );

        let json = schema.to_json();
        assert!(json.is_object());
        assert!(json["title"] == "测试 Schema");
    }

    #[test]
    fn test_perception_schema() {
        let agent_id = Uuid::new_v4();
        let persona = DynamicPersona::new(agent_id, "测试角色", "基础描述");

        let schema = perception_schema(&persona);
        assert_eq!(schema.title, "感知阶段输出");
        assert!(schema.properties.contains_key("self_status"));
        assert!(schema.properties.contains_key("environment"));
        assert!(schema.properties.contains_key("key_observations"));
    }

    #[test]
    fn test_schema_validator() {
        let mut schema = JsonSchema::new("测试".to_string(), Some("测试验证".to_string()));

        schema.add_property(
            "name".to_string(),
            SchemaProperty::string(Some("名称".to_string())),
            true,
        );

        schema.add_property(
            "age".to_string(),
            SchemaProperty::integer(Some("年龄".to_string()), Some(0), Some(150)),
            true,
        );

        // 有效数据
        let valid_json = json!({
            "name": "测试",
            "age": 25
        });

        assert!(SchemaValidator::validate(&valid_json, &schema).is_ok());

        // 缺少必填字段
        let missing_field_json = json!({
            "name": "测试"
        });

        assert!(SchemaValidator::validate(&missing_field_json, &schema).is_err());

        // 类型错误
        let type_error_json = json!({
            "name": "测试",
            "age": "not_a_number"
        });

        assert!(SchemaValidator::validate(&type_error_json, &schema).is_err());
    }

    #[test]
    fn test_schema_generator() {
        let agent_id = Uuid::new_v4();
        let persona = DynamicPersona::new(agent_id, "测试角色", "基础描述");

        let available_actions = vec![
            "attack".to_string(),
            "trade".to_string(),
            "idle".to_string(),
        ];

        let generator = SchemaGenerator::new(available_actions);
        let schemas = generator.all_stages(&persona);

        assert_eq!(schemas.len(), 4);
        assert!(schemas.contains_key("perception"));
        assert!(schemas.contains_key("motivation"));
        assert!(schemas.contains_key("planning"));
        assert!(schemas.contains_key("decision"));
    }

    #[test]
    fn test_enum_validation() {
        let mut schema = JsonSchema::new("枚举测试".to_string(), Some("测试枚举验证".to_string()));

        schema.add_property(
            "action".to_string(),
            SchemaProperty::string_enum(
                Some("动作类型".to_string()),
                vec!["attack".to_string(), "defend".to_string()],
            ),
            true,
        );

        // 有效枚举值
        let valid_json = json!({
            "action": "attack"
        });

        assert!(SchemaValidator::validate(&valid_json, &schema).is_ok());

        // 无效枚举值
        let invalid_json = json!({
            "action": "fly"
        });

        assert!(SchemaValidator::validate(&invalid_json, &schema).is_err());
    }
}
