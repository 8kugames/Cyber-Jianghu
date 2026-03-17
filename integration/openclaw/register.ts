// register.ts - Cyber-Jianghu OpenClaw Plugin Entry Point
// ============================================================================
// This file is the main entry point for the Cyber-Jianghu plugin.
// OpenClaw calls the register(api) function when the plugin is loaded.
//
// 架构说明：
// - jianghu_act 工具在这里注册，执行时只记录意图
// - agent_end hook 负责实际的验证、提交和执行
// - 这样可以集中处理重试、验证和记忆归档逻辑
// ============================================================================

/**
 * Plugin API type (minimal definition for type safety)
 */
interface PluginAPI {
	registerTool(params: ToolDefinition): void;
	on(
		event: string,
		handler: (event: any, context: any) => any | Promise<any>,
		options?: any,
	): void;
	config?: Record<string, unknown>;
}

interface ToolDefinition {
	name: string;
	description: string;
	parameters: {
		type: string;
		properties: Record<
			string,
			{
				type: string;
				description: string;
			}
		>;
		required: string[];
	};
	execute: (id: string, params: Record<string, unknown>) => Promise<ToolResult>;
}

interface ToolResult {
	content: Array<{ type: string; text: string }>;
	isError?: boolean;
}

// Store the last jianghu_act call for the enforcement hook
let lastGameActionCall: { action: string; target?: string; data?: string; reasoning?: string } | null = null;

/**
 * Plugin entry point - called by OpenClaw when the plugin is loaded
 */
export default function register(api: PluginAPI) {
	// Register jianghu_act tool
	//
	// 工具执行时只记录意图，实际的验证和提交由 agent_end hook 处理
	// 这样可以集中处理验证逻辑、重试机制和记忆归档
	api.registerTool({
		name: "jianghu_act",
		description:
			"提交游戏动作到赛博江湖世界。你必须每个 Tick 调用这个工具。可用动作请参考 CONTEXT.md 中的 available_actions 字段。",
		parameters: {
			type: "object",
			properties: {
				action: {
					type: "string",
					description:
						"动作类型（从 CONTEXT.md 的 available_actions 中选择）",
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
		execute: async (_id, params) => {
			// 存储工具调用供 enforcement hook 使用
			lastGameActionCall = params as {
				action: string;
				target?: string;
				data?: string;
				reasoning?: string;
			};

			console.log(
				`[jianghu_act] Intent recorded: ${lastGameActionCall.action} ${lastGameActionCall.target || ""} ${lastGameActionCall.data || ""} (${lastGameActionCall.reasoning || ""})`,
			);

			return {
				content: [
					{
						type: "text",
						text: `动作已记录: ${lastGameActionCall.action}`,
					},
				],
			};
		},
	});

	// Register agent_end lifecycle hook (plugin hook, not internal hook)
	//
	// 这个 hook 在每次 agent 决策周期后运行
	// 它确保 jianghu_act 被调用，并将意图提交到游戏服务器
	api.on("agent_end", async (event, context) => {
		// 将存储的工具调用传递给 enforcement handler
		const enrichedContext = {
			...context,
			lastGameActionCall,
		};

		const { runEnforcement } = await import("./tools/jianghu_act/enforcement.js");
		await runEnforcement(event, enrichedContext);

		// 重置状态
		lastGameActionCall = null;
	});

	console.log("[cyber-jianghu] Plugin registered successfully");
}
