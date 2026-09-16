//! 配置热重载（actions/game_rules/world_building_rules，mtime 监视）

use super::*;

impl TickScheduler {
    /// 检查 actions.yaml 是否变更，若变更则重新加载并广播
    pub(super) async fn check_and_reload_actions(&mut self) -> Result<()> {
        let config_dir = get_config_dir();
        let actions_path = config_dir.join("actions.yaml");
        let json_path = config_dir.join("actions.json");

        // 确定实际使用的文件
        let file_path = if actions_path.exists() {
            &actions_path
        } else if json_path.exists() {
            &json_path
        } else {
            return Ok(()); // 文件不存在，跳过
        };

        let modified = match read_file_metadata_for_hot_reload(file_path)? {
            Some(t) => t,
            None => return Ok(()),
        };

        // 检查是否是新文件或已修改
        let should_reload = match self.last_actions_mtime {
            Some(last) => modified > last,
            None => true,
        };

        if should_reload {
            self.last_actions_mtime = Some(modified);

            // 重新加载 actions
            match load_actions(&config_dir) {
                Ok(new_actions) => {
                    let version = new_actions.version.clone();
                    let actions_count = new_actions.data.len();

                    // 更新缓存
                    self.game_data_cache.update_actions(new_actions);

                    // 重新初始化注册表
                    crate::game_data::init_registry(self.game_data_cache.clone());

                    info!(
                        "动作配置已热重载: version={}, actions={}",
                        version, actions_count
                    );

                    // 构建 AvailableAction 列表
                    let available_actions =
                        crate::game_data::ActionRegistry::build_available_actions();

                    // 广播给所有在线 Agent
                    let config_update = ServerMessage::config_update_full_value(
                        ConfigType::Actions,
                        version,
                        serde_json::to_value(available_actions)?,
                        None,
                    );

                    if let Err(e) =
                        broadcast_config_update(config_update, &self.connection_manager).await
                    {
                        warn!("广播动作更新失败: {}", e);
                    }
                }
                Err(e) => {
                    warn!("重新加载 actions.yaml 失败: {}", e);
                }
            }
        }

        Ok(())
    }

    /// 检查 game_rules.yaml 是否变更，若变更则重新加载并广播
    pub(super) async fn check_and_reload_game_rules(&mut self) -> Result<()> {
        let config_dir = get_config_dir();
        let game_rules_path = config_dir.join("game_rules.yaml");
        let json_path = config_dir.join("game_rules.json");

        // 确定实际使用的文件
        let file_path = if game_rules_path.exists() {
            &game_rules_path
        } else if json_path.exists() {
            &json_path
        } else {
            return Ok(()); // 文件不存在，跳过
        };

        let modified = match read_file_metadata_for_hot_reload(file_path)? {
            Some(t) => t,
            None => return Ok(()),
        };

        // 检查是否是新文件或已修改
        let should_reload = match self.last_game_rules_mtime {
            Some(last) => modified > last,
            None => true,
        };

        if should_reload {
            self.last_game_rules_mtime = Some(modified);

            // 重新加载 game_rules
            match crate::game_data::load_from_dir(&config_dir) {
                Ok(new_data) => {
                    let version = new_data.game_rules.version.clone();

                    // 更新缓存
                    self.game_data_cache.update_game_rules(new_data.game_rules);

                    info!("游戏规则已热重载: version={}", version);

                    // 广播给所有在线 Agent
                    let config_update = ServerMessage::config_update_full_value(
                        ConfigType::GameRules,
                        version,
                        serde_json::to_value(&self.game_data_cache.get().game_rules)?,
                        None,
                    );

                    if let Err(e) =
                        broadcast_config_update(config_update, &self.connection_manager).await
                    {
                        warn!("广播游戏规则更新失败: {}", e);
                    }
                }
                Err(e) => {
                    warn!("重新加载 game_rules.yaml 失败: {}", e);
                }
            }
        }

        Ok(())
    }

    /// 检查 world_building_rules.yaml 是否变更，若变更则重新加载并广播
    pub(super) async fn check_and_reload_world_building_rules(&mut self) -> Result<()> {
        let config_dir = get_config_dir();
        let world_building_path = config_dir.join("world_building_rules.yaml");
        let json_path = config_dir.join("world_building_rules.json");

        // 确定实际使用的文件
        let file_path = if world_building_path.exists() {
            &world_building_path
        } else if json_path.exists() {
            &json_path
        } else {
            return Ok(()); // 文件不存在，跳过
        };

        let modified = match read_file_metadata_for_hot_reload(file_path)? {
            Some(t) => t,
            None => return Ok(()),
        };

        // 检查是否是新文件或已修改
        let should_reload = match self.last_world_building_rules_mtime {
            Some(last) => modified > last,
            None => true,
        };

        if should_reload {
            self.last_world_building_rules_mtime = Some(modified);

            // 重新加载 world_building_rules
            if let Some(world_building_rules) = crate::websocket::types::load_world_building_rules()
            {
                let version = world_building_rules.version.clone();

                info!("世界观规则已热重载: version={}", version);

                // 广播给所有在线 Agent
                let config_update = ServerMessage::config_update_full_value(
                    ConfigType::WorldBuildingRules,
                    version,
                    serde_json::to_value(&world_building_rules)?,
                    None,
                );

                if let Err(e) =
                    broadcast_config_update(config_update, &self.connection_manager).await
                {
                    warn!("广播世界观规则更新失败: {}", e);
                }
            }
        }

        Ok(())
    }
}
