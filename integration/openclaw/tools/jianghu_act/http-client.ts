// integration/openclaw/tools/jianghu_act/http-client.ts
// ============================================================================
// Generic HTTP Client for crates/agent Local API
// ============================================================================
//
// 设计原则：
// - 不定义具体接口，只提供通用 HTTP 方法
// - 所有接口以 crates/agent 实际披露的为准
// - 端口范围: 23340-23349 (避免与服务器端口 23333 冲突)

/** Agent HTTP API 端口范围 */
export const PORT_RANGE = { min: 23340, max: 23349 };

/**
 * 自动发现 Agent HTTP API 端口
 */
export async function discoverPort(
	host: string = "127.0.0.1",
): Promise<number | null> {
	for (let port = PORT_RANGE.min; port <= PORT_RANGE.max; port++) {
		try {
			const response = await fetch(`http://${host}:${port}/api/v1/health`, {
				method: "GET",
				signal: AbortSignal.timeout(500),
			});
			if (response.ok) {
				const data = await response.json();
				if (data.status === "ok") {
					console.log(`[http-client] Discovered agent at port ${port}`);
					return port;
				}
			}
		} catch {
			// Port not available, continue scanning
		}
	}
	return null;
}

/**
 * Generic HTTP Client for crates/agent API
 *
 * 提供通用的 HTTP 方法，不定义具体的接口。
 * 调用者需要根据 crates/agent 的实际 API 使用。
 */
export class HttpClient {
	private baseUrl: string;
	private timeoutMs: number;

	constructor(
		port: number,
		host: string = "127.0.0.1",
		timeoutMs: number = 5000,
	) {
		this.baseUrl = `http://${host}:${port}`;
		this.timeoutMs = timeoutMs;
	}

	/**
	 * Make HTTP GET request
	 */
	async get<T = unknown>(path: string): Promise<T> {
		const controller = new AbortController();
		const timeoutId = setTimeout(() => controller.abort(), this.timeoutMs);

		try {
			const response = await fetch(`${this.baseUrl}${path}`, {
				method: "GET",
				headers: { Accept: "application/json" },
				signal: controller.signal,
			});

			if (!response.ok) {
				throw new Error(`HTTP ${response.status}: ${response.statusText}`);
			}

			return await response.json();
		} finally {
			clearTimeout(timeoutId);
		}
	}

	/**
	 * Make HTTP POST request
	 */
	async post<T = unknown>(path: string, body: unknown): Promise<T> {
		const controller = new AbortController();
		const timeoutId = setTimeout(() => controller.abort(), this.timeoutMs);

		try {
			const response = await fetch(`${this.baseUrl}${path}`, {
				method: "POST",
				headers: {
					"Content-Type": "application/json",
					Accept: "application/json",
				},
				body: JSON.stringify(body),
				signal: controller.signal,
			});

			if (!response.ok) {
				const errorText = await response.text();
				throw new Error(`HTTP ${response.status}: ${errorText}`);
			}

			// Handle empty response
			const text = await response.text();
			if (!text) {
				return {} as T;
			}
			return JSON.parse(text) as T;
		} finally {
			clearTimeout(timeoutId);
		}
	}

	/**
	 * Get base URL for debugging
	 */
	getBaseUrl(): string {
		return this.baseUrl;
	}
}

// Global HTTP client cache
const clientCache: Map<string, HttpClient> = new Map();
let discoveredPort: number | null = null;

/**
 * Get or create HTTP client
 *
 * 如果 port 为 0，会自动在 23340-23349 范围内发现可用端口
 */
export async function getHttpClientAsync(
	port: number = 0,
	host: string = "127.0.0.1",
): Promise<HttpClient> {
	// 如果 port 为 0，自动发现端口
	if (port === 0) {
		if (discoveredPort === null) {
			discoveredPort = await discoverPort(host);
			if (discoveredPort === null) {
				throw new Error("No agent HTTP API found in port range 23340-23349");
			}
		}
		port = discoveredPort;
	}

	const key = `${host}:${port}`;

	if (!clientCache.has(key)) {
		console.log(`[http-client] Creating new HTTP client for ${key}`);
		clientCache.set(key, new HttpClient(port, host));
	}

	return clientCache.get(key)!;
}

/**
 * Get or create HTTP client (sync version for backward compatibility)
 */
export function getHttpClient(
	port: number,
	host: string = "127.0.0.1",
): HttpClient {
	// 如果 port 为 0，使用默认端口 23340（向后兼容）
	const actualPort = port === 0 ? PORT_RANGE.min : port;
	const key = `${host}:${actualPort}`;

	if (!clientCache.has(key)) {
		console.log(`[http-client] Creating new HTTP client for ${key}`);
		clientCache.set(key, new HttpClient(actualPort, host));
	}

	return clientCache.get(key)!;
}

/**
 * Reset discovered port (useful for testing)
 */
export function resetDiscovery(): void {
	discoveredPort = null;
}
