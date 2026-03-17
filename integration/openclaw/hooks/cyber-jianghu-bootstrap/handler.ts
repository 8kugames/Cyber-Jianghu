// cyber-jianghu-bootstrap/handler.ts
// ============================================================================
// Bootstrap Hook Handler - Fetches WorldState and generates CONTEXT.md
// ============================================================================
//
// 数据驱动设计：
// - 使用通用 HTTP 客户端，// - 所有接口以 crates/agent 实际披露的为准

import { getHttpClientAsync } from "../../tools/jianghu_act/http-client";

/**
 * Hook event type (compatible with OpenClaw internal hooks)
 */
type HookEvent = {
	type: "agent";
	action: "bootstrap";
	context: {
		workspaceDir: string;
		cfg?: any;
		sessionKey?: string;
		sessionId?: string;
		agentId?: string;
		workspace?: {
			writeFile: (path: string, content: string) => Promise<void>;
			[key: string]: unknown;
		};
		[key: string]: unknown;
	};
	timestamp: number;
};

/**
 * Add decision hints to the context
 */
function addDecisionHints(context: string): string {
	const hints = `
## Decision Hints

Based on the above status, choose an appropriate action and submit it using the \`jianghu_act\` tool.

**CRITICAL**: You must call the jianghu_act tool to submit your action. No exceptions.
`;
	return context + hints;
}

/**
 * Bootstrap hook handler
 *
 * This function is called on agent bootstrap or cron tick.
 * It fetches the formatted context from the agent HTTP API and writes it to CONTEXT.md.
 */
const handler = async (event: HookEvent): Promise<void> => {
	const context = event.context || {};
	const workspaceDir = context.workspaceDir;

	if (!workspaceDir) {
		console.warn("[bootstrap] No workspaceDir in context, skipping");
		return;
	}

	try {
		// Discover the agent HTTP API port
		const client = await getHttpClientAsync(0);

		// Check if connected (GET /api/v1/health)
		let isHealthy = false;
		try {
			const health = await client.get<{ status: string }>("/api/v1/health");
			isHealthy = health.status === "ok";
		} catch {
			isHealthy = false;
		}

		if (!isHealthy) {
			console.warn(
				"[bootstrap] Agent HTTP API not reachable. Make sure cyber-jianghu-agent is running.",
			);
			return;
		}

		// Get formatted context from agent HTTP API (GET /api/v1/context)
		const response = await client.get<{
			context: string;
			tick_id: number;
			agent_id: string;
		}>("/api/v1/context");

		console.log(`[bootstrap] New Tick: ${response.tick_id}`);

		// Add decision hints
		const contextMd = addDecisionHints(response.context);

		// Write to workspace using the workspace API
		// Note: OpenClaw provides workspace.writeFile through context
		if (
			context.workspace &&
			typeof context.workspace.writeFile === "function"
		) {
			await context.workspace.writeFile("CONTEXT.md", contextMd);
		} else {
			// Fallback: write directly to file system
			const fs = await import("fs/promises");
			const path = await import("path");
			const contextPath = path.join(workspaceDir, "CONTEXT.md");
			await fs.writeFile(contextPath, contextMd, "utf-8");
		}

		console.log(`[bootstrap] CONTEXT.md updated for tick ${response.tick_id}`);
	} catch (error) {
		console.error("[bootstrap] Failed:", error);
		// Don't throw - the previous CONTEXT.md will be used if available
	}
};

export default handler;
