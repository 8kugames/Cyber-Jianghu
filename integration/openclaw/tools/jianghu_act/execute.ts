// tools/jianghu_act/execute.ts
// ============================================================================
// Game Action Execution - Simple interface for LLM to submit actions
// ============================================================================
//
// 数据驱动设计：
// - 使用通用 HTTP 客户端，不定义具体接口
// - 所有接口以 crates/agent 实际披露的为准

import { getHttpClientAsync } from "./http-client.js";
import type { Intent } from "./types.js";

/**
 * Game action parameter types
 */
export interface GameActionParams {
	action: string;
	target?: string;
	data?: string;
	reasoning?: string;
}

/**
 * Build a partial intent from the tool parameters
 *
 * This is used by the enforcement hook to build the complete intent
 * with agent_id and tick_id from the current context.
 */
export function buildIntent(
	params: GameActionParams,
): Omit<Intent, "agent_id" | "tick_id"> {
	const intent: Omit<Intent, "agent_id" | "tick_id"> = {
		action_type: params.action,
		priority: 5,
	};

	if (params.reasoning) {
		intent.thought_log = params.reasoning;
	}

	if (params.data) {
		intent.action_data = { content: params.data };
	}

	if (params.target) {
		if (!intent.action_data) {
			intent.action_data = {};
		}
		(intent.action_data as Record<string, unknown>).target_id = params.target;
	}

	return intent;
}

/**
 * Submit an intent directly to the agent HTTP API
 *
 * POST /api/v1/intent
 */
export async function submitIntentToServer(intent: Intent): Promise<void> {
	const httpClient = await getHttpClientAsync(0);
	await httpClient.post("/api/v1/intent", intent);
	console.log(`[jianghu_act] Submitted: ${intent.action_type}`);
}

/**
 * Validate an intent using the agent HTTP API
 *
 * POST /api/v1/validate
 */
export async function validateIntent(request: {
	intent: Intent;
	persona: {
		gender: string;
		age: number;
		personality: string[];
		values: string[];
	};
	world_context: string;
}): Promise<{
	valid: boolean;
	reason?: string;
	rejection_type?: string;
	narrative?: string;
}> {
	const httpClient = await getHttpClientAsync(0);
	return await httpClient.post("/api/v1/validate", {
		action_type: request.intent.action_type,
		agent_id: request.intent.agent_id,
		tick_id: request.intent.tick_id,
		action_data: request.intent.action_data,
		persona_gender: request.persona.gender,
		persona_age: request.persona.age,
		persona_personality: request.persona.personality,
		persona_values: request.persona.values,
		world_context: request.world_context,
	});
}
