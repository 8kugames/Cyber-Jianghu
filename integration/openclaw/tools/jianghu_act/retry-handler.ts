// integration/openclaw/tools/jianghu_act/retry-handler.ts
// ============================================================================
// Retry Handler - Retry logic and fallback mechanisms
// ============================================================================
//
// 数据驱动设计：
// - 使用通用 HTTP 客户端，不定义具体接口
// - 所有 API 调用以 crates/agent 实际披露的为准

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import type { HttpClient } from "./http-client";
import { buildIntentFromParams } from "./intent-builder";
import type {
	ActionResult,
	GameActionParams,
	Intent,
	PersonaInfo,
	RetryConfig,
	WorldState,
} from "./types";
import { DEFAULT_RETRY_CONFIG } from "./types";

/**
 * Calculate backoff delay with exponential backoff
 */
function calculateBackoff(attempt: number, config: RetryConfig): number {
	const delay = config.baseDelayMs * config.backoffMultiplier ** attempt;
	return Math.min(delay, config.maxDelayMs);
}

/**
 * Sleep for specified milliseconds
 */
function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Execute game action with retry logic
 */
export async function executeWithRetry(
	params: GameActionParams,
	context: {
		httpClient: HttpClient;
		agentId: string;
		tickId: number;
		worldState: WorldState | null;
		persona: PersonaInfo;
	},
	config: RetryConfig = DEFAULT_RETRY_CONFIG,
): Promise<ActionResult> {
	let lastError: string | null = null;

	for (let attempt = 0; attempt <= config.maxRetries; attempt++) {
		try {
			const result = await executeGameAction(params, context);

			if (result.success) {
				return result;
			}

			// Validation failed - prepare for retry
			lastError = result.error || "Unknown validation error";

			console.warn(
				`[retry-handler] Attempt ${attempt + 1} failed: ${lastError}`,
			);

			// Wait before retry (except on last attempt)
			if (attempt < config.maxRetries) {
				const delay = calculateBackoff(attempt, config);
				console.log(`[retry-handler] Waiting ${delay}ms before retry...`);
				await sleep(delay);
			}
		} catch (error) {
			lastError = (error as Error).message;
			console.error(
				`[retry-handler] Attempt ${attempt + 1} threw error:`,
				error,
			);

			if (attempt < config.maxRetries) {
				await sleep(calculateBackoff(attempt, config));
			}
		}
	}

	// All retries exhausted - apply fallback strategy
	console.warn(
		`[retry-handler] All ${config.maxRetries + 1} attempts failed, applying fallback`,
	);

	return handleFallback(params, context, lastError, config);
}

/**
 * Execute a single game action (validation + submission)
 *
 * 使用通用 HTTP 客户端，直接调用 crates/agent API
 */
async function executeGameAction(
	params: GameActionParams,
	context: {
		httpClient: HttpClient;
		agentId: string;
		tickId: number;
		worldState: WorldState | null;
		persona: PersonaInfo;
	},
): Promise<ActionResult> {
	const { httpClient, agentId, tickId, worldState, persona } = context;

	// 1. Build Intent
	const intent = buildIntentFromParams(params, agentId, tickId);

	// 2. Build validation request
	// 使用服务端提供的 context（如果有），否则使用简单的 context
	const worldContextStr = worldState
		? JSON.stringify({
				tick_id: worldState.tick_id,
				location: worldState.location?.name,
				attributes: worldState.self_state?.attributes,
			})
		: "{}";

	// 3. Call validation endpoint (POST /api/v1/validate)
	const validateResponse = await httpClient.post<{
		valid: boolean;
		reason?: string;
		rejection_type?: string;
		narrative?: string;
	}>("/api/v1/validate", {
		action_type: intent.action_type,
		agent_id: intent.agent_id,
		tick_id: intent.tick_id,
		action_data: intent.action_data,
		persona_gender: persona.gender,
		persona_age: persona.age,
		persona_personality: persona.personality,
		persona_values: persona.values,
		world_context: worldContextStr,
	});

	// 4. Handle validation result
	if (!validateResponse.valid) {
		return {
			success: false,
			error: validateResponse.reason,
			rejection_type: validateResponse.rejection_type,
			hint: generateRetryHint(validateResponse),
		};
	}

	// 5. Validation approved - submit intent (POST /api/v1/intent)
	await httpClient.post("/api/v1/intent", intent);

	return {
		success: true,
		narrative: validateResponse.narrative,
	};
}

/**
 * Generate retry hint based on rejection type
 */
function generateRetryHint(response: {
	reason?: string;
	rejection_type?: string;
}): string {
	if (response.rejection_type) {
		return `验证失败: ${response.rejection_type}。原因: ${response.reason || "未知"}`;
	}
	return response.reason || "请重新考虑你的行动";
}

/**
 * Handle fallback when all retries fail
 */
async function handleFallback(
	originalParams: GameActionParams,
	context: {
		httpClient: HttpClient;
		agentId: string;
		tickId: number;
		worldState: WorldState | null;
		persona: PersonaInfo;
	},
	lastError: string | null,
	_config: RetryConfig,
): Promise<ActionResult> {
	// Log incident
	logIncident("fallback_triggered", {
		original_action: originalParams.action,
		error: lastError,
		tick: context.tickId,
	});

	console.log(
		`[retry-handler] All retries failed, returning idle for LLM to decide next action`,
	);

	// 提交 idle 动作
	const idleIntent = buildIntentFromParams(
		{ action: "idle", reasoning: `重试失败: ${lastError}` },
		context.agentId,
		context.tickId,
	);

	try {
		await context.httpClient.post("/api/v1/intent", idleIntent);
	} catch (error) {
		console.error("[retry-handler] Idle submission failed:", error);
	}

	return {
		success: true,
		narrative: `验证多次失败，已执行 idle。请根据 CONTEXT.md 重新决策。`,
	};
}

/**
 * Log incident for analysis
 */
function logIncident(type: string, data: Record<string, unknown>): void {
	const incident = {
		type,
		timestamp: new Date().toISOString(),
		...data,
	};

	console.log(`[incident] ${JSON.stringify(incident)}`);

	try {
		const logDir = path.join(os.homedir(), ".openclaw", "logs");

		if (!fs.existsSync(logDir)) {
			fs.mkdirSync(logDir, { recursive: true });
		}
		const logFile = path.join(logDir, "incidents.ndjson");
		fs.appendFileSync(logFile, JSON.stringify(incident) + "\n");
	} catch (e) {
		console.error("[retry-handler] Failed to write incident log:", e);
	}
}
