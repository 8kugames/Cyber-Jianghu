// Dashboard page: three-column overview

import { API, get } from './api.js';
import { escapeHtml, showLoading, getAttrColor, fmtNum, STATUS_MAP, formatDateTime } from './ui.js';
import { onEvent } from './app.js';

export const dashboardPage = {
    mount(container) {
        showLoading(container);
        render(container);
        loadAll();
    },
    unmount() {
        if (unsubscribe) { unsubscribe(); unsubscribe = null; }
    },
};

let unsubscribe = null;

function render(container) {
    container.innerHTML = `
    <div class="dashboard" style="grid-template-columns:minmax(320px,1fr) 1fr">
        <div class="dashboard-left" id="dash-left">
            <div class="card">
                <div class="card-header">角色状态</div>
                <div class="card-body" id="dash-char-status">
                    <p class="text-muted">加载中...</p>
                </div>
            </div>
            <div class="card" style="margin-top:12px">
                <div class="card-body" id="dash-radar" style="text-align:center;min-height:160px">
                    <canvas id="radar-canvas" width="360" height="360"></canvas>
                </div>
            </div>
            <div class="card" style="margin-top:12px">
                <div class="card-header">系统监控</div>
                <div class="card-body" id="dash-monitor">
                    <p class="text-muted">加载中...</p>
                </div>
            </div>
        </div>
        <div class="dashboard-center" id="dash-center">
            <div class="card">
                <div class="card-header">经历日志</div>
                <div class="card-body" style="text-align:center;padding:30px;color:var(--text-muted);">
                    <p style="margin-bottom:12px;">经历日志已迁移至角色面板</p>
                    <a href="#/characters" style="color:var(--accent,#14b8a6);font-size:14px;">前往查看 →</a>
                </div>
            </div>
        </div>
    </div>`;

    // Subscribe to SSE events
    unsubscribe = onEvent((event) => handleSSEEvent(event));
}

async function loadAll() {
    await Promise.allSettled([
        loadCharStatus(),
        loadMonitor(),
    ]);
}

async function loadCharStatus() {
    const el = document.getElementById('dash-char-status');
    if (!el) return;

    try {
        const [state, attrs, meta, character] = await Promise.all([
            get(API.STATE),
            get(API.ATTRIBUTES),
            get(API.ATTRIBUTE_META),
            get(API.CHARACTER).catch(() => null),
        ]);

        const charName = character?.name || character?.agent_name || state.agent_name || '-';
        const agentId = character?.agent_id || '-';
        const status = character?.status || 'unknown';
        const statusLabel = STATUS_MAP[status] || status;
        const age = character?.age ?? '-';
        const gender = character?.gender || '-';
        const registeredAt = formatDateTime(character?.registered_at);
        const serverUrl = character?.server_url || '-';

        // Group attributes by category
        const categories = meta.categories || {};
        const statusAttrs = categories.status || [];
        const displayNames = meta.display_names || {};

        // Build attribute bars for status category
        let barsHtml = '';
        const allAttrs = attrs.attributes || [];
        for (const attrName of statusAttrs) {
            const attr = allAttrs.find(a => a.name === attrName);
            if (!attr) continue;
            const raw = attrs.raw?.[attrName];
            const pct = raw != null ? Math.min(100, Math.max(0, raw)) : 50;
            const color = getAttrColor(attrName, pct);
            const label = displayNames[attrName] || attrName;
            barsHtml += `
            <div class="attr-bar">
                <div class="attr-bar-label">
                    <span>${escapeHtml(label)}</span>
                    <span>${escapeHtml(attr.value_str)}</span>
                </div>
                <div class="attr-bar-track">
                    <div class="attr-bar-fill" style="width:${pct}%;background:${color}"></div>
                </div>
            </div>`;
        }

        // Location
        const location = character?.location_name || character?.location_id || state.location?.name || state.location?.node_id || '-';

        el.innerHTML = `
            <div style="display:flex;align-items:center;gap:10px;margin-bottom:12px">
                <div style="width:42px;height:42px;background:var(--accent-light);border-radius:50%;display:flex;align-items:center;justify-content:center;font-size:18px">${escapeHtml(charName.charAt(0))}</div>
                <div style="flex:1;min-width:0">
                    <div style="display:flex;align-items:center;gap:8px">
                        <span style="font-size:16px;font-weight:600">${escapeHtml(charName)}</span>
                        <span style="color:${status === 'alive' ? 'var(--success)' : 'var(--danger)'};font-size:12px">${statusLabel}</span>
                    </div>
                    <div style="font-size:12px;color:var(--text-muted)">${escapeHtml(gender)} · ${age}岁 · ${escapeHtml(location)}</div>
                    <div style="font-size:11px;color:var(--text-muted);margin-top:1px">${escapeHtml(agentId)}</div>
                    <div style="font-size:11px;color:var(--text-muted);margin-top:1px">注册时间: ${escapeHtml(registeredAt)}</div>
                    <div style="font-size:11px;color:var(--text-muted);margin-top:1px">Server: ${escapeHtml(serverUrl)}</div>
                </div>
            </div>
            ${barsHtml}
        `;

        // Draw radar chart
        drawRadar(allAttrs, categories, displayNames);
    } catch (e) {
        el.innerHTML = `<p class="text-muted">角色状态不可用</p>`;
    }
}

async function loadMonitor() {
    const el = document.getElementById('dash-monitor');
    if (!el) return;

    try {
        const [tick, usage, cognitive, status] = await Promise.allSettled([
            get(API.TICK),
            get(API.CONFIG_LLM_USAGE),
            get(API.COGNITIVE),
            get(API.SETUP_STATUS),
        ]);

        let totalInput = 0, totalOutput = 0, totalCalls = 0;
        if (usage.status === 'fulfilled' && Array.isArray(usage.value)) {
            usage.value.forEach(item => {
                totalInput += item.prompt_tokens || 0;
                totalOutput += item.completion_tokens || 0;
                totalCalls += item.calls || 0;
            });
        }

        const connected = status.status === 'fulfilled' && status.value?.has_server;
        const tickId = tick.status === 'fulfilled' ? tick.value?.tick_id ?? '-' : '-';
        // sanity: 从 cognitive context 中提取
        let sanity = '-';
        if (cognitive.status === 'fulfilled' && cognitive.value) {
            const cog = cognitive.value;
            const attrs = cog.world_state?.attributes || {};
            sanity = attrs.sanity ?? attrs.理智 ?? cog.sanity ?? '-';
        }

        el.innerHTML = `
        <div class="metric-grid">
            <div class="metric-item">
                <div class="metric-label">连接</div>
                <div class="metric-value"><span class="connection-dot ${connected ? 'connected' : 'disconnected'}"></span></div>
            </div>
            <div class="metric-item">
                <div class="metric-label">Tick</div>
                <div class="metric-value">${tickId}</div>
            </div>
            <div class="metric-item">
                <div class="metric-label">LLM 调用</div>
                <div class="metric-value">${fmtNum(totalCalls)}</div>
            </div>
            <div class="metric-item">
                <div class="metric-label">Token</div>
                <div class="metric-value">${fmtNum(totalInput + totalOutput)}</div>
            </div>
            <div class="metric-item">
                <div class="metric-label">理智</div>
                <div class="metric-value">${sanity}</div>
            </div>
        </div>`;
    } catch (e) {
        el.innerHTML = '<p class="text-muted">监控数据不可用</p>';
    }
}

function handleSSEEvent(event) {
    if (event.type === 'death' || event.type === 'tick') {
        loadAll();
    }
}

function drawRadar(attributes, categories, displayNames) {
    const canvas = document.getElementById('radar-canvas');
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    const w = canvas.width, h = canvas.height;
    const cx = w / 2, cy = h / 2, r = Math.min(cx, cy) - 30;

    // Collect combat-related attributes for radar
    const combatAttrs = categories.combat || categories.physical || [];
    const allNames = combatAttrs.length > 0 ? combatAttrs : attributes.slice(0, 6).map(a => a.name);
    const radarAttrs = allNames
        .map(name => {
            const attr = attributes.find(a => a.name === name);
            return attr ? { name, display: displayNames[name] || attr.display_name || name, value: attr.value_str } : null;
        })
        .filter(Boolean);

    if (radarAttrs.length < 3) return;

    ctx.clearRect(0, 0, w, h);
    const n = radarAttrs.length;
    const angleStep = (2 * Math.PI) / n;

    // Draw grid
    ctx.strokeStyle = '#e2e4e8';
    ctx.lineWidth = 0.5;
    for (let ring = 1; ring <= 3; ring++) {
        const rr = (r * ring) / 3;
        ctx.beginPath();
        for (let i = 0; i <= n; i++) {
            const angle = -Math.PI / 2 + i * angleStep;
            const x = cx + rr * Math.cos(angle);
            const y = cy + rr * Math.sin(angle);
            if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
        }
        ctx.stroke();
    }

    // Draw data polygon — 动态归一化：取数据最大值作基准
    const numericValues = radarAttrs.map(a => parseFloat(a.value)).filter(n => !isNaN(n));
    const maxValue = numericValues.length > 0 ? Math.max(...numericValues, 1) : 100;
    const values = radarAttrs.map(a => {
        const num = parseFloat(a.value);
        return isNaN(num) ? 0.5 : Math.min(1, Math.max(0, num / maxValue));
    });

    ctx.fillStyle = 'rgba(64, 120, 242, 0.15)';
    ctx.strokeStyle = '#4078f2';
    ctx.lineWidth = 2;
    ctx.beginPath();
    for (let i = 0; i <= n; i++) {
        const idx = i % n;
        const angle = -Math.PI / 2 + idx * angleStep;
        const x = cx + r * values[idx] * Math.cos(angle);
        const y = cy + r * values[idx] * Math.sin(angle);
        if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.fill();
    ctx.stroke();

    // Draw labels
    ctx.fillStyle = '#5c6370';
    ctx.font = '10px sans-serif';
    ctx.textAlign = 'center';
    for (let i = 0; i < n; i++) {
        const angle = -Math.PI / 2 + i * angleStep;
        const lx = cx + (r + 18) * Math.cos(angle);
        const ly = cy + (r + 18) * Math.sin(angle);
        ctx.fillText(`${radarAttrs[i].display} ${radarAttrs[i].value}`, lx, ly + 4);
    }
}
