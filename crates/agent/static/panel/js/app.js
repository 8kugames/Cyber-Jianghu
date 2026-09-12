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
            // 连续失败超过上限：很可能是认证/配置问题，停止重连避免死循环刷日志。
            // 轻提示（底部，避让顶部认证横幅）替代纯 console：用户可感知实时流已停
            console.error(`[SSE] 连续 ${sseRetryCount - 1} 次失败，已停止重连。请检查 auth_token / 服务端状态。`);
            showDismissibleBanner('sse-stopped-warning',
                '实时事件流（SSE）连接失败已停止重连，面板数据可能滞后。'
                + '常见原因：访问令牌无效或服务端不可达；刷新页面重试，若持续失败请检查 agent 状态。',
                'bottom:0', '#8a6d1f');
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

// 访问对端未获授权 token（远程浏览器 / Docker 端口发布形态下 setup/status 不返回 token）。
// 显式提示替代静默 401：本机原生部署可自动获取；容器/远程形态请编辑 agent.yaml 后重启。
const AUTH_WARNING_MISSING = '未获得访问令牌：当前访问形态（远程浏览器或 Docker 端口发布）下 '
    + 'setup/status 不返回 token，认证 API 将被拒绝。本机原生部署可自动获取；'
    + '容器/远程部署请编辑 agent.yaml 后重启实例。';

// 可关闭的全屏宽横幅工厂：认证类用顶部红，SSE 新提示用底部 Qualcomm 色避免与认证横幅重叠
function showDismissibleBanner(id, message, cssPosition, background) {
    if (document.getElementById(id)) return;
    const el = document.createElement('div');
    el.id = id;
    el.style.cssText = `position:fixed;${cssPosition};left:0;right:0;z-index:9999;`
        + `background:${background};`
        + 'color:#fff;padding:8px 32px 8px 12px;font-size:13px;line-height:1.5;';
    el.textContent = message;
    const close = document.createElement('button');
    close.textContent = '×';
    close.setAttribute('aria-label', '关闭');
    close.style.cssText = 'position:absolute;right:6px;top:4px;background:none;border:none;'
        + 'color:#fff;font-size:16px;cursor:pointer;';
    close.onclick = () => el.remove();
    el.appendChild(close);
    document.body.appendChild(el);
}

function showAuthTokenWarning(message) {
    showDismissibleBanner('auth-token-warning', message, 'top:0', '#7a2e2e');
}

// 令牌失效（服务端轮换等）：清缓存动作在 api.js 的 handleUnauthorized 内完成，
// 这里接收事件并以横幅提示，替代静默 401 空数据
window.addEventListener('cj:unauthorized', () => showAuthTokenWarning(
    '访问令牌无效或已失效（可能已被服务端轮换），已清除本地缓存。'
    + '刷新页面将重新获取；若持续失败，当前访问形态（远程浏览器或 Docker 端口发布）下 '
    + '请编辑 agent.yaml 后重启实例。'));

async function init() {
    // Register routes
    router.register('dashboard', dashboardPage);
    router.register('characters', characterPage);
    router.register('settings', settingsPage);

    // 从 setup/status（公开端点）获取 auth_token 并缓存到 localStorage。
    // 必须在任何受保护 API 调用之前完成。refreshAuthToken 内部调用 get(SETUP_STATUS)。
    await refreshAuthToken();
    if (!getStoredAuthToken()) showAuthTokenWarning(AUTH_WARNING_MISSING);

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
