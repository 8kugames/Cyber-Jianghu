use crate::game_data::registry_or_error;
use crate::game_data::types::RecipeDefinition;

/// 配方注册表
///
/// 提供对配方配置的安全访问
pub struct RecipeRegistry;

impl RecipeRegistry {
    /// 获取配方定义
    pub fn get(recipe_id: &str) -> Option<RecipeDefinition> {
        let registry = registry_or_error().ok()?;
        registry.get().recipes.data.get(recipe_id).cloned()
    }

    /// 获取全部配方的 (recipe_id, 名称) 列表，按 recipe_id 排序（确定性）。
    ///
    /// 供 display-map 等需要枚举全部配方的消费方使用。
    pub fn all() -> Vec<(String, String)> {
        let registry = match registry_or_error() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let mut list: Vec<(String, String)> = registry
            .get()
            .recipes
            .data
            .iter()
            .map(|(id, r)| (id.clone(), r.name.clone()))
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    /// 获取配方展示名：名称[短 uuid 前 8 位]。
    ///
    /// 与角色/物品展示名同构：既可读（名称），又可还原（短 uuid）。
    /// 未注册配方理论上不可达（入口全量校验）；
    /// 一旦出现（配置漂移/LLM 幻觉穿透），大声标注而非静默退化。
    pub fn display_name(recipe_id: &str) -> String {
        match Self::get(recipe_id) {
            Some(recipe) => format!(
                "{}[{}]",
                recipe.name,
                &Self::uuid(recipe_id).to_string()[..8]
            ),
            None => format!("未知配方[{}]", &Self::uuid(recipe_id).to_string()[..8]),
        }
    }

    /// 从配方 uuid 反查内部 recipe_id（uuid → recipe_id，严格模式）。
    ///
    /// v5 为单向派生，反查靠枚举注册表。仅接受完整 uuid；
    /// 裸 recipe_id 一律返回 None（强制全链路 uuid 引用）。
    pub fn resolve_recipe_id(uuid_str: &str) -> Option<String> {
        let target = uuid::Uuid::parse_str(uuid_str).ok()?;
        Self::all()
            .iter()
            .find(|(id, _)| Self::uuid(id) == target)
            .map(|(id, _)| id.clone())
    }

    /// 获取配方的稳定 uuid（UUID v5，从 recipe_id 确定性派生）。
    ///
    /// 配方是数据驱动的"类型"而非实例，uuid 由专用命名空间
    /// （ASCII "cjh-recipe-uuid1"，16 字节）派生：同一 recipe_id 在任何部署、
    /// 任何重启下得到同一 uuid，零配置、零迁移、天然防碰撞。
    /// 无需查表：任何 recipe_id（含未注册的）都可派生。
    pub fn uuid(recipe_id: &str) -> uuid::Uuid {
        const RECIPE_UUID_NAMESPACE: uuid::Uuid =
            uuid::Uuid::from_u128(0x636a_682d_7265_6369_7065_2d75_7569_6431);
        uuid::Uuid::new_v5(&RECIPE_UUID_NAMESPACE, recipe_id.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recipe_uuid_deterministic() {
        let a = RecipeRegistry::uuid("recipe_mantou");
        let b = RecipeRegistry::uuid("recipe_mantou");
        assert_eq!(a, b, "同一 recipe_id 必须派生出同一 uuid");
        assert_ne!(
            a,
            RecipeRegistry::uuid("recipe_knife"),
            "不同 recipe_id 必须派生出不同 uuid"
        );
        assert_eq!(a.get_version_num(), 5, "必须是 v5 uuid");
        // 与物品命名空间隔离：同一字符串在物品/配方两个体系下 uuid 不同
        assert_ne!(a, crate::items::item_uuid("recipe_mantou"));
    }
}
