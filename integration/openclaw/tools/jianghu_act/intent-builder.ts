// integration/openclaw/tools/jianghu_act/intent-builder.ts
// ============================================================================
// Intent Builder - Build Intent from jianghu_act params
// ============================================================================
//
// 数据驱动设计：
// - 客户端直接提供 action_data，不进行服务端业务逻辑
// - 属性从 WorldState.attributes 动态获取，不硬编码属性名
// - 移除 generateSafeAction：游戏逻辑应在服务端

import type {
	GameActionParams,
	Intent,
	PersonaInfo,
	WorldState,
} from "./types";

/**
 * Build Intent from jianghu_act tool parameters
 *
 * 数据驱动设计：直接透传客户端提供的参数
 */
export function buildIntentFromParams(
	params: GameActionParams,
	agentId: string,
	tickId: number,
): Intent {
	const intent: Intent = {
		agent_id: agentId,
		tick_id: tickId,
		action_type: params.action, // 直接使用客户端提供的 action type
		priority: 5,
	};

	// Add thought log if reasoning provided
	if (params.reasoning) {
		intent.thought_log = params.reasoning;
	}

	// 数据驱动：直接使用 action_data（如果提供）
	// 注意：action_data 应该由 LLM 根据游戏状态构建
	// 服务端不再负责构建 action_data
	if (params.data) {
		intent.action_data = { content: params.data };
	}

	// 如果有 target，添加到 action_data
	if (params.target) {
		if (!intent.action_data) {
			intent.action_data = {};
		}
		(intent.action_data as Record<string, unknown>).target_id = params.target;
	}

	return intent;
}

/**
 * Extract persona info from config/context
 */
export function extractPersonaInfo(context: unknown): PersonaInfo {
	// Try to get from context.config.persona first
	const config = (context as { config?: { persona?: Partial<PersonaInfo> } })
		?.config;

	if (config?.persona) {
		const personaConfig = config.persona;
		return {
			gender: personaConfig.gender || "男",
			age: personaConfig.age || 28,
			personality: personaConfig.personality || ["沉稳"],
			values: personaConfig.values || ["江湖道义为先"],
		};
	}

	// Fallback to defaults
	return {
		gender: "男",
		age: 28,
		personality: ["沉稳", "重情义"],
		values: ["江湖道义为先"],
	};
}
