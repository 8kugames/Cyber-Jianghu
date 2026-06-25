// Entry: router init, global state, SSE connection

import * as router from './router.js';
import { API, get, refreshAuthToken } from './api.js';

// Page modules (loaded on demand)
import { dashboardPage } from './dashboard.js';
import { characterPage } from './character.js';
import { settingsPage } from './settings.js';

// Global SSE connection
let eventSource = null;
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
    const url = `${window.location.protocol}//${window.location.host}${API.EVENTS}`;
    eventSource = new EventSource(url);

    eventSource.addEventListener('message', (e) => {
        try {
            const data = JSON.parse(e.data);
            for (const cb of eventListeners) cb(data);
        } catch (_) {}
    });

    eventSource.addEventListener('error', () => {
        eventSource.close();
        eventSource = null;
        setTimeout(startSSE, 5000);
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
