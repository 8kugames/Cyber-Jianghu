// ============================================================================
// AgentClient（RwLock 包装 WebSocketClient 的门面，宏化转发）
// ============================================================================

use super::*;

// ============================================================================
// 旧接口兼容（AgentClient）
// ============================================================================

/// Agent 客户端（兼容旧接口）
///
/// 使用 tokio::sync::RwLock 替代 std::sync::RwLock，避免跨 await 持有同步锁导致死锁
/// AgentClient 对 WebSocketClient 一对一转发的宏生成（read 锁变体，&self 方法）
///
/// 内层是异步 RwLock，Deref 无法跨 await，转发统一由宏生成防止样板漂移；
/// write 锁方法与改名转发（close→disconnect）保留手写。
macro_rules! forward_read {
    ($(
        $(#[$meta:meta])*
        $vis:vis async fn $name:ident(&self $(, $arg:ident: $ty:ty)* $(,)?) $(-> $ret:ty)?;
    )+) => {
        $(
            $(#[$meta])*
            $vis async fn $name(&self $(, $arg: $ty)*) $(-> $ret)? {
                let client = self.client.read().await;
                client.$name($($arg),*).await
            }
        )+
    };
}

/// AgentClient 对 WebSocketClient 一对一转发的宏生成（write 锁变体，&mut self 方法）
macro_rules! forward_write {
    ($(
        $(#[$meta:meta])*
        $vis:vis async fn $name:ident(&self $(, $arg:ident: $ty:ty)* $(,)?);
    )+) => {
        $(
            $(#[$meta])*
            $vis async fn $name(&self $(, $arg: $ty)*) {
                let mut client = self.client.write().await;
                client.$name($($arg),*)
            }
        )+
    };
}

/// read 锁变体（内层同步方法）
macro_rules! forward_read_sync {
    ($(
        $(#[$meta:meta])*
        $vis:vis async fn $name:ident(&self $(, $arg:ident: $ty:ty)* $(,)?) $(-> $ret:ty)?;
    )+) => {
        $(
            $(#[$meta])*
            $vis async fn $name(&self $(, $arg: $ty)*) $(-> $ret)? {
                let client = self.client.read().await;
                client.$name($($arg),*)
            }
        )+
    };
}

pub struct AgentClient {
    client: RwLock<WebSocketClient>,
}

impl AgentClient {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            client: RwLock::new(WebSocketClient::new(config)),
        }
    }

    forward_write! {
        /// 设置设备身份
        pub async fn set_identity(&self, device_id: Uuid, auth_token: String);
        /// 更新服务器 URL（用于热切换）
        pub async fn update_server_url(&self, ws_url: String, http_url: String);
    }

    forward_read! {
        pub async fn connect(&self) -> Result<(), ConnectError>;
        pub async fn receive_world_state(&self) -> Result<WorldState>;
        /// 非阻塞 drain 事件流队列（与 WebSocketClient::try_drain_pending_events 同语义）
        pub async fn try_drain_pending_events(&self) -> Vec<WorldEvent>;
        pub async fn try_receive_execution_result(&self) -> Result<Vec<ExecutionResultData>>;
        pub async fn wait_for_execution_result(&self, timeout_ms: u64) -> Result<Vec<ExecutionResultData>>;
        pub async fn send_intent(
            &self,
            intent: &Intent,
            soul_cycle_metadata: Option<cyber_jianghu_protocol::SoulCycleMetadata>,
        ) -> Result<()>;
        /// 获取 Intent 发送端
        pub async fn intent_sender(&self) -> Option<tokio::sync::mpsc::Sender<ClientMessage>>;
        /// 发送三魂循环元数据
        pub async fn send_soul_cycle_report(
            &self,
            tick_id: i64,
            pipe_seq: i32,
            metadata: cyber_jianghu_protocol::SoulCycleMetadata,
        ) -> Result<()>;
        /// 发送每日 LLM 日志摘要
        pub async fn send_daily_summary(&self, game_day: i64, summary: &str) -> Result<()>;
        /// 发送关系图谱全量快照
        pub async fn send_relationship_snapshot(
            &self,
            agent_id: uuid::Uuid,
            game_day: i64,
            relationships: Vec<cyber_jianghu_protocol::types::RelationshipMemory>,
        ) -> Result<()>;
        pub async fn is_connected(&self) -> bool;
        /// 等待 Agent ID 可用（注册后）
        pub async fn wait_for_agent_id(&self) -> Result<Uuid>;
        /// 等待注册响应
        #[allow(clippy::type_complexity)]
        pub async fn wait_for_registration(
            &self,
        ) -> Result<
            Option<(
                Uuid,
                GameRules,
                Option<WorldBuildingRules>,
                Option<String>,
                bool,
                Option<cyber_jianghu_protocol::NarrativeConfig>,
                Option<String>,
            )>,
        >;
    }

    forward_read_sync! {
        /// 获取 Agent ID
        pub async fn agent_id(&self) -> Option<Uuid>;
        /// 设置游戏规则回调
        pub async fn set_game_rules_callback(
            &self,
            callback: Arc<dyn Fn(GameRules) + Send + Sync>,
        );
        /// 设置对话消息回调
        pub async fn set_dialogue_callback(
            &self,
            callback: Arc<dyn Fn(DialogueMessage) + Send + Sync>,
        );
        /// 设置世界观规则回调
        pub async fn set_world_building_rules_callback(
            &self,
            callback: Arc<dyn Fn(WorldBuildingRules) + Send + Sync>,
        );
        /// 设置技能配置更新回调
        /// 参数: (skills, removed_items)
        pub async fn set_skill_update_callback(&self, callback: SkillUpdateCallback);
        /// 设置动作配置更新回调（ConfigUpdate with config_type="actions"）
        pub async fn set_action_update_callback(
            &self,
            callback: Arc<dyn Fn(ServerMessage) + Send + Sync>,
        );
        /// 设置 Prompt 模板配置更新回调
        /// 参数: (PromptTemplateConfig)
        pub async fn set_prompt_template_callback(
            &self,
            callback: Arc<dyn Fn(cyber_jianghu_protocol::PromptTemplateConfig) + Send + Sync>,
        );
        /// 检查 WS 后台线程是否已成功投递 prompt_templates
        pub async fn is_prompt_template_received(&self) -> bool;
        /// 设置事件特质规则更新回调（ConfigUpdate with config_type="persona_event_rules"）
        pub async fn set_persona_event_rules_callback(
            &self,
            callback: Arc<dyn Fn(Vec<crate::component::persona::TraitMappingRule>) + Send + Sync>,
        );
        /// 设置叙事化配置更新回调（ConfigUpdate with config_type="narrative_config"）
        pub async fn set_narrative_config_callback(&self, callback: NarrativeConfigCallback);
        /// 设置 Server 消息透传回调（用于 OpenClaw 集成）
        ///
        /// 当收到 Server 下行消息时，此回调会被调用，允许将消息
        /// 转发到外部系统（如 OpenClaw）
        pub async fn set_server_msg_callback(
            &self,
            callback: Arc<dyn Fn(ServerMessage) + Send + Sync>,
        );
        /// 获取当前 server_msg_callback（用于 callback chaining）
        pub async fn get_server_msg_callback(
            &self,
        ) -> Option<Arc<dyn Fn(ServerMessage) + Send + Sync>>;
        /// 获取游戏规则
        pub async fn game_rules(&self) -> Option<GameRules>;
    }

    /// 设置指定的 Agent ID（用于热切换）
    pub async fn set_agent_id(&self, agent_id: Option<Uuid>) {
        let client = self.client.read().await;
        client.set_agent_id(agent_id);
    }

    /// 关闭连接（转发到 disconnect）
    pub async fn close(&self) {
        let client = self.client.read().await;
        client.disconnect().await
    }
}
