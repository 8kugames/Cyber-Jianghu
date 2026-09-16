//! prompt_templates / skills / narrative_config 热重载与 vendor refill

use super::*;

impl TickScheduler {
    /// 预加载 prompt_templates 到 AppState 缓存（Server 启动时调用一次）
    pub async fn preload_prompt_templates(&self) -> Result<()> {
        self.load_prompt_templates_to_cache().await
    }

    /// 从 YAML 加载 prompt_templates → JSON → hash → 写入缓存
    pub(super) async fn load_prompt_templates_to_cache(&self) -> Result<()> {
        let config_dir = get_config_dir();
        let path = config_dir.join("prompt_templates.yaml");

        if !path.exists() {
            return Ok(());
        }

        let yaml_content = std::fs::read_to_string(&path)
            .with_context(|| format!("读取 {} 失败", path.display()))?;

        let config: cyber_jianghu_protocol::PromptTemplateConfig =
            serde_yaml::from_str(&yaml_content)
                .with_context(|| format!("解析 {} 失败", path.display()))?;

        let version = config.version.clone();
        let json_bytes = config
            .to_json_bytes()
            .context("Prompt 模板 JSON 序列化失败")?;
        let hash = format!("{:x}", sha2::Sha256::digest(&json_bytes));
        let content: serde_json::Value = serde_json::from_slice(&json_bytes)
            .context("Prompt 模板 JSON bytes → Value 反序列化失败")?;

        info!(
            "Prompt 模板已加载: version={}, {} bytes, hash={}",
            version,
            json_bytes.len(),
            &hash[..12]
        );

        if let Some(cache) = &self.prompt_template_cache {
            let mut guard = cache.write().await;
            *guard = Some(crate::state::PromptTemplateCache {
                json_value: content,
                hash,
                version,
            });
        }

        // 将 canonical JSON 持久化到运行时数据目录，供 Agent HTTP 拉取
        let runtime_dir = crate::paths::get_data_dir();
        if let Err(e) = std::fs::create_dir_all(&runtime_dir) {
            warn!("运行时数据目录创建失败: {}", e);
        }
        let json_path = runtime_dir.join("prompt_templates.json");
        if let Err(e) = std::fs::write(&json_path, &json_bytes) {
            warn!("prompt_templates.json 写盘失败: {}", e);
        }

        Ok(())
    }

    /// 检查 prompt_templates.yaml 是否变更，若变更则重新加载并广播
    pub(super) async fn check_and_reload_prompt_templates(&mut self) -> Result<()> {
        let config_dir = get_config_dir();
        let prompt_templates_path = config_dir.join("prompt_templates.yaml");

        if !prompt_templates_path.exists() {
            return Ok(());
        }

        let modified = match read_file_metadata_for_hot_reload(&prompt_templates_path)? {
            Some(t) => t,
            None => return Ok(()),
        };

        let should_reload = match self.last_prompt_templates_mtime {
            Some(last) => modified > last,
            None => true,
        };

        if !should_reload {
            return Ok(());
        }

        self.last_prompt_templates_mtime = Some(modified);

        if let Err(e) = self.load_prompt_templates_to_cache().await {
            warn!("Prompt 模板热重载失败: {}", e);
            return Ok(());
        }

        // 从缓存读取并广播给在线 Agent
        if let Some(cache) = &self.prompt_template_cache {
            let guard = cache.read().await;
            if let Some(ref pt_cache) = *guard {
                let config_update = ServerMessage::config_update_full_value(
                    ConfigType::PromptTemplates,
                    pt_cache.version.clone(),
                    pt_cache.json_value.clone(),
                    Some(pt_cache.hash.clone()),
                );

                if let Err(e) =
                    broadcast_config_update(config_update, &self.connection_manager).await
                {
                    warn!("广播 prompt_templates 更新失败: {}", e);
                }
            }
        }

        Ok(())
    }

    /// 检查 skills/ 目录是否变更，若变更则重新加载并广播
    pub(super) async fn check_and_reload_skills(&mut self) -> Result<()> {
        use crate::game_data::loaders::load_skills;

        let config_dir = get_config_dir();
        let skills_path = config_dir.join("skills");

        if !skills_path.exists() {
            return Ok(()); // 目录不存在，跳过
        }

        let modified = match read_file_metadata_for_hot_reload(&skills_path)? {
            Some(t) => t,
            None => return Ok(()),
        };

        // 检查是否是新文件或已修改
        let should_reload = match self.last_skills_mtime {
            Some(last) => modified > last,
            None => true,
        };

        if should_reload {
            self.last_skills_mtime = Some(modified);

            // 重新加载 skills
            match load_skills(&skills_path) {
                Ok(new_skills) => {
                    let version = "1.0.0".to_string();
                    let skills_count = new_skills.len();

                    // 更新 GameDataCache（SkillsData）
                    // 注意：这需要 GameDataCache 支持 update_skills 方法
                    info!(
                        "技能配置已热重载: version={}, skills={}",
                        version, skills_count
                    );

                    // 构建 SkillContent 列表并广播
                    let skill_contents: Vec<cyber_jianghu_protocol::types::SkillContent> =
                        new_skills
                            .into_iter()
                            .map(
                                |(skill_id, def)| cyber_jianghu_protocol::types::SkillContent {
                                    skill_id,
                                    name: def.name,
                                    body: def.content,
                                },
                            )
                            .collect();

                    // 广播给所有在线 Agent
                    let config_update = ServerMessage::config_update_full_value(
                        ConfigType::Skills,
                        version,
                        serde_json::to_value(skill_contents).unwrap_or_default(),
                        None,
                    );

                    // 使用广播函数
                    let connections = self.connection_manager.read().await;
                    let mut success_count = 0;
                    let mut fail_count = 0;

                    for connection in connections.values() {
                        if connection.is_dead() {
                            fail_count += 1;
                            continue;
                        }

                        let json = serde_json::to_string(&config_update)?;
                        if connection
                            .send(axum::extract::ws::Message::Text(json.into()))
                            .await
                            .is_err()
                        {
                            fail_count += 1;
                        } else {
                            success_count += 1;
                        }
                    }

                    info!(
                        "Skills ConfigUpdate broadcast complete: {} success, {} failed",
                        success_count, fail_count
                    );
                }
                Err(e) => {
                    warn!("重新加载 skills/ 目录失败: {}", e);
                }
            }
        }

        Ok(())
    }

    /// 检查 narrative_config.yaml 是否变更，若变更则重新加载并广播
    pub(super) async fn check_and_reload_narrative_config(&mut self) -> Result<()> {
        let config_dir = get_config_dir();
        let nc_path = config_dir.join("narrative_config.yaml");

        if !nc_path.exists() {
            return Ok(());
        }

        let modified = match read_file_metadata_for_hot_reload(&nc_path)? {
            Some(t) => t,
            None => return Ok(()),
        };

        let should_reload = match self.last_narrative_config_mtime {
            Some(last) => modified > last,
            None => true,
        };

        if !should_reload {
            return Ok(());
        }

        self.last_narrative_config_mtime = Some(modified);

        // 从 GameData 重新加载 narrative_config（在块内克隆以避免跨 await 持有锁）
        let nc = self.game_data_cache.get().narrative.clone();

        let config_update =
            ServerMessage::config_update_full(ConfigType::NarrativeConfig, "1.0", &nc);

        if let Err(e) = broadcast_config_update(config_update, &self.connection_manager).await {
            warn!("广播 narrative_config 更新失败: {}", e);
        } else {
            info!("narrative_config 热重载广播完成");
        }

        Ok(())
    }

    /// Vendor 自动补货：从 DB 读取补货规则，低于 threshold 时触发，扣除银两
    pub(super) async fn refill_vendors(&mut self, tick_id: i64) -> Result<()> {
        let refill_rules = crate::db::get_all_enabled_vendor_refills(&self.db_pool)
            .await
            .context("读取 Vendor 补货规则失败")?;

        if refill_rules.is_empty() {
            return Ok(());
        }

        // 按 agent_id 分组
        let mut rules_by_agent: std::collections::HashMap<
            uuid::Uuid,
            Vec<&crate::db::VendorRefillRule>,
        > = std::collections::HashMap::new();
        for rule in &refill_rules {
            rules_by_agent.entry(rule.agent_id).or_default().push(rule);
        }

        for (agent_id, rules) in &rules_by_agent {
            // 查询当前库存
            let inventory: Vec<(String, i32)> =
                sqlx::query_as("SELECT item_id, quantity FROM agent_inventory WHERE agent_id = $1")
                    .bind(*agent_id)
                    .fetch_all(&self.db_pool)
                    .await
                    .context("查询 Vendor 库存失败")?;

            let inv_map: std::collections::HashMap<String, i32> = inventory.into_iter().collect();

            let silver = inv_map.get("银子").copied().unwrap_or(0);
            if silver == 0 {
                continue;
            }

            // 取所有规则中最高的 budget_ratio
            let budget_ratio = rules.iter().map(|r| r.budget_ratio).max().unwrap_or(50);
            let max_spend = silver * budget_ratio / 100;
            let mut total_spent = 0i32;
            let mut restocked_items: Vec<(String, i32)> = Vec::new();

            for rule in rules {
                let current = inv_map.get(&rule.item_id).copied().unwrap_or(0);
                if current >= rule.threshold {
                    continue;
                }

                let remaining_budget = max_spend - total_spent;
                if remaining_budget <= 0 {
                    break;
                }
                // refill_to 语义是"补到该数量"（handler 校验 refill_to > threshold），
                // 采购量 = 目标 - 现有，受剩余预算封顶；旧实现按 refill_to 全量买入，
                // 库存接近阈值时也会超补
                let buy_count = (rule.refill_to - current).max(0).min(remaining_budget);
                if buy_count <= 0 {
                    continue;
                }

                sqlx::query(
                    "INSERT INTO agent_inventory (agent_id, item_id, quantity) \
                     VALUES ($1, $2, $3) \
                     ON CONFLICT (agent_id, item_id) \
                     DO UPDATE SET quantity = agent_inventory.quantity + EXCLUDED.quantity, \
                                   updated_at = CURRENT_TIMESTAMP",
                )
                .bind(*agent_id)
                .bind(&rule.item_id)
                .bind(buy_count)
                .execute(&self.db_pool)
                .await
                .context("Vendor 补货失败")?;

                total_spent += buy_count;

                let item_name = crate::game_data::registry::ItemRegistry::get(&rule.item_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| rule.item_id.clone());
                restocked_items.push((item_name, buy_count));

                info!(
                    "Vendor 补货: agent={} item={} qty={} ({} -> {})",
                    agent_id,
                    rule.item_id,
                    buy_count,
                    current,
                    current + buy_count
                );
            }

            if total_spent > 0 {
                // 相对增量扣银：绝对值 SET quantity=$1 会与 IntentWorker 正在处理的
                // 交易（买入/卖出改银子）互相覆盖，后写者吞掉先写者的变更；
                // 单语句相对减法是原子的，不再丢更新
                if total_spent >= silver {
                    sqlx::query(
                        "DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = '银子'",
                    )
                    .bind(*agent_id)
                    .execute(&self.db_pool)
                    .await
                    .context("扣除 Vendor 银两失败")?;
                } else {
                    sqlx::query(
                        "UPDATE agent_inventory \
                         SET quantity = agent_inventory.quantity - $1, \
                             updated_at = CURRENT_TIMESTAMP \
                         WHERE agent_id = $2 AND item_id = '银子'",
                    )
                    .bind(total_spent)
                    .bind(*agent_id)
                    .execute(&self.db_pool)
                    .await
                    .context("扣除 Vendor 银两失败")?;
                }

                // 注入 LLM 消息
                let items_desc: String = restocked_items
                    .iter()
                    .map(|(name, qty)| format!("{}x{}", name, qty))
                    .collect::<Vec<_>>()
                    .join(", ");

                let messages = [
                    format!("从外地采购{}，可用于销售", items_desc),
                    format!("新到一批货：{}，可用于销售", items_desc),
                ];
                let msg = &messages[tick_id as usize % messages.len()];

                // 经 vendor_pending_events 通道：直接写 event_manager 会被
                // 随后 broadcast_new_tick 开头的 clear() 清空，vendor 永远收不到
                self.vendor_pending_events
                    .entry(*agent_id)
                    .or_default()
                    .push(crate::models::WorldEvent {
                        event_type: cyber_jianghu_protocol::WorldEventType::SystemNotification,
                        tick_id,
                        description: msg.clone(),
                        metadata: serde_json::json!({
                            "type": "vendor_restock",
                            "items": restocked_items.iter().map(|(n, q)| serde_json::json!({"name": n, "quantity": q})).collect::<Vec<_>>(),
                            "cost_silver": total_spent,
                        }),
                    });

                info!(
                    "Vendor 补货完成: agent={} spent={} silver_before={}",
                    agent_id, total_spent, silver
                );
            }
        }

        Ok(())
    }
}
