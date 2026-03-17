// ============================================================================
// 计算上下文 - 属性值提供者
// ============================================================================

use crate::game_data::types;

/// 主属性值提供者（trait）- 数据驱动版本，支持任意属性名
pub trait PrimaryAttributeProvider {
    /// 获取属性值（数据驱动，支持任意属性名）
    fn get_attribute(&self, name: &str) -> Option<u8>;
}

/// 为新的 AttributeComponent 实现 PrimaryAttributeProvider（数据驱动）
impl PrimaryAttributeProvider for types::AttributeComponent {
    fn get_attribute(&self, name: &str) -> Option<u8> {
        self.get_value(name).map(|v| v as u8)
    }
}
