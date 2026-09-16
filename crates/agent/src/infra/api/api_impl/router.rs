//! API 路由表（create_api_router）

use super::*;

/// 创建 HTTP API Router
///
/// 返回包含所有数据访问 API 的 Router，需要调用者提供 HttpApiState
pub fn create_api_router() -> Router<HttpApiState> {
    Router::new()
        // === API 发现端点 ===
        .route("/api/v1", get(handlers::api_list_handler)) // API 列表和使用规范
        // === 基础端点 ===
        .route("/api/v1/health", get(handlers::health_handler)) // 健康检查
        .route("/api/v1/version", get(handlers::version_handler)) // 协议握手（公开，无需认证）
        .route("/api/v1/state", get(handlers::get_state_handler)) // 获取当前世界状态
        .route("/api/v1/context", get(handlers::get_context_handler)) // 获取格式化上下文
        .route("/api/v1/attributes", get(handlers::get_attributes_handler)) // 梦中一瞥：属性数值
        .route("/api/v1/tick", get(handlers::get_tick_status_handler)) // 获取 Tick 状态（轮询用）
        // === 认知上下文端点（引导 OpenClaw 按阶段推理）===
        .route(
            "/api/v1/cognitive",
            get(handlers::get_cognitive_context_handler),
        ) // 结构化认知上下文
        // === 关系管理端点 ===
        .route(
            "/api/v1/relationship/list",
            get(handlers::get_relationships_handler),
        ) // 获取所有关系
        .route(
            "/api/v1/relationship/{id}",
            get(handlers::get_relationship_handler),
        ) // 获取特定关系
        .route(
            "/api/v1/relationship",
            post(handlers::update_relationship_handler),
        ) // 更新关系
        // === 寿命端点 ===
        .route("/api/v1/lifespan", get(handlers::get_lifespan_handler)) // 获取寿命状态
        // === 记忆管理端点 ===
        .route(
            "/api/v1/memory/recent",
            get(handlers::get_recent_memory_handler),
        ) // 获取近期记忆
        .route(
            "/api/v1/memory/daily-summaries",
            get(handlers::get_daily_summaries_handler),
        ) // 获取每日摘要
        .route(
            "/api/v1/memory/search",
            post(handlers::search_memory_handler),
        ) // 搜索记忆（语义搜索已实现，见 MemoryManager::recall_archived）
        .route("/api/v1/memory", post(handlers::store_memory_handler)) // 存储记忆
        // === 意图验证端点 ===
        .route("/api/v1/validate", post(handlers::validate_intent_handler)) // 验证意图是否符合人设
        // === 角色注册端点 ===
        .route(
            "/api/v1/character/generate",
            post(handlers::generate_character_handler),
        ) // LLM 一键生成角色
        .route(
            "/api/v1/character/register",
            post(handlers::register_character_handler),
        ) // 创建新角色（转发到 Server）
        .route(
            "/api/v1/admin/reload-character",
            post(handlers::reload_character),
        ) // 从 server reload 已注册角色（解决 server API register 缺 WS Registered 通道 gap）
        // === 角色信息端点 ===
        .route(
            "/api/v1/attribute-meta",
            get(handlers::get_attribute_meta_handler),
        ) // 属性元数据（分类）
        .route("/api/v1/character", get(handlers::get_character_handler)) // 获取角色信息
        .route(
            "/api/v1/character/soul-cycles",
            get(handlers::get_soul_cycles_handler),
        ) // 获取三魂循环完整记录（本地内存）
        .route(
            "/api/dashboard/layer-display",
            get(handlers::get_layer_display),
        ) // 天魂层展示名映射（数据驱动，从 souls.yaml 读取）
        .route(
            "/api/v1/character/biography",
            get(handlers::get_biography_handler),
        ) // 获取角色传记（缓存）
        .route(
            "/api/v1/character/biography",
            post(handlers::generate_biography_handler),
        ) // 生成角色传记（LLM 纪传体）
        .route(
            "/api/v1/character/rebirth",
            post(handlers::rebirth_character_handler),
        ) // 转生（强制归隐重新注册）
        .route("/api/v1/character/dream", get(handlers::get_dream_handler)) // 获取托梦状态
        .route(
            "/api/v1/character/dream",
            post(handlers::dream_character_handler),
        ) // 托梦（持续 n 回合的念头注入）
        .route(
            "/api/v1/character/dream/records",
            get(handlers::get_dream_records_handler),
        )
        // === 多角色管理端点 ===
        .route("/api/v1/characters", get(handlers::list_characters_handler)) // 获取所有角色列表
        .route(
            "/api/v1/characters/switch",
            post(handlers::switch_character_handler),
        ) // 切换当前角色
        .route(
            "/api/v1/characters/{agent_id}",
            get(handlers::get_character_by_id_handler),
        ) // 获取指定角色详情
        // === 角色管理端点（client 契约的 id 路由形态，委托现有 handler）===
        .route(
            "/api/v1/characters/{agent_id}/rebirth",
            post(handlers::rebirth_character_by_id_handler),
        ) // 重生（仅当前活跃角色，其余 409）
        .route(
            "/api/v1/characters/{agent_id}/inject-dream",
            post(handlers::inject_dream_by_id_handler),
        ) // 托梦（仅当前活跃角色，其余 409）
        .route(
            "/api/v1/characters/{agent_id}/biography",
            get(handlers::get_biography_by_id_handler),
        ) // 获取指定角色传记
        .route(
            "/api/v1/characters/{agent_id}/biography",
            post(handlers::generate_biography_by_id_handler),
        ) // 生成指定角色传记（LLM 纪传体）
        // === 实时事件端点（SSE）===
        .route("/api/v1/events", get(handlers::death_events_handler)) // 死亡事件 SSE 流
        .route("/api/v1/state/stream", get(handlers::state_stream_handler)) // WorldState+IntentSnapshot 复合 SSE 流（桌面 client 消费）
        // === 配置管理端点 ===
        .route("/api/v1/config", get(handlers::get_config_handler)) // 获取当前配置
        .route(
            "/api/v1/config/llm-disabled",
            get(handlers::get_llm_disabled_handler),
        ) // 获取 LLM 停止状态
        .route(
            "/api/v1/config/llm-disabled",
            post(handlers::set_llm_disabled_handler),
        ) // 设置 LLM 停止状态
        .route(
            "/api/v1/config/auto-rebirth",
            get(handlers::get_auto_rebirth_handler),
        ) // 获取自动重生开关
        .route(
            "/api/v1/config/auto-rebirth",
            post(handlers::set_auto_rebirth_handler),
        ) // 设置自动重生开关
        .route("/api/v1/actions", get(handlers::get_actions_handler)) // 获取动作类型映射
        .route("/api/v1/metrics", get(handlers::get_metrics_handler)) // LLM 性能指标
        .route(
            "/api/v1/config/reload",
            post(handlers::reload_config_handler),
        ) // 热重载配置
        .route("/api/v1/config/server", post(handlers::set_server_handler)) // 设置服务器地址
        // === 引导状态端点 ===
        .route("/api/v1/setup/status", get(handlers::setup_status_handler)) // 获取引导状态
        // === LLM 配置端点 ===
        .route(
            "/api/v1/config/llm/providers",
            get(handlers::get_llm_providers_handler),
        ) // 获取支持的 LLM Provider 列表
        .route(
            "/api/v1/config/llm/providers/openclaw/defaults",
            get(handlers::get_openclaw_defaults_handler),
        ) // 获取 OpenClaw 默认配置（仅当选择 openclaw 时调用）
        .route("/api/v1/config/llm", get(handlers::get_llm_config_handler)) // 获取当前 LLM 配置
        .route(
            "/api/v1/config/llm",
            post(handlers::update_llm_config_handler),
        ) // 更新 LLM 配置
        .route(
            "/api/v1/config/llm/usage",
            get(handlers::get_llm_usage_handler),
        ) // 获取 LLM Token 累计使用统计
        // === 自更新端点（GitHub Release，需 Bearer 认证） ===
        .route(
            "/api/v1/update/status",
            get(handlers::get_update_status_handler),
        ) // 更新状态视图（当前版本/最新 release/上次检查）
        .route(
            "/api/v1/update/check",
            post(handlers::post_update_check_handler),
        ) // 立即检查最新 release
        .route(
            "/api/v1/update/apply",
            post(handlers::post_update_apply_handler),
        ) // 下载安装最新版并重启
}
