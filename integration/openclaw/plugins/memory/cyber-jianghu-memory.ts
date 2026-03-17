// integration/openclaw/plugins/memory/cyber-jianghu-memory.ts
// ============================================================================
// Cyber-Jianghu Memory Backend - Bridges to crates/agent HTTP API
// ============================================================================
//
// 数据驱动设计：
// - 使用通用 HTTP 方法调用 API
// - 所有接口以 crates/agent 实际披露的为准

import { getHttpClientAsync } from "../../tools/jianghu_act/http-client";
import type { MemoryEntry } from "../../tools/jianghu_act/types";

/**
 * Memory Backend implementation for OpenClaw
 * Bridges to crates/agent HTTP API memory endpoints
 */
export class CyberJianghuMemoryBackend {
	private port: number;
	private host: string;

	constructor(port: number = 0, host: string = "127.0.0.1") {
		this.port = port;
		this.host = host;
	}

	/**
	 * Search memories by query
	 * POST /api/v1/memory/search
	 */
	async search(query: string, limit: number = 10): Promise<MemoryEntry[]> {
		try {
			const client = await getHttpClientAsync(this.port, this.host);
			const response = await client.post<{
				memories: MemoryEntry[];
				count: number;
				query: string;
			}>("/api/v1/memory/search", { query, limit });
			return response.memories;
		} catch (error) {
			console.error("[memory-backend] Search failed:", error);
			return [];
		}
	}

	/**
	 * Get memory by path
	 */
	async get(path: string): Promise<string> {
		try {
			const client = await getHttpClientAsync(this.port, this.host);
			// Path format: type/id or just id
			const parts = path.split("/");
			const id = parts.length > 1 ? parts[1] : parts[0];

			// For now, return from recent memories
			// TODO: Add specific get endpoint to crates/agent
			const response = await client.get<{
				memories: MemoryEntry[];
				count: number;
			}>(`/api/v1/memory/recent?limit=100`);

			const found = response.memories.find(
				(m) => m.tick_id?.toString() === id,
			);
			return found?.content || "";
		} catch (error) {
			console.error("[memory-backend] Get failed:", error);
			return "";
		}
	}

	/**
	 * Archive new memory
	 * POST /api/v1/memory
	 */
	async archive(content: string, metadata: Record<string, unknown>): Promise<void> {
		try {
			const client = await getHttpClientAsync(this.port, this.host);
			await client.post("/api/v1/memory", {
				content,
				importance: (metadata.importance as number) || 0.5,
			});
		} catch (error) {
			console.error("[memory-backend] Archive failed:", error);
		}
	}

	/**
	 * Get recent memories
	 * GET /api/v1/memory/recent
	 */
	async getRecent(count: number = 10): Promise<MemoryEntry[]> {
		try {
			const client = await getHttpClientAsync(this.port, this.host);
			const response = await client.get<{
				memories: MemoryEntry[];
				count: number;
			}>(`/api/v1/memory/recent?limit=${count}`);
			return response.memories;
		} catch (error) {
			console.error("[memory-backend] GetRecent failed:", error);
			return [];
		}
	}

	/**
	 * Forget memories (not directly supported)
	 */
	async forget(_criteria: unknown): Promise<number> {
		console.warn(
			"[memory-backend] Forget not directly supported - use crates/agent lifecycle",
		);
		return 0;
	}
}

export default CyberJianghuMemoryBackend;
