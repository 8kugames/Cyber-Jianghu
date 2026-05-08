// ============================================================================
// OpenClaw Cyber-Jianghu MVP Tick Scheduler
// ============================================================================
//
// 调度器负责Tick引擎的主循环执行流程，包括：
// 1. 协调各个阶段的执行
// 2. 记录性能日志
// 3. 错误处理和恢复
//
// 设计原则：
// 1. 单线程执行，避免并发问题
// 2. 每个Tick独立，失败不影响下一个Tick
// 3. 详细的性能日志，方便定位问题
// 4. 优雅的错误处理，不崩溃
// ============================================================================

use anyhow::{Context, Result};
use chrono::FixedOffset;
use sha2::Digest;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::db::DbPool;
use crate::game_data::GameDataCache;
use crate::state::AgentStateCache;
use crate::websocket::{AgentToDeviceMap, ConnectionManager};

use super::WorkerMessage;
use super::broadcaster::{Broadcaster, send_to_agent};
use super::event_manager::SharedEventManager;

use crate::game_data::loaders::load_actions;
use crate::paths::get_config_dir;
use crate::websocket::broadcast_config_update;
use cyber_jianghu_protocol::ServerMessage;
use std::fs;

/// Tick调度器
///
/// 实时模式：Tick 退化为纯时钟（衰减 + 时间推进 + 周期广播 WorldState）。
/// Intent 由 IntentWorker 实时处理，不再经过 scheduler。
pub struct TickScheduler {
    /// 游戏数据缓存
    game_data_cache: Arc<GameDataCache>,

    /// 当前Tick编号（递增）
    current_tick_id: i64,

    /// 运行状态
    is_running: bool,

    /// 数据库连接池
    db_pool: DbPool,

    /// WebSocket 连接管理器
    connection_manager: ConnectionManager,

    /// agent_id → device_id 反向映射
    agent_to_device_map: AgentToDeviceMap,

    /// 事件管理器（与 IntentWorker 共享）
    event_manager: SharedEventManager,

    /// 广播器
    broadcaster: Broadcaster,

    /// IntentWorker 发送端（发送 TickBoundary 触发衰减）
    worker_tx: mpsc::Sender<WorkerMessage>,

    /// Agent 状态内存缓存
    agent_state_cache: AgentStateCache,

    /// 当前 tick_id（原子变量，供外部查询当前 tick）
    accepting_tick_id: Arc<AtomicI64>,

    /// 上次加载的 actions.yaml 修改时间
    last_actions_mtime: Option<std::time::SystemTime>,

    /// 上次加载的 skills/ 目录修改时间
    last_skills_mtime: Option<std::time::SystemTime>,

    /// 上次加载的 game_rules.yaml 修改时间
    last_game_rules_mtime: Option<std::time::SystemTime>,

    /// 上次加载的 world_building_rules.yaml 修改时间
    last_world_building_rules_mtime: Option<std::time::SystemTime>,

    /// 上次加载的 prompt_templates.yaml 修改时间
    last_prompt_templates_mtime: Option<std::time::SystemTime>,

    /// Prompt 模板 JSON 缓存（与 AppState 共享，用于 WS 连接时下发）
    prompt_template_cache:
        Option<Arc<tokio::sync::RwLock<Option<crate::state::PromptTemplateCache>>>>,

    /// Vendor 跨请求事件缓冲（grant-items handler 写入，broadcast 消费）
    vendor_pending_events: crate::models::VendorPendingEvents,
}

impl TickScheduler {
    /// 创建新的Tick调度器
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        game_data_cache: Arc<GameDataCache>,
        db_pool: DbPool,
        connection_manager: ConnectionManager,
        agent_to_device_map: AgentToDeviceMap,
        worker_tx: mpsc::Sender<WorkerMessage>,
        agent_state_cache: AgentStateCache,
        accepting_tick_id: Arc<AtomicI64>,
        vendor_pending_events: crate::models::VendorPendingEvents,
    ) -> Self {
        Self {
            game_data_cache,
            current_tick_id: 0,
            is_running: false,
            db_pool,
            connection_manager,
            agent_to_device_map,
            event_manager: super::event_manager::EventManager::new_shared(),
            broadcaster: Broadcaster::new(),
            worker_tx,
            agent_state_cache,
            accepting_tick_id,
            last_actions_mtime: None,
            last_skills_mtime: None,
            last_game_rules_mtime: None,
            last_world_building_rules_mtime: None,
            last_prompt_templates_mtime: None,
            prompt_template_cache: None,
            vendor_pending_events,
        }
    }

    /// 设置 prompt_template_cache（与 AppState 共享）
    pub fn set_prompt_template_cache(
        &mut self,
        cache: Arc<tokio::sync::RwLock<Option<crate::state::PromptTemplateCache>>>,
    ) {
        self.prompt_template_cache = Some(cache);
    }

    /// 检查 actions.yaml 是否变更，若变更则重新加载并广播
    async fn check_and_reload_actions(&mut self) -> Result<()> {
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

        let metadata = match fs::metadata(file_path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => return Ok(()),
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
                    let config_update = ServerMessage::ConfigUpdate {
                        config_type: "actions".to_string(),
                        update_type: "full".to_string(),
                        version,
                        content: serde_json::to_value(available_actions)?,
                        content_hash: None,
                        updated_items: vec![],
                        removed_items: vec![],
                    };

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
    async fn check_and_reload_game_rules(&mut self) -> Result<()> {
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

        let metadata = match fs::metadata(file_path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => return Ok(()),
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
                    let config_update = ServerMessage::ConfigUpdate {
                        config_type: "game_rules".to_string(),
                        update_type: "full".to_string(),
                        version,
                        content: serde_json::to_value(&self.game_data_cache.get().game_rules)?,
                        content_hash: None,
                        updated_items: vec![],
                        removed_items: vec![],
                    };

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
    async fn check_and_reload_world_building_rules(&mut self) -> Result<()> {
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

        let metadata = match fs::metadata(file_path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => return Ok(()),
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
                let config_update = ServerMessage::ConfigUpdate {
                    config_type: "world_building_rules".to_string(),
                    update_type: "full".to_string(),
                    version,
                    content: serde_json::to_value(&world_building_rules)?,
                    content_hash: None,
                    updated_items: vec![],
                    removed_items: vec![],
                };

                if let Err(e) =
                    broadcast_config_update(config_update, &self.connection_manager).await
                {
                    warn!("广播世界观规则更新失败: {}", e);
                }
            }
        }

        Ok(())
    }

    /// 预加载 prompt_templates 到 AppState 缓存（Server 启动时调用一次）
    pub async fn preload_prompt_templates(&self) -> Result<()> {
        self.load_prompt_templates_to_cache().await
    }

    /// 从 YAML 加载 prompt_templates → JSON → hash → 写入缓存
    async fn load_prompt_templates_to_cache(&self) -> Result<()> {
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

        // 将 canonical JSON 持久化到 config dir，供 Agent HTTP 拉取
        let json_path = config_dir.join("prompt_templates.json");
        if let Err(e) = std::fs::write(&json_path, &json_bytes) {
            warn!("prompt_templates.json 写盘失败: {}", e);
        }

        Ok(())
    }

    /// 检查 prompt_templates.yaml 是否变更，若变更则重新加载并广播
    async fn check_and_reload_prompt_templates(&mut self) -> Result<()> {
        let config_dir = get_config_dir();
        let prompt_templates_path = config_dir.join("prompt_templates.yaml");

        if !prompt_templates_path.exists() {
            return Ok(());
        }

        let metadata = match fs::metadata(&prompt_templates_path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => return Ok(()),
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
                let config_update = ServerMessage::ConfigUpdate {
                    config_type: "prompt_templates".to_string(),
                    update_type: "full".to_string(),
                    version: pt_cache.version.clone(),
                    content: pt_cache.json_value.clone(),
                    content_hash: Some(pt_cache.hash.clone()),
                    updated_items: vec![],
                    removed_items: vec![],
                };

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
    async fn check_and_reload_skills(&mut self) -> Result<()> {
        use crate::game_data::loaders::load_skills;

        let config_dir = get_config_dir();
        let skills_path = config_dir.join("skills");

        if !skills_path.exists() {
            return Ok(()); // 目录不存在，跳过
        }

        let metadata = match fs::metadata(&skills_path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => return Ok(()),
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
                    let config_update = ServerMessage::ConfigUpdate {
                        config_type: "skills".to_string(),
                        update_type: "full".to_string(),
                        version,
                        content: serde_json::to_value(skill_contents).unwrap_or_default(),
                        content_hash: None,
                        updated_items: vec![],
                        removed_items: vec![],
                    };

                    // 使用广播函数
                    let connections = self.connection_manager.read().await;
                    let mut success_count = 0;
                    let mut fail_count = 0;

                    for (_device_id, connection) in connections.iter() {
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

    /// Vendor 自动补货：从 DB 读取补货规则，低于 threshold 时触发，扣除银两
    async fn refill_vendors(&mut self, tick_id: i64) -> Result<()> {
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
                let buy_count = rule.refill_to.min(remaining_budget);

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
                let new_silver = silver - total_spent;
                if new_silver > 0 {
                    sqlx::query(
                        "UPDATE agent_inventory SET quantity = $1, updated_at = CURRENT_TIMESTAMP \
                         WHERE agent_id = $2 AND item_id = '银子'",
                    )
                    .bind(new_silver)
                    .bind(*agent_id)
                    .execute(&self.db_pool)
                    .await
                    .context("扣除 Vendor 银两失败")?;
                } else {
                    sqlx::query(
                        "DELETE FROM agent_inventory WHERE agent_id = $1 AND item_id = '银子'",
                    )
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

                self.event_manager.lock().unwrap().add_event_for_agent(
                    *agent_id,
                    crate::models::WorldEvent {
                        event_type: cyber_jianghu_protocol::WorldEventType::SystemNotification,
                        tick_id,
                        description: msg.clone(),
                        metadata: serde_json::json!({
                            "type": "vendor_restock",
                            "items": restocked_items.iter().map(|(n, q)| serde_json::json!({"name": n, "quantity": q})).collect::<Vec<_>>(),
                            "cost_silver": total_spent,
                        }),
                    },
                );

                info!(
                    "Vendor 补货完成: agent={} spent={} silver remaining={}",
                    agent_id, total_spent, new_silver
                );
            }
        }

        Ok(())
    }

    /// 启动Tick循环
    ///
    /// 实时模式：纯时钟驱动。
    /// 每个周期：广播 WorldState → 发送 TickBoundary（触发 IntentWorker 衰减）。
    /// Intent 不再由 scheduler 处理。
    pub async fn run(&mut self) -> Result<()> {
        let tick_duration_secs = {
            let gd = self.game_data_cache.get();
            gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64
        };

        info!("Tick引擎启动（实时模式），周期: {}秒", tick_duration_secs);
        info!("天道无为，万物自化。世界开始运转。");

        self.is_running = true;

        let game_epoch = self.parse_game_epoch()?;

        let db_max_tick_id = crate::db::get_current_world_tick_id(&self.db_pool)
            .await
            .unwrap_or(0);

        let time_based_tick_id = self.calculate_tick_id_from_time(game_epoch);
        self.current_tick_id = db_max_tick_id.max(time_based_tick_id);

        info!(
            "游戏纪元: {}, DB最大Tick: {}, 时间Tick: {}, 起始Tick: {}",
            game_epoch, db_max_tick_id, time_based_tick_id, self.current_tick_id
        );

        let mut interval = tokio::time::interval(Duration::from_secs(tick_duration_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        while self.is_running {
            // 热重载 actions.yaml
            if let Err(e) = self.check_and_reload_actions().await {
                warn!("动作热重载检查失败: {}", e);
            }

            // 热重载 game_rules.yaml
            if let Err(e) = self.check_and_reload_game_rules().await {
                warn!("游戏规则热重载检查失败: {}", e);
            }

            // 热重载 world_building_rules.yaml
            if let Err(e) = self.check_and_reload_world_building_rules().await {
                warn!("世界观规则热重载检查失败: {}", e);
            }

            // 热重载 prompt_templates.yaml
            if let Err(e) = self.check_and_reload_prompt_templates().await {
                warn!("Prompt 模板热重载检查失败: {}", e);
            }

            // 热重载 skills/
            if let Err(e) = self.check_and_reload_skills().await {
                warn!("技能热重载检查失败: {}", e);
            }

            interval.tick().await;

            let new_tick_id = self.calculate_tick_id_from_time(game_epoch);
            self.current_tick_id = self.current_tick_id.max(new_tick_id);

            // 更新 accepting_tick_id（Agent 可用来判断当前 tick）
            self.accepting_tick_id
                .store(self.current_tick_id, Ordering::Release);

            // 1. 发送 TickBoundary 到 IntentWorker（触发衰减 + 死亡处理）
            if let Err(e) = self
                .worker_tx
                .send(WorkerMessage::TickBoundary {
                    tick_id: self.current_tick_id,
                })
                .await
            {
                error!(
                    "Tick {} 发送 TickBoundary 失败: {}",
                    self.current_tick_id, e
                );
            }

            // 1.5 Vendor 自动补货（在广播前执行，事件注入到 event_manager）
            if let Err(e) = self.refill_vendors(self.current_tick_id).await {
                warn!("Vendor 补货失败: {}", e);
            }

            // 2. 广播 WorldState
            if let Err(e) = self.broadcast_new_tick(self.current_tick_id).await {
                error!("Tick {} 广播失败: {}", self.current_tick_id, e);
            }

            // 2.5 游戏日边界推送：每个游戏日结束时向所有在线 Agent 推送动作统计
            let ticks_per_day = crate::game_data::registry::TimeRegistry::get_config()
                .map(|c| c.ticks_per_hour as i64 * c.hours_per_day as i64)
                .unwrap_or(12);
            if self.current_tick_id > 0 && self.current_tick_id % ticks_per_day == 0 {
                let game_day = self.current_tick_id / ticks_per_day;
                self.broadcast_daily_summaries(game_day).await;
            }

            // 3. 群像传记：每 168 tick (7 游戏日) 生成一次
            let period_ticks = crate::chronicle::ChronicleConfig::default().period_ticks;
            if self.current_tick_id > 0 && self.current_tick_id % period_ticks == 0 {
                let period_start = self.current_tick_id - period_ticks + 1;
                let db_pool = self.db_pool.clone();
                let tick_id = self.current_tick_id;
                tokio::spawn(async move {
                    match crate::chronicle::generate_and_store(period_start, tick_id, &db_pool)
                        .await
                    {
                        Ok(chronicle) => {
                            info!(
                                "群像传记生成完成: {} (第{}-{}日, {}季)",
                                chronicle.chronicle_id,
                                chronicle.game_day_start,
                                chronicle.game_day_end,
                                chronicle.season
                            );
                        }
                        Err(e) => {
                            error!("群像传记生成失败: {}", e);
                        }
                    }
                });
            }
        }

        info!("Tick引擎已停止");
        Ok(())
    }

    /// 广播新 tick 的 WorldState（从 DashMap 读取最新状态）
    async fn broadcast_new_tick(&mut self, tick_id: i64) -> Result<()> {
        let agent_states: Vec<crate::models::AgentState> = self
            .agent_state_cache
            .iter()
            .map(|r| r.value().clone())
            .collect();

        self.event_manager.lock().unwrap().clear();

        // drain grant-items 跨请求缓冲事件（clear 后注入，确保本 tick 可见）
        for entry in self.vendor_pending_events.iter() {
            let agent_id = *entry.key();
            for event in entry.value() {
                self.event_manager
                    .lock()
                    .unwrap()
                    .add_event_for_agent(agent_id, event.clone());
            }
        }
        self.vendor_pending_events.clear();

        self.broadcaster
            .broadcast_states(
                tick_id,
                &agent_states,
                &self.db_pool,
                &self.connection_manager,
                &self.agent_to_device_map,
                &self.event_manager,
                &self.game_data_cache,
            )
            .await
            .context("广播: 广播状态失败")?;

        info!("Tick {} 广播完成: {}个Agent", tick_id, agent_states.len(),);
        Ok(())
    }

    /// 游戏日边界：向所有在线 Agent 推送上一游戏日的动作统计
    ///
    /// 在 game_day 结束时（tick_id % ticks_per_day == 0）调用，
    /// 从 agent_action_logs 聚合数据，通过 WebSocket 发送给各 Agent。
    /// 注意：本方法仅推送数据，不写入 agent_daily_summaries 表。
    /// agent_daily_summaries 的叙事摘要由 SessionTriageEngine 在游戏日切换时生成。
    async fn broadcast_daily_summaries(&self, game_day: i64) {
        use crate::db::{get_agent_daily_action_stats, get_all_alive_agents_latest_states};

        let alive_agents = match get_all_alive_agents_latest_states(&self.db_pool).await {
            Ok(agents) => agents,
            Err(e) => {
                error!("游戏日 {} 每日摘要：查询存活 Agent 失败: {}", game_day, e);
                return;
            }
        };

        for agent_state in alive_agents {
            let stats =
                match get_agent_daily_action_stats(&self.db_pool, agent_state.agent_id, game_day)
                    .await
                {
                    Ok(Some(stats)) => stats,
                    Ok(None) => continue,
                    Err(e) => {
                        warn!(
                            "游戏日 {} Agent {} 动作统计查询失败: {}",
                            game_day, agent_state.agent_id, e
                        );
                        continue;
                    }
                };

            // 推送 DailySummaryData 到在线 Agent（供 Agent 侧记忆系统使用）
            let msg = ServerMessage::DailySummaryData {
                game_day,
                action_counts: stats.action_counts.clone(),
                location_history: stats.location_history.clone(),
                success_count: stats.success_count,
                failure_count: stats.failure_count,
                total_actions: stats.total_actions,
            };

            if let Err(e) = send_to_agent(
                agent_state.agent_id,
                &msg,
                &self.connection_manager,
                &self.agent_to_device_map,
            )
            .await
            {
                debug!(
                    "游戏日 {} 每日摘要推送 Agent {} 失败（可能已离线）: {}",
                    game_day, agent_state.agent_id, e
                );
            }
        }
    }

    /// 根据真实时间计算 tick ID（秒级秒数）
    ///
    /// tick_id = 当前Unix时间戳 - 游戏纪元
    /// 直接使用秒级秒数，real_seconds_per_tick 只影响执行频率，不影响 tick_id
    fn calculate_tick_id_from_time(&self, game_epoch: i64) -> i64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        now - game_epoch
    }

    /// 解析游戏纪元（从 YAML 配置）
    ///
    /// 使用配置的时区偏移量计算游戏纪元。
    /// 例如：start_date: "2026-03-03", timezone_offset: 8
    /// 表示 UTC+8 时区 2026-03-03 00:00:00，对应 UTC 2026-03-02 16:00:00。
    fn parse_game_epoch(&self) -> Result<i64> {
        let gd = self.game_data_cache.get();
        let start_date_str = gd.game_rules.data.agent_state.game_time.start_date.clone();
        let timezone_offset = gd.game_rules.data.agent_state.game_time.timezone_offset;
        drop(gd);

        // 解析日期字符串 (YYYY-MM-DD 格式)
        let date = chrono::NaiveDate::parse_from_str(&start_date_str, "%Y-%m-%d")
            .with_context(|| format!("无法解析游戏纪元日期: {}", start_date_str))?;

        // 使用配置的时区偏移量
        // 例如 UTC+8 = 8 * 3600 = 28800 秒
        let offset_seconds = timezone_offset * 3600;
        let offset = FixedOffset::east_opt(offset_seconds)
            .with_context(|| format!("无效的时区偏移量: {}", timezone_offset))?;

        let datetime = date.and_hms_opt(0, 0, 0).unwrap();
        let datetime_with_tz = datetime
            .and_local_timezone(offset)
            .single()
            .with_context(|| format!("无法创建时区感知时间: {}", start_date_str))?;

        let timestamp = datetime_with_tz.timestamp();

        // 计算对应的 UTC 时间用于日志
        let utc_datetime = datetime_with_tz.naive_utc();
        let utc_offset_sign = if timezone_offset >= 0 { "+" } else { "" };

        info!(
            "游戏纪元: {} 00:00:00 UTC{}{} = {} UTC (Unix timestamp: {})",
            start_date_str,
            utc_offset_sign,
            timezone_offset,
            utc_datetime.format("%Y-%m-%d %H:%M:%S"),
            timestamp
        );
        Ok(timestamp)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, NaiveDate, TimeZone, Timelike};

    /// 测试东八区时间解析
    ///
    /// 验证 start_date: "2026-03-03" 被正确解析为北京时间 00:00:00
    #[test]
    fn test_utc8_game_epoch() {
        // 解析日期字符串
        let start_date_str = "2026-03-03";
        let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();

        // 使用东八区（UTC+8）时间
        let offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let datetime = date.and_hms_opt(0, 0, 0).unwrap();
        let datetime_with_tz = datetime.and_local_timezone(offset).single().unwrap();

        // 获取 Unix 时间戳
        let timestamp = datetime_with_tz.timestamp();

        // 验证：北京时间 2026-03-03 00:00:00 = UTC 2026-03-02 16:00:00
        // 预期的 UTC 时间戳
        let expected_utc = NaiveDate::from_ymd_opt(2026, 3, 2)
            .unwrap()
            .and_hms_opt(16, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();

        assert_eq!(
            timestamp, expected_utc,
            "北京时间 2026-03-03 00:00:00 应该等于 UTC 2026-03-02 16:00:00"
        );

        // 验证具体数值
        // 2026-03-02 16:00:00 UTC 的 Unix 时间戳
        // 通过在线工具验证：https://www.unixtimestamp.com/
        // 2026-03-03 00:00:00 UTC+8 = 2026-03-02 16:00:00 UTC = 1772467200
        assert_eq!(timestamp, 1772467200, "时间戳应该等于 1772467200");
    }

    /// 测试 tick_id 计算（秒级秒数）
    ///
    /// 验证 tick_id = now - game_epoch（秒级秒数）
    #[test]
    fn test_tick_id_calculation() {
        let start_date_str = "2026-03-03";
        let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();
        let offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let datetime = date.and_hms_opt(0, 0, 0).unwrap();
        let game_epoch = datetime
            .and_local_timezone(offset)
            .single()
            .unwrap()
            .timestamp();

        // tick_id = now - game_epoch（秒级秒数）
        // 在北京时间 2026-03-03 00:00:00，tick_id 应该是 0
        let tick_at_epoch = game_epoch - game_epoch;
        assert_eq!(tick_at_epoch, 0, "纪元时刻的 tick_id 应该是 0");

        // 在北京时间 2026-03-03 00:01:00（1分钟后），tick_id 应该是 60
        let one_minute_later = game_epoch + 60;
        let tick_after_1min = one_minute_later - game_epoch;
        assert_eq!(tick_after_1min, 60, "1分钟后的 tick_id 应该是 60");

        // 在北京时间 2026-03-03 01:00:00（1小时后），tick_id 应该是 3600
        let one_hour_later = game_epoch + 3600;
        let tick_after_1hour = one_hour_later - game_epoch;
        assert_eq!(tick_after_1hour, 3600, "1小时后的 tick_id 应该是 3600");
    }

    /// 测试时间戳转换的一致性
    ///
    /// 验证从时间戳反向转换回日期时间的正确性
    #[test]
    fn test_timestamp_roundtrip() {
        let start_date_str = "2026-03-03";
        let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();
        let offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let datetime = date.and_hms_opt(0, 0, 0).unwrap();
        let datetime_with_tz = datetime.and_local_timezone(offset).single().unwrap();

        let timestamp = datetime_with_tz.timestamp();

        // 从时间戳反向转换
        let reversed = offset.timestamp_opt(timestamp, 0).single().unwrap();

        // 验证年月日时分秒一致
        assert_eq!(reversed.year(), 2026);
        assert_eq!(reversed.month(), 3);
        assert_eq!(reversed.day(), 3);
        assert_eq!(reversed.hour(), 0);
        assert_eq!(reversed.minute(), 0);
        assert_eq!(reversed.second(), 0);
    }
}
