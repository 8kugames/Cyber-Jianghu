// Shared UI components: toast, modal, loading

export function escapeHtml(text) {
    if (!text) return '';
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

export function showToast(message, type = 'info', duration = 3000) {
    const container = document.getElementById('toast-container');
    if (!container) return;

    const toast = document.createElement('div');
    toast.className = `toast toast-${type}`;
    toast.innerHTML = `
        <span class="toast-message">${escapeHtml(message)}</span>
        <button class="toast-close" aria-label="close">&times;</button>
    `;
    container.appendChild(toast);
    requestAnimationFrame(() => toast.classList.add('toast-show'));

    const timeoutId = setTimeout(() => removeToast(toast), duration);
    toast.querySelector('.toast-close').addEventListener('click', () => {
        clearTimeout(timeoutId);
        removeToast(toast);
    });
}

function removeToast(toast) {
    toast.classList.remove('toast-show');
    toast.classList.add('toast-hide');
    setTimeout(() => toast.remove(), 300);
}

export const showSuccess = (m) => showToast(m, 'success');
export const showError = (m) => showToast(m, 'error', 4000);
export const showWarning = (m) => showToast(m, 'warning', 3500);
export const showInfo = (m) => showToast(m, 'info');

export function showModal(html, options = {}) {
    const overlay = document.getElementById('modal-overlay');
    const body = document.getElementById('modal-body');
    if (!overlay || !body) return;
    body.innerHTML = html;
    overlay.classList.remove('hidden');
    if (options.className) body.classList.add(options.className);
    return { close: () => hideModal() };
}

export function hideModal() {
    const overlay = document.getElementById('modal-overlay');
    const body = document.getElementById('modal-body');
    if (overlay) overlay.classList.add('hidden');
    if (body) body.innerHTML = '';
}

export function showLoading(container, message = '加载中...') {
    container.innerHTML = `<div class="loading"><div class="spinner"></div><p>${escapeHtml(message)}</p></div>`;
}

export function formatDateTime(isoString) {
    if (!isoString) return '-';
    const date = new Date(isoString);
    return date.toLocaleDateString('zh-CN') + ' ' +
        date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

function getShichen(hour) {
    const table = [[0,1,'子时'],[2,3,'丑时'],[4,5,'寅时'],[6,7,'卯时'],[8,9,'辰时'],[10,11,'巳时'],[12,13,'午时'],[14,15,'未时'],[16,17,'申时'],[18,19,'酉时'],[20,21,'戌时'],[22,23,'亥时']];
    for (const [lo, hi, name] of table) { if (hour >= lo && hour <= hi) return name; }
    return '';
}

export function extractActionSummary(rec) {
    if (!rec.final_action_type) return '-';
    const pipeline = rec.final_pipeline_json;
    if (pipeline) {
        try {
            const items = JSON.parse(pipeline);
            if (Array.isArray(items) && items.length > 0) {
                return items.map(i => {
                    const content = i.action_data?.content;
                    if (content && (i.action_type === 'speak' || i.action_type === 'whisper')) return `${i.action_type}: ${content}`;
                    return i.action_type || '-';
                }).join(' → ');
            }
        } catch (_) {}
    }
    const data = rec.final_action_data;
    if (data) {
        try {
            const parsed = JSON.parse(data);
            if (parsed.content) return `${rec.final_action_type}: ${parsed.content}`;
        } catch (_) {}
    }
    return rec.final_action_type;
}

export function getActionColor(type) {
    if (!type) return '#e2e4e8';
    const t = type.toLowerCase();
    if (t === 'speak' || t === 'whisper' || t === 'shout') return '#4fc08d';
    if (t === 'move' || t === 'travel') return '#7b68ee';
    if (t.includes('fight') || t.includes('attack') || t === 'combat') return '#e06c75';
    if (t.includes('gather') || t.includes('craft') || t.includes('cook')) return '#e5c07b';
    return '#61afef';
}

export function getAttrColor(name, _pct) {
    const n = name.toLowerCase();
    if (n.includes('hp') || n.includes('health') || n.includes('生命')) return '#e06c75';
    if (n.includes('hunger') || n.includes('饥饿')) return '#e5c07b';
    if (n.includes('stamina') || n.includes('体力')) return '#4fc08d';
    if (n.includes('thirst') || n.includes('渴')) return '#61afef';
    if (n.includes('sanity') || n.includes('精神') || n.includes('理智')) return '#7b68ee';
    return '#4078f2';
}

export function fmtNum(n) {
    if (n >= 1000000) return (n / 1000000).toFixed(1) + 'M';
    if (n >= 1000) return (n / 1000).toFixed(1) + 'K';
    return String(n);
}

export const STATUS_MAP = { alive: '活跃', dead: '死亡', retired: '退休' };

export function formatWorldTime(worldTime) {
    if (!worldTime) return '-';
    if (typeof worldTime === 'string' || typeof worldTime === 'number') return String(worldTime);
    if (typeof worldTime === 'object') {
        if (worldTime.display != null) return String(worldTime.display);
        const year = worldTime.year ?? worldTime.y;
        const month = worldTime.month ?? worldTime.m;
        const day = worldTime.day ?? worldTime.d;
        const hour = worldTime.hour ?? worldTime.h;
        if (year !== undefined) {
            return `${year}年${month ?? '?'}月${day ?? '?'}日 ${getShichen(hour ?? 0)}`;
        }
    }
    return '-';
}
