// 赛博江湖 - Agent Web Panel 共享模块

const API_BASE = `${window.location.protocol}//${window.location.host}`;
const DEFAULT_TIMEOUT_MS = 10000;
const MAX_RETRIES = 2;

// XSS 防护
function escapeHtml(text) {
    if (!text) return '';
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

// 带超时 + 重试的 fetch
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
            console.warn(`网络错误，${MAX_RETRIES - retries + 1} 秒后重试...`);
            await new Promise(r => setTimeout(r, (MAX_RETRIES - retries + 1) * 1000));
            return fetchWithRetry(url, options, retries - 1, timeoutMs);
        }
        throw error;
    }
}

function isNetworkError(error) {
    return error.message.includes('网络错误') ||
           error.message.includes('Failed to fetch') ||
           error.message.includes('NetworkError') ||
           error.message.includes('请求超时');
}

// Toast 通知
function showToast(message, type = 'info', duration = 3000) {
    const existing = document.querySelector('.toast-container');
    if (existing) existing.remove();

    const container = document.createElement('div');
    container.className = 'toast-container';
    container.innerHTML = `
        <div class="toast toast-${type}">
            <span class="toast-message">${escapeHtml(message)}</span>
            <button class="toast-close" aria-label="关闭">&times;</button>
        </div>
    `;
    document.body.appendChild(container);
    requestAnimationFrame(() => container.querySelector('.toast').classList.add('toast-show'));

    const timeoutId = setTimeout(() => hideToast(container), duration);
    container.querySelector('.toast-close').addEventListener('click', () => {
        clearTimeout(timeoutId);
        hideToast(container);
    });
}

function hideToast(container) {
    const toast = container.querySelector('.toast');
    if (toast) {
        toast.classList.remove('toast-show');
        toast.classList.add('toast-hide');
        setTimeout(() => container.remove(), 300);
    }
}

const showSuccess = (m) => showToast(m, 'success');
const showError = (m) => showToast(m, 'error', 4000);
const showWarning = (m) => showToast(m, 'warning', 3500);
const showInfo = (m) => showToast(m, 'info');

// API 请求
async function parseApiResponse(response) {
    const text = await response.text();
    if (!text) return null;
    try {
        return JSON.parse(text);
    } catch (_) {
        return { message: text };
    }
}

async function apiGet(endpoint, options = {}) {
    const timeout = options.timeout ?? DEFAULT_TIMEOUT_MS;
    const retries = options.retries ?? MAX_RETRIES;
    const response = await fetchWithRetry(`${API_BASE}${endpoint}`, {
        method: 'GET',
        headers: { 'Content-Type': 'application/json' },
    }, retries, timeout);
    const data = await parseApiResponse(response);
    if (!response.ok) throw new Error((data && data.message) || `服务器错误: ${response.status}`);
    return data ?? {};
}

async function apiPost(endpoint, body, options = {}) {
    const timeout = options.timeout ?? DEFAULT_TIMEOUT_MS;
    const retries = options.retries ?? MAX_RETRIES;
    const response = await fetchWithRetry(`${API_BASE}${endpoint}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
    }, retries, timeout);
    const data = await parseApiResponse(response);
    if (!response.ok) throw new Error((data && data.message) || `服务器错误: ${response.status}`);
    return data ?? {};
}

// 工具函数
function formatDateTime(isoString) {
    if (!isoString) return '-';
    const date = new Date(isoString);
    return date.toLocaleDateString('zh-CN') + ' ' +
           date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

function formatWorldTime(worldTime) {
    if (!worldTime) return '-';
    const h = String(worldTime.hour || 0).padStart(2, '0');
    const m = String(worldTime.minute || 0).padStart(2, '0');
    return `${worldTime.year}年${worldTime.month}月${worldTime.day}日 ${h}:${m}`;
}

function setVisible(selector, visible) {
    const el = typeof selector === 'string' ? document.querySelector(selector) : selector;
    if (el) el.classList.toggle('hidden', !visible);
}

const show = (s) => setVisible(s, true);
const hide = (s) => setVisible(s, false);

// 注入 Toast 样式
(function initToastStyles() {
    if (document.getElementById('toast-styles')) return;
    const style = document.createElement('style');
    style.id = 'toast-styles';
    style.textContent = `
        .toast-container {
            position: fixed;
            top: 70px;
            right: 20px;
            z-index: 9999;
            max-width: 320px;
        }
        .toast {
            display: flex;
            align-items: center;
            gap: 10px;
            padding: 12px 16px;
            background: #fff;
            border-radius: 8px;
            box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
            border-left: 4px solid #999;
            transform: translateX(100%);
            opacity: 0;
            transition: all 0.3s ease;
        }
        .toast-show { transform: translateX(0); opacity: 1; }
        .toast-hide { transform: translateX(100%); opacity: 0; }
        .toast-success { border-left-color: #4fc08d; }
        .toast-error { border-left-color: #e06c75; }
        .toast-warning { border-left-color: #e5c07b; }
        .toast-info { border-left-color: #61afef; }
        .toast-message { flex: 1; font-size: 14px; color: #383a42; }
        .toast-close {
            background: none;
            border: none;
            font-size: 20px;
            color: #5c6370;
            cursor: pointer;
            padding: 0;
            line-height: 1;
        }
        .toast-close:hover { color: #383a42; }
    `;
    document.head.appendChild(style);
})();
