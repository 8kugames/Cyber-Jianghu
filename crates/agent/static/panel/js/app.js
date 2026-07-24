// Entry: router init, global state, SSE connection

import * as router from './router.js';
import { API, get, refreshAuthToken, getStoredAuthToken } from './api.js';

// Page modules (loaded on demand)
import { dashboardPage } from './dashboard.js';
import { characterPage } from './character.js';
import { settingsPage } from './settings.js';

// Global SSE connection
let eventSource = null;
let sseRetryCount = 0;
const SSE_MAX_RETRIES = 5; // 连续失败上限；超过后停止重连，避免 401 死循环
const eventListeners = [];

// Global app state
export const appState = {
    setupStatus: null,
    currentAgentId: null,
};

export function onEvent(callback) {
    eventListeners.push(callback);
    return () => {
        const idx = eventListeners.indexOf(callback);
        if (idx >= 0) eventListeners.splice(idx, 1);
    };
}

function startSSE() {
    if (eventSource) return;
    // EventSource 不支持自定义 header，SSE 端点改走 ?token= query 通道（后端对 /api/v1/events 开放）
    const token = getStoredAuthToken();
    if (!token) {
        console.warn('[SSE] 无 auth_token，跳过连接（请先完成 setup）');
        updateNavStatus(false);
        return;
    }
    const url = `${window.location.protocol}//${window.location.host}${API.EVENTS}?token=${encodeURIComponent(token)}`;
    eventSource = new EventSource(url);

    eventSource.addEventListener('open', () => {
        // 连接成功 → 复位重试计数
        sseRetryCount = 0;
        updateNavStatus(true);
    });

    // 后端用 SSE 命名事件（event: agent_died / tick_update / heartbeat），
    // 命名事件不会触发 'message' 监听器（SSE 规范行为），必须按名称逐个注册。
    // 这里统一包装成 { type, ...payload } 后转发给业务 callback。
    // connected/heartbeat 无业务语义，仅用于保活和连接状态，不转发。
    const namedEvents = ['agent_died', 'tick_update'];
    for (const eventName of namedEvents) {
        eventSource.addEventListener(eventName, (e) => {
            let payload;
            try {
                payload = JSON.parse(e.data);
            } catch (_) {
                return;
            }
            // 统一分发结构：payload 若无 type 字段则补 event 名
            const unified = payload && typeof payload === 'object'
                ? { ...payload, type: payload.type || eventName }
                : { type: eventName, data: payload };
            for (const cb of eventListeners) {
                try { cb(unified); } catch (_) {}
            }
        });
    }

    eventSource.addEventListener('error', () => {
        eventSource.close();
        eventSource = null;
        sseRetryCount += 1;
        if (sseRetryCount > SSE_MAX_RETRIES) {
            // 连续失败超过上限：很可能是认证/配置问题，停止重连避免死循环刷日志
            console.error(`[SSE] 连续 ${sseRetryCount - 1} 次失败，已停止重连。请检查 auth_token / 服务端状态。`);
            updateNavStatus(false);
            return;
        }
        // 指数退避：2s, 4s, 8s, 16s, 32s（带上限）
        const delay = Math.min(2000 * Math.pow(2, sseRetryCount - 1), 30000);
        setTimeout(startSSE, delay);
    });
}

function updateNavStatus(connected) {
    const el = document.getElementById('nav-status');
    if (!el) return;
    el.innerHTML = connected
        ? '<span class="status-dot connected"></span>'
        : '<span class="status-dot disconnected"></span>';
}

async function init() {
    // Register routes
    router.register('dashboard', dashboardPage);
    router.register('characters', characterPage);
    router.register('settings', settingsPage);

    // P0-11(b)：从 setup/status（公开端点）获取 auth_token 并缓存到 localStorage。
    // 必须在任何受保护 API 调用之前完成。refreshAuthToken 内部调用 get(SETUP_STATUS)。
    await refreshAuthToken();

    // Check setup status
    try {
        const status = await get(API.SETUP_STATUS);
        appState.setupStatus = status;
        updateNavStatus(true);
    } catch (_) {
        updateNavStatus(false);
    }

    // Start SSE
    startSSE();

    // Initial route
    if (!window.location.hash) {
        const configured = appState.setupStatus?.server_configured && appState.setupStatus?.llm_configured;
        window.location.hash = configured ? '#/dashboard' : '#/settings';
    }

    router.init();

    // Nav click handlers
    document.querySelectorAll('.nav-tab').forEach(tab => {
        tab.addEventListener('click', (e) => {
            e.preventDefault();
            router.navigate(`#/${tab.dataset.route}`);
        });
    });
}

document.addEventListener('DOMContentLoaded', init);
