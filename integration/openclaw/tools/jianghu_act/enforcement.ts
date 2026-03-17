// integration/openclaw/tools/jianghu_act/enforcement.ts
// ============================================================================
// Agent End Hook Handler - Ensures jianghu_act is called every tick
// ============================================================================
//
// 数据驱动设计：
// - 使用通用 HTTP 客户端，不定义具体接口
// - 所有接口以 crates/agent 实际披露的为准
//
// 架构说明：
// - register.ts 中的 jianghu_act 工具只记录意图
// - 这个 hook 负责实际的验证、提交和记忆归档

import { getHttpClientAsync } from "./http-client.js";
import { executeWithRetry } from "./retry-handler.js";
import type { GameActionParams, PersonaInfo, WorldState } from "./types.js";
import { DEFAULT_RETRY_CONFIG } from "./types.js";

/**
 * Run enforcement logic after agent completes
 *
 * This is called by the agent_end plugin hook.
 */
export async function runEnforcement(
	_event: { messages?: unknown[]; runId?: string; [key: string]: unknown },
	context: {
		toolCalls?: Array<{ name: string; arguments: Record<string, unknown> }>;
		tickId?: number;
		agentId?: string;
		localApiPort?: number;
		lastAssistantMessage?: string;
		// 由 register.ts 传递
		lastGameActionCall?: {
			action: string;
			target?: string;
			data?: string;
			reasoning?: string;
		} | null;
		worldState?: WorldState;
		persona?: PersonaInfo;
		[key: string]: unknown;
	},
): Promise<void> {
	// 优先使用 register.ts 传递的 lastGameActionCall
	// 如果没有（例如直接调用），则从 toolCalls 中查找
	const gameActionCall = context.lastGameActionCall ||
		context.toolCalls?.find((tc) => tc.name === "jianghu_act")?.arguments;

	const gameActionCalled = !!gameActionCall;

	if (!gameActionCalled || !gameActionCall) {
		console.warn("[enforcement] LLM did not call jianghu_act, submitting idle action");
		await submitIdleAction(context);
		return;
	}

	// 提取参数
	const params = gameActionCall as GameActionParams;

	// 执行动作（带重试）
	try {
		const httpClient = await getHttpClientAsync(context.localApiPort || 0);

		// 构建 context
		const executeContext = {
			httpClient,
			agentId: context.agentId || "unknown",
			tickId: context.tickId || 0,
			worldState: context.worldState || null,
			persona: context.persona || {
				gender: "男",
				age: 28,
				personality: ["沉稳", "重情义"],
				values: ["江湖道义为先"],
			},
		};

		const result = await executeWithRetry(params, executeContext, DEFAULT_RETRY_CONFIG);

		if (!result.success) {
			console.warn(`[enforcement] Action failed: ${result.error}`);
		}

		// 归档决策到记忆
		await archiveDecision(context, params, gameActionCalled);
	} catch (error) {
		console.error("[enforcement] Failed to execute action:", error);
		// 即使失败也尝试归档
		await archiveDecision(context, params, gameActionCalled);
	}
}

/**
 * Submit idle action when LLM fails to call jianghu_act
 */
async function submitIdleAction(
	context: { tickId?: number; agentId?: string; localApiPort?: number },
): Promise<void> {
	try {
		const httpClient = await getHttpClientAsync(context.localApiPort || 0);

		const idleIntent = {
			agent_id: context.agentId || "unknown",
			tick_id: context.tickId || 0,
			action_type: "idle",
		};

		// POST /api/v1/intent
		await httpClient.post("/api/v1/intent", idleIntent);
	} catch (error) {
		console.error("[enforcement] Failed to submit idle action:", error);
	}
}

/**
 * Archive decision to memory
 */
async function archiveDecision(
	context: { tickId?: number; lastAssistantMessage?: string; localApiPort?: number },
	action: { action: string; target?: string; data?: string; reasoning?: string },
	gameActionCalled: boolean,
): Promise<void> {
	try {
		const httpClient = await getHttpClientAsync(context.localApiPort || 0);

		const decision = {
			tick: context.tickId,
			action: {
				action: action.action,
				target: action.target,
				data: action.data,
			},
			reasoning: action.reasoning || context.lastAssistantMessage,
			jianghu_act_called: gameActionCalled,
		};

		// POST /api/v1/memory
		await httpClient.post("/api/v1/memory", {
			content: JSON.stringify(decision),
			importance: calculateImportance(decision),
			metadata: {
				type: "decision",
				tick: decision.tick,
				action: decision.action?.action,
			},
		});
	} catch (error) {
		console.error("[enforcement] Failed to archive decision:", error);
	}
}

/**
 * Calculate importance score for a decision
 */
function calculateImportance(decision: {
	action?: { action: string } | null;
	reasoning?: string;
}): number {
	let importance = 0.5;

	if (decision.action?.action === "attack") importance += 0.3;
	if (decision.action?.action === "use" || decision.action?.action === "pickup") importance += 0.1;
	if (decision.reasoning && decision.reasoning.length > 50) importance += 0.1;

	return Math.min(importance, 1.0);
}
