// ============================================================================
// OpenClaw Cyber-Jianghu 初始配方配置访问器
// ============================================================================

use super::global::registry;

/// 初始配方配置访问器
pub struct InitialRecipesRegistry;

impl InitialRecipesRegistry {
    /// 获取角色的初始配方列表
    ///
    /// agent_name: 注册时的角色名，用于匹配 roles 中的 role_key
    pub fn get_initial_recipes(agent_name: Option<&str>) -> Vec<String> {
        let config = match registry() {
            Some(r) => r.get().initial_recipes.data.clone(),
            None => return Vec::new(),
        };

        let mut recipes: Vec<String> = config.default.iter().map(|r| r.recipe_id.clone()).collect();

        if let Some(name) = agent_name {
            for role in &config.roles {
                if name.contains(&role.role_key) {
                    for r in &role.recipes {
                        if !recipes.contains(&r.recipe_id) {
                            recipes.push(r.recipe_id.clone());
                        }
                    }
                }
            }
        }

        recipes
    }

    /// 获取指定角色的配方列表
    pub fn get_role_recipes(role_key: &str) -> Vec<String> {
        let config = match registry() {
            Some(r) => r.get().initial_recipes.data.clone(),
            None => return Vec::new(),
        };
        for role in &config.roles {
            if role.role_key == role_key {
                return role.recipes.iter().map(|r| r.recipe_id.clone()).collect();
            }
        }
        Vec::new()
    }

    /// 获取所有定义的角色列表
    pub fn get_roles() -> Vec<String> {
        registry()
            .map(|r| {
                r.get()
                    .initial_recipes
                    .data
                    .roles
                    .iter()
                    .map(|r| r.role_key.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}
