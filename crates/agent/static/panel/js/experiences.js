// 经历独立页：全宽三魂时间线 + 传统分页 + 跳转 Tick
// 数据源：GET /api/v1/character/soul-cycles（agent 本地三魂记录，按 tick 分页）
// 渲染层复用 panels.js 的 renderTickCard / 翻译层 loadExpNameMaps

import { API, get } from './api.js';
import { escapeHtml, showLoading, showError, STATUS_MAP } from './ui.js';
import { groupByTick, renderTickCard, loadExpNameMaps, tryFetchLayerDisplay } from './panels.js';

let page = 1;
let pageSize = 20; // API 上限 50
let totalPages = 1;
let agentId = ''; // '' = 当前活跃角色

export const experiencesPage = {
    mount(container) {
        page = 1;
        agentId = '';
        showLoading(container);
        renderSkeleton(container);
        bindControls();
        tryFetchLayerDisplay(); // best-effort 从服务器拉取层配置；失败时静默保留硬编码
        init();
    },
};

function renderSkeleton(container) {
    container.innerHTML = `
    <div>
        <div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-bottom:12px">
            <select class="form-select" id="exp-agent-select" style="min-width:220px">
                <option value="">当前角色</option>
            </select>
            <input class="form-input" type="number" id="exp-tick-input" placeholder="跳转 Tick" min="0" style="width:110px">
            <button class="btn btn-sm" id="exp-tick-btn">跳转</button>
            <select class="form-select" id="exp-page-size" style="width:auto">
                <option value="20">20 tick/页</option>
                <option value="50">50 tick/页</option>
            </select>
            <button class="btn btn-sm" id="exp-refresh-btn">刷新</button>
        </div>
        <div id="exp-container"></div>
        <div id="exp-pagination" style="display:none;justify-content:center;gap:12px;align-items:center;margin-top:12px">
            <button class="btn btn-sm" id="exp-prev-btn">上一页</button>
            <span id="exp-page-info" style="font-size:12px;color:var(--text-muted)"></span>
            <button class="btn btn-sm" id="exp-next-btn">下一页</button>
        </div>
    </div>`;
}

async function init() {
    await Promise.allSettled([loadAgentOptions(), loadExpNameMaps()]); // 翻译层就绪后再渲染
    await loadPage();
}

function bindControls() {
    document.getElementById('exp-agent-select')?.addEventListener('change', (e) => {
        agentId = e.target.value;
        page = 1;
        loadPage();
    });
    document.getElementById('exp-page-size')?.addEventListener('change', (e) => {
        pageSize = parseInt(e.target.value) || 20;
        page = 1;
        loadPage();
    });
    document.getElementById('exp-refresh-btn')?.addEventListener('click', () => loadPage());
    document.getElementById('exp-prev-btn')?.addEventListener('click', () => {
        if (page > 1) { page--; loadPage(); }
    });
    document.getElementById('exp-next-btn')?.addEventListener('click', () => {
        if (page < totalPages) { page++; loadPage(); }
    });
    document.getElementById('exp-tick-btn')?.addEventListener('click', jumpToTick);
    document.getElementById('exp-tick-input')?.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') jumpToTick();
    });
}

async function loadAgentOptions() {
    try {
        const data = await get(API.CHARACTERS);
        const characters = data.characters || data || [];
        const select = document.getElementById('exp-agent-select');
        if (!select) return;
        let html = '<option value="">当前角色</option>';
        for (const ch of characters) {
            const id = ch.agent_id || ch.id;
            const status = ch.status || 'unknown';
            html += `<option value="${escapeHtml(id)}">${escapeHtml(ch.name || id)}(${String(id).substring(0, 8)})(${STATUS_MAP[status] || status})</option>`;
        }
        select.innerHTML = html;
    } catch (_) { /* 下拉加载失败不阻塞经历列表 */ }
}

async function loadPage() {
    const box = document.getElementById('exp-container');
    if (!box) return;
    showLoading(box);
    try {
        const param = agentId ? `&agent_id=${agentId}` : '';
        const data = await get(`${API.SOUL_CYCLES}?page=${page}&limit=${pageSize}${param}`);

        // records: { tick_id: [SoulCycleAttemptEntry] }, immediate_intents: { tick_id: [...] }
        let recordMap = data.records || {};
        if (Array.isArray(recordMap)) { recordMap = groupByTick(recordMap); }
        const immMap = data.immediate_intents || {};

        const tickIds = Object.keys(recordMap).sort((a, b) => Number(b) - Number(a));

        if (tickIds.length === 0) {
            // 页码越界（如筛选后 total 变小）：钳到末页重载，避免空白列表
            const pages = Math.max(1, Math.ceil((data.total ?? 0) / pageSize));
            if (page > pages) {
                page = pages;
                return loadPage();
            }
            box.innerHTML = '<p class="text-muted">暂无经历记录</p>';
            updatePagination(data.total ?? 0);
            return;
        }

        let html = '<div class="exp-list">';
        for (const tickId of tickIds) {
            html += renderTickCard(tickId, recordMap[tickId] || [], immMap[tickId] || []);
        }
        html += '</div>';
        box.innerHTML = html;
        updatePagination(data.total ?? tickIds.length);
    } catch (e) {
        box.innerHTML = `<p class="text-muted">经历加载失败: ${escapeHtml(e.message)}</p>`;
    }
}

function updatePagination(total) {
    totalPages = Math.max(1, Math.ceil(total / pageSize));
    const el = document.getElementById('exp-pagination');
    if (!el) return;
    el.style.display = 'flex';
    document.getElementById('exp-page-info').textContent = `第 ${page} / ${totalPages} 页 · 共 ${total} tick`;
    document.getElementById('exp-prev-btn').disabled = page <= 1;
    document.getElementById('exp-next-btn').disabled = page >= totalPages;
}

async function jumpToTick() {
    const input = document.getElementById('exp-tick-input');
    const box = document.getElementById('exp-container');
    if (!input || !box) return;
    const tick = parseInt(input.value);
    if (!Number.isFinite(tick) || tick < 0) {
        showError('请输入有效的 Tick');
        return;
    }
    showLoading(box);
    try {
        const param = agentId ? `&agent_id=${agentId}` : '';
        const data = await get(`${API.SOUL_CYCLES}?tick_id=${tick}${param}`);
        const attempts = data.attempts || [];
        const immediate = data.immediate_intents || [];

        let html = `<div style="margin-bottom:10px"><button class="btn btn-sm" id="exp-back-btn">← 返回列表</button></div>`;
        if (attempts.length === 0 && immediate.length === 0) {
            html += `<p class="text-muted">Tick ${escapeHtml(String(tick))} 无经历记录</p>`;
        } else {
            html += `<div class="exp-list">${renderTickCard(String(tick), attempts, immediate)}</div>`;
        }
        box.innerHTML = html;

        // 跳转视图隐藏分页栏；返回按钮恢复
        const pag = document.getElementById('exp-pagination');
        if (pag) pag.style.display = 'none';
        document.getElementById('exp-back-btn')?.addEventListener('click', () => loadPage());
    } catch (e) {
        box.innerHTML = `<p class="text-muted">经历加载失败: ${escapeHtml(e.message)}</p>`;
    }
}
