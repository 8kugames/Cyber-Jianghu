// ============================================================================
// OpenClaw Cyber-Jianghu 配置加载器
// ============================================================================
//
// 本模块负责从配置文件加载游戏配置
// 支持 JSON 和 YAML 两种格式（优先 YAML）
// ============================================================================

use super::loaders;
use super::types::GameData;
use anyhow::{Context, Result};
use std::path::Path;

/// 配置加载器
pub struct GameDataLoader {
    config_dir: std::path::PathBuf,
}

impl GameDataLoader {
    /// 创建新的配置加载器
    ///
    /// # 参数
    /// - `config_dir`: 配置文件目录路径
    pub fn new<P: AsRef<Path>>(config_dir: P) -> Self {
        Self {
            config_dir: config_dir.as_ref().to_path_buf(),
        }
    }

    /// 加载所有配置
    ///
    /// 一次性加载所有配置文件，返回完整的 GameData
    /// 优先加载 YAML 格式，回退到 JSON 格式
    ///
    /// # Errors
    /// 如果核心配置文件加载失败或格式不正确，返回错误
    pub fn load_all(&self) -> Result<GameData> {
        let game_rules = loaders::load_game_rules(&self.config_dir)?;
        let items = loaders::load_items(&self.config_dir)?;
        let actions = loaders::load_actions(&self.config_dir)?;
        let initial_inventory = loaders::load_initial_inventory(&self.config_dir)?;
        let inventory = loaders::load_inventory(&self.config_dir)?;
        let network = loaders::load_network(&self.config_dir)?;
        let locations = loaders::load_locations(&self.config_dir)?;
        let attributes =
            loaders::load_attributes(&self.config_dir).context("加载统一属性配置失败")?;
        let recipes = loaders::load_recipes(self.find_config_path("recipes"))?;
        let time = loaders::load_time(self.find_config_path("time"))?;
        let narrative = loaders::load_narrative(&self.config_dir)?;
        let display_messages = loaders::load_display_messages(&self.config_dir)?;
        let skills = loaders::load_skills(self.config_dir.join("skills"))?;
        let initial_recipes = loaders::load_initial_recipes(&self.config_dir)?;
        let rules_json = loaders::load_rules_json(&self.config_dir)?;
        let reward = loaders::load_reward(&self.config_dir)?;

        Ok(GameData {
            game_rules,
            items,
            actions,
            initial_inventory,
            inventory,
            network,
            locations,
            attributes,
            recipes,
            time,
            narrative,
            display_messages,
            skills,
            initial_recipes,
            rules_json,
            reward,
        })
    }

    /// 查找配置文件路径（优先 YAML，回退 JSON）
    fn find_config_path(&self, name: &str) -> std::path::PathBuf {
        let yaml_path = self.config_dir.join(format!("{}.yaml", name));
        if yaml_path.exists() {
            return yaml_path;
        }
        self.config_dir.join(format!("{}.json", name))
    }
}

/// 便捷函数：从目录加载所有配置
///
/// # 参数
/// - `config_dir`: 配置文件目录路径
///
/// # 返回
/// 完整的游戏数据
pub fn load_from_dir<P: AsRef<Path>>(config_dir: P) -> Result<GameData> {
    let loader = GameDataLoader::new(config_dir);
    loader.load_all()
}
