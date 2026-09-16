//! 路由表构建（自 main.rs 外移的阶段 11）

use super::admin_static::{serve_admin, serve_admin_index};
use axum::{
    Router,
    routing::{delete, get, post, put},
};
use cyber_jianghu_server::handlers;

/// 构建全站路由（阶段 11）；state 由调用方注入
pub(crate) fn build_router(
    state: std::sync::Arc<cyber_jianghu_server::state::AppState>,
) -> axum::Router {
    Router::new()
        .route("/", get(handlers::system::root))
        .route("/health", get(handlers::system::health_check))
        .route("/api/v1/version", get(handlers::system::version)) // 协议握手（公开，无需认证）
        // 设备身份生命周期 v2 — 严格校验（DB 不存在时返回 404）
        .route(
            "/api/v1/device/verify",
            post(handlers::device::device_verify),
        )
        // 设备身份生命周期 v2 — 显式注册（server 生成 device_id，201 Created）
        .route(
            "/api/v1/device/register",
            post(handlers::device::device_register),
        )
        // 角色注册 - 创建游戏角色
        .route(
            "/api/v1/agent/register",
            post(handlers::agent::agent_register),
        )
        // 角色归隐 - 将活跃角色标记为 retired，允许创建新角色
        .route("/api/v1/agent/retire", post(handlers::agent::agent_retire))
        // 设备→活跃角色查询（用于 agent 端 reload 已注册角色）
        .route(
            "/api/v1/agent/by-device",
            post(handlers::agent_by_device::get_agent_by_device),
        )
        // 自动重生 - Agent 死亡后延迟调用
        .route(
            "/api/v1/agent/auto-rebirth",
            post(handlers::agent::agent_auto_rebirth),
        )
        // 管理员库存注入（Vendor 补货等）
        .route(
            "/api/v1/agent/grant-items",
            post(handlers::agent::agent_grant_items).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_write_token,
            )),
        )
        .route(
            "/api/v1/agent/grant-recipes",
            post(handlers::agent::agent_grant_recipes).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_write_token,
            )),
        )
        // 传记回传 - Agent 端死亡/归隐时将纪传体传记回传 server
        .route(
            "/api/v1/agent/biography",
            post(handlers::agent::update_biography),
        )
        // 传记查询 - Agent 端回退读取（本地无传记时从 server DB 获取）
        .route(
            "/api/v1/agent/{id}/biography",
            get(handlers::agent::get_agent_biography),
        )
        // Prompt Templates 拉取 — Agent 启动时主动获取
        .route(
            "/api/v1/agent/prompt-templates",
            post(handlers::agent::get_prompt_templates),
        )
        // Vendor 补货规则管理
        // 注意：MethodRouter::layer 只包裹调用时已有的方法。读写分权路由必须用
        // merge 拼接，否则后加的 write layer 会把 GET 一并包进写鉴权。
        .route(
            "/api/dashboard/agent/{id}/vendor-refill",
            get(handlers::vendor::get_vendor_refill_rules)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ))
                .merge(put(handlers::vendor::set_vendor_refill_rule).layer(
                    axum::middleware::from_fn_with_state(
                        state.clone(),
                        handlers::auth::require_write_token,
                    ),
                )),
        )
        .route(
            "/api/dashboard/agent/{id}/vendor-refill/{item_id}",
            delete(handlers::vendor::delete_vendor_refill_rule).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agent/{id}/roles",
            get(handlers::role::get_agent_roles_handler)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ))
                .merge(post(handlers::role::assign_role_handler).layer(
                    axum::middleware::from_fn_with_state(
                        state.clone(),
                        handlers::auth::require_write_token,
                    ),
                )),
        )
        .route(
            "/api/dashboard/roles",
            get(handlers::role::list_available_roles).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/agent/{id}/roles/{role_key}",
            delete(handlers::role::remove_role_handler).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        .route(
            "/api/v1/agent/{id}/context",
            get(handlers::context::get_agent_context),
        )
        .route(
            "/api/v1/validate-action",
            post(handlers::validation::validate_action),
        )
        .route(
            "/ws",
            get(cyber_jianghu_server::websocket::websocket_handler),
        )
        // Dashboard API - 无需认证
        .route(
            "/api/dashboard/actions-map",
            get(handlers::dashboard::get_actions_map),
        )
        .route(
            "/api/dashboard/items",
            get(handlers::dashboard::get_items).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        // 展示名映射（经历日志前端翻译 agent_id/item_id 用）
        .route(
            "/api/dashboard/display-map",
            get(handlers::dashboard::get_display_map).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        // 天魂层展示名映射（数据驱动，从 souls.yaml 读取）
        .route(
            "/api/dashboard/layer-display",
            get(handlers::dashboard::get_layer_display),
        )
        // Dashboard API (需要 Read 权限)
        .route(
            "/api/dashboard/stats",
            get(handlers::dashboard::get_dashboard_stats).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/reward/trends",
            get(handlers::dashboard::get_reward_trends).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/reward/lifetime/{id}",
            get(handlers::dashboard::get_agent_lifetime_reward).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/emergence",
            get(handlers::dashboard::get_emergence).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/health",
            get(handlers::dashboard::get_health).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/agents/offline",
            get(handlers::dashboard::get_offline_agents).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agents/dead",
            get(handlers::dashboard::get_dead_agents).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/agent/{id}",
            get(handlers::dashboard::get_agent_details).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agent/{id}/experiences",
            get(handlers::dashboard::get_agent_experiences).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agents",
            get(handlers::dashboard::get_all_agents).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        // C3：统一世界快照（一次请求取 agents + tick_info + recent_events，
        // 单只读事务隔离读，消除 tick 边界瞬时跨 agent 不一致）
        .route(
            "/api/dashboard/world-snapshot",
            get(handlers::dashboard::get_world_snapshot).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        // C4：地点拓扑图（节点+边，来自 LocationRegistry 内存快照）
        .route(
            "/api/dashboard/locations",
            get(handlers::dashboard::get_locations).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        // C4：对话聚合（最近 speak 动作流，支持 ?limit & ?tick_from）
        .route(
            "/api/dashboard/dialogues",
            get(handlers::dashboard::get_dialogues).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        // C4：死亡时间线（status='dead' 的 agent + 失败战斗动作叙事，支持 ?limit & ?tick_from）
        .route(
            "/api/dashboard/deaths",
            get(handlers::dashboard::get_deaths).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/status-configs",
            get(handlers::dashboard::get_status_configs).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/experiences",
            get(handlers::dashboard::get_experiences).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/agents/cleanup",
            post(handlers::dashboard::cleanup_offline_agents).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        // Chronicle API (群像传记)
        .route(
            "/api/dashboard/chronicles",
            get(handlers::chronicle::list_chronicles).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/chronicles/{id}",
            get(handlers::chronicle::get_chronicle).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/chronicles/generate",
            post(handlers::chronicle::generate_chronicle).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        // LLM Token 统计
        .route(
            "/api/dashboard/chronicles/llm-stats",
            get(handlers::chronicle::get_llm_stats).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        // 异步生成任务进度
        .route(
            "/api/dashboard/chronicles/pending",
            get(handlers::chronicle::get_pending_generations).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        // Agent 每日摘要 API
        .route(
            "/api/dashboard/agent-daily-summaries",
            get(handlers::agent_daily_summaries::list_summaries).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agent-daily-summaries/{agent_id}",
            get(handlers::agent_daily_summaries::get_by_agent).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        // Agent 关系图谱 API（C1 全量快照同步）
        .route(
            "/api/dashboard/agent-relationships",
            get(handlers::agent_relationships::get_all_relationships).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agent-relationships/{agent_id}",
            get(handlers::agent_relationships::get_relationships_by_agent).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        // Config API (List/Get 需要 Read 权限, Update 需要 Write 权限)
        .route(
            "/api/config",
            get(handlers::config_editor::list_configs).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/config/{filename}",
            get(handlers::config_editor::get_config_content)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ))
                .merge(put(handlers::config_editor::update_config_content).layer(
                    axum::middleware::from_fn_with_state(
                        state.clone(),
                        handlers::auth::require_write_token,
                    ),
                )),
        )
        // LLM Config API (独立于通用配置编辑器)
        // 前端 settings.html 经 API.BASE="/api/dashboard" 发请求，
        // 故路由需带 /dashboard 前缀以与其它 dashboard 接口一致。
        .route(
            "/api/dashboard/config/llm",
            get(handlers::config_llm::get_llm_config)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ))
                .merge(post(handlers::config_llm::save_llm_config).layer(
                    axum::middleware::from_fn_with_state(
                        state.clone(),
                        handlers::auth::require_write_token,
                    ),
                )),
        )
        // LLM Status & Enabled API
        .route(
            "/api/dashboard/config/llm/status",
            get(handlers::config_llm::get_llm_status).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_client_read_token,
            )),
        )
        .route(
            "/api/dashboard/config/llm/enabled",
            get(handlers::config_llm::get_llm_enabled)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ))
                .merge(post(handlers::config_llm::set_llm_enabled).layer(
                    axum::middleware::from_fn_with_state(
                        state.clone(),
                        handlers::auth::require_write_token,
                    ),
                )),
        )
        // Config Reload API (需要 Write 权限)
        .route(
            "/api/admin/reload-config",
            post(handlers::config_reload::reload_config_handler).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        // Admin Auth API (Cookie Session)
        .route("/api/admin/login", post(handlers::admin_auth::login))
        .route("/api/admin/logout", post(handlers::admin_auth::logout))
        .route(
            "/api/admin/session",
            get(handlers::admin_auth::check_session),
        )
        // Admin Static Files (no auth - login page must be accessible without token)
        // Auth is enforced client-side: frontend stores token in localStorage,
        // sends it via Bearer header on API calls. API routes have their own middleware.
        .route("/admin/", get(serve_admin_index))
        .route("/admin/{*path}", get(serve_admin))
        // Redirect /admin to /admin/
        .route(
            "/admin",
            get(|| async { axum::response::Redirect::temporary("/admin/") }),
        )
        // Action Evolution — 管理面板统计
        .route(
            "/api/dashboard/action-evolution/stats",
            get(handlers::dashboard::get_action_evolution_stats).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        // Action Evolution — 提案组列表（支持 status 过滤）
        .route(
            "/api/dashboard/action-evolution/groups",
            get(handlers::dashboard::get_proposal_groups).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        // Action Evolution — 提案组详情
        .route(
            "/api/dashboard/action-evolution/groups/{id}",
            get(handlers::dashboard::get_proposal_group_detail).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        // Action Evolution — 管理员审批/驳回提案组
        .route(
            "/api/dashboard/action-evolution/groups/{id}/action",
            post(handlers::dashboard::admin_action_on_group).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        // Telemetry API — 行为遥测聚合查询
        .route(
            "/api/dashboard/telemetry",
            get(handlers::dashboard::list_telemetry_aggregations).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/telemetry/{aggregation_name}",
            get(handlers::dashboard::get_telemetry_aggregation).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        // Action Evolution — 治理提案提交（Agent 设备认证）
        .route(
            "/api/v1/action-evolution/propose",
            post(cyber_jianghu_server::governance::handlers::submit_proposal).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_device_token,
                ),
            ),
        )
        // Training export — scheduled/manual 共用 scheduler, read/write 权限按 method 隔离。
        .route(
            "/api/v1/training/export",
            post(handlers::training_export_handler::trigger_export).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        .route(
            "/api/v1/training/exports",
            get(handlers::training_export_handler::list_exports).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/v1/training/exports/{run_id}",
            get(handlers::training_export_handler::get_export)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ))
                .merge(
                    delete(handlers::training_export_handler::delete_export).layer(
                        axum::middleware::from_fn_with_state(
                            state.clone(),
                            handlers::auth::require_write_token,
                        ),
                    ),
                ),
        )
        .route(
            "/api/v1/training/exports/{run_id}/download",
            get(handlers::training_export_handler::download_export).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/v1/training/checkpoint",
            get(handlers::training_export_handler::get_checkpoint).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .with_state(state)
}
