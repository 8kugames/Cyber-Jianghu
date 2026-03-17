// integration/openclaw/tools/jianghu_act/index.ts
// ============================================================================
// jianghu_act Tool - Core constraint that enforces LLM thinking
// ============================================================================
//
// This tool is the MANDATORY entry point for all game actions.
// OpenClaw agent MUST call this tool to act in the game world.
//
// 数据驱动设计：
// - 动作类型从服务端 available_actions 动态获取
// - 不硬编码动作列表
// - 参数验证由服务端处理
//
// HTTP 客户端：
// - 使用通用 HTTP 方法，不定义具体接口
// - 所有接口以 crates/agent 实际披露的为准
// ============================================================================

import { getHttpClient } from "./http-client";
import { extractPersonaInfo } from "./intent-builder";
import { executeWithRetry } from "./retry-handler";
import type {
	ActionResult,
	GameActionParams,
	PersonaInfo,
	WorldState,
} from "./types";

/**
 * Tool definition for OpenClaw
 *
 * 数据驱动设计：
 * - action 参数不使用 enum，由服务端验证
 * - description 引用 CONTEXT.md 中的 available_actions
 */
export const toolDefinition = {
	name: "jianghu_act",
	description: `提交游戏动作到赛博江湖世界。

⚠️ CRITICAL: 你必须每个 Tick 调用这个工具。没有例外。
如果你在没有调用 jianghu_act 的情况下回复，系统会自动提交一个 idle 动作。

📌 可用动作请参考 CONTEXT.md 中的 \`available_actions\` 字段。
服务端会验证你的动作是否合法。

参数说明:
- action: 动作类型（从 available_actions 中选择）
- target: 目标实体/物品/地点的ID（可选）
- data: 额外数据，如说话内容、物品ID等（可选）
- reasoning: 你的思考过程（强烈建议填写）`,
	parameters: {
		type: "object",
		properties: {
			action: {
				type: "string",
				description: "动作类型（从 CONTEXT.md 的 available_actions 中选择）",
			},
			target: {
				type: "string",
				description: "目标实体/物品/地点的ID (可选)",
			},
			data: {
				type: "string",
				description: "额外数据，如说话内容、物品ID等 (可选)",
			},
			reasoning: {
				type: "string",
				description: "你的思考过程，解释为什么选择这个动作 (强烈建议)",
			},
		},
		required: ["action"],
	},
};

/**
 * Tool handler context
 */
export interface ToolContext {
	config: {
		localApiPort?: number;
		localApiHost?: string;
	};
	worldState?: WorldState;
	tickId?: number;
	agentId?: string;
	persona?: PersonaInfo;
	[key: string]: unknown;
}

/**
 * Main tool handler
 */
export async function gameActionHandler(
	params: GameActionParams,
	context: ToolContext,
): Promise<ActionResult> {
	const port = context.config?.localApiPort || 23340;
	const host = context.config?.localApiHost || "127.0.0.1";

	console.log(`[jianghu_act] Processing action: ${params.action}`);
	console.log(`[jianghu_act] API endpoint: ${host}:${port}`);

	// Get HTTP client
	const httpClient = getHttpClient(port, host);

	// Check connection (GET /api/v1/health)
	try {
		const health = await httpClient.get<{
			status: string;
			agent_id: string;
			tick_id?: number;
		}>("/api/v1/health");
		if (health.status !== "ok") {
			console.error("[jianghu_act] Agent health check failed");
			return {
				success: false,
				error: "游戏客户端状态异常。请检查 cyber-jianghu-agent。",
			};
		}
	} catch (error) {
		console.error(
			"[jianghu_act] Cannot connect to crates/agent HTTP API:",
			error,
		);
		return {
			success: false,
			error: "无法连接到游戏客户端。请确保 cyber-jianghu-agent 正在运行。",
		};
	}

	// Get current state if not provided
	let worldState = context.worldState;
	let tickId = context.tickId || 0;
	let agentId = context.agentId || "unknown";

	if (!worldState) {
		try {
			// GET /api/v1/state
			worldState = await httpClient.get<WorldState>("/api/v1/state");
			if (worldState) {
				tickId = worldState.tick_id;
				agentId = worldState.agent_id || agentId;
				console.log(
					`[jianghu_act] Fetched state: tick=${tickId}, agent=${agentId}`,
				);

				// 数据驱动：记录可用动作
				if (worldState.available_actions?.length) {
					console.log(
						`[jianghu_act] Available actions: ${worldState.available_actions.map((a) => a.action).join(", ")}`,
					);
				}
			}
		} catch (error) {
			console.error("[jianghu_act] Failed to fetch world state:", error);
		}
	}

	// Get persona info
	const persona = context.persona || extractPersonaInfo(context);

	// Execute with retry
	return executeWithRetry(params, {
		httpClient,
		agentId,
		tickId,
		worldState: worldState || null,
		persona,
	});
}

/**
 * Export for OpenClaw tool registration
 */
export default {
	definition: toolDefinition,
	handler: gameActionHandler,
};
