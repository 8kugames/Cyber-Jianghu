// HTTP helpers: fetch wrappers, all endpoint constants

const API_BASE = `${window.location.protocol}//${window.location.host}`;

// 所有 API 路径集中定义
export const API = {
    // Setup & Status
    SETUP_STATUS: '/api/v1/setup/status',
    STATE: '/api/v1/state',
    TICK: '/api/v1/tick',

    // Character
    CHARACTER: '/api/v1/character',
    CHARACTERS: '/api/v1/characters',
    CHARACTERS_SWITCH: '/api/v1/characters/switch',
    CHARACTER_GENERATE: '/api/v1/character/generate',
    CHARACTER_REGISTER: '/api/v1/character/register',
    CHARACTER_REBIRTH: '/api/v1/character/rebirth',

    // Attributes
    ATTRIBUTES: '/api/v1/attributes',
    ATTRIBUTE_META: '/api/v1/attribute-meta',

    // Biography
    BIOGRAPHY: '/api/v1/character/biography',

    // Soul Cycles
    SOUL_CYCLES: '/api/v1/character/soul-cycles',

    // Memory
    MEMORY_RECENT: '/api/v1/memory/recent',
    MEMORY_SEARCH: '/api/v1/memory/search',
    MEMORY: '/api/v1/memory',

    // Dream
    DREAM: '/api/v1/character/dream',
    DREAM_RECORDS: '/api/v1/character/dream/records',

    // Relationships
    RELATIONSHIP_LIST: '/api/v1/relationship/list',

    // Cognitive
    COGNITIVE: '/api/v1/cognitive',

    // Events (SSE)
    EVENTS: '/api/v1/events',

    // Config
    CONFIG_LLM: '/api/v1/config/llm',
    CONFIG_LLM_PROVIDERS: '/api/v1/config/llm/providers',
    CONFIG_LLM_USAGE: '/api/v1/config/llm/usage',
    CONFIG_LLM_DISABLED: '/api/v1/config/llm-disabled',
    CONFIG_SERVER: '/api/v1/config/server',
    CONFIG_AUTO_REBIRTH: '/api/v1/config/auto-rebirth',
    CONFIG_RELOAD: '/api/v1/config/reload',

    // Validation
    VALIDATE: '/api/v1/validate',

    // Metrics
    METRICS: '/api/v1/metrics',

    // Actions
    ACTIONS: '/api/v1/actions',

    // Context
    CONTEXT: '/api/v1/context',

    // Lifespan
    LIFESPAN: '/api/v1/lifespan',
};

const DEFAULT_TIMEOUT_MS = 10000;
const MAX_RETRIES = 2;

function isNetworkError(error) {
    return error.message.includes('网络错误') ||
        error.message.includes('Failed to fetch') ||
        error.message.includes('NetworkError') ||
        error.message.includes('请求超时');
}

async function fetchWithTimeout(url, options = {}, timeoutMs = DEFAULT_TIMEOUT_MS) {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), timeoutMs);
    try {
        const response = await fetch(url, { ...options, signal: controller.signal });
        clearTimeout(timeoutId);
        return response;
    } catch (error) {
        clearTimeout(timeoutId);
        if (error.name === 'AbortError') throw new Error(`请求超时（${timeoutMs}ms）`);
        throw error;
    }
}

async function fetchWithRetry(url, options = {}, retries = MAX_RETRIES, timeoutMs = DEFAULT_TIMEOUT_MS) {
    try {
        return await fetchWithTimeout(url, options, timeoutMs);
    } catch (error) {
        if (retries > 0 && isNetworkError(error)) {
            await new Promise(r => setTimeout(r, (MAX_RETRIES - retries + 1) * 1000));
            return fetchWithRetry(url, options, retries - 1, timeoutMs);
        }
        throw error;
    }
}

async function parseResponse(response) {
    const text = await response.text();
    if (!text) return null;
    try {
        return JSON.parse(text);
    } catch (_) {
        return { message: text };
    }
}

export async function get(path, options = {}) {
    const timeout = options.timeout ?? DEFAULT_TIMEOUT_MS;
    const retries = options.retries ?? MAX_RETRIES;
    const response = await fetchWithRetry(`${API_BASE}${path}`, {
        method: 'GET',
        headers: { 'Content-Type': 'application/json' },
    }, retries, timeout);
    const data = await parseResponse(response);
    if (!response.ok) throw new Error(data?.message || data?.error || `服务器错误: ${response.status}`);
    return data ?? {};
}

export async function post(path, body, options = {}) {
    const timeout = options.timeout ?? DEFAULT_TIMEOUT_MS;
    const retries = options.retries ?? MAX_RETRIES;
    const response = await fetchWithRetry(`${API_BASE}${path}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
    }, retries, timeout);
    const data = await parseResponse(response);
    if (!response.ok) throw new Error(data?.message || data?.error || `服务器错误: ${response.status}`);
    return data ?? {};
}
