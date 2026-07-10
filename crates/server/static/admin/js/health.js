// ============================================================================
// 健康度看板（MVP §6.1 验收指标视图）
// ============================================================================
//
// 只读展示 MVP 验收三大维度：运行稳定性 / 生存能力 / 涌现。
// 每项标注阈值 + pass/fail。10s 自动刷新。
// 超时率标注"近似值"（server 端无 deadline 概念）。

let healthRefreshTimer = null;

async function loadHealth() {
    await renderHealth();
    // 自动刷新 10s
    if (healthRefreshTimer) clearInterval(healthRefreshTimer);
    healthRefreshTimer = setInterval(renderHealth, 10000);
}

function stopHealthRefresh() {
    if (healthRefreshTimer) {
        clearInterval(healthRefreshTimer);
        healthRefreshTimer = null;
    }
}

async function renderHealth() {
    const container = document.getElementById('health-content');
    if (!container) return;
    container.innerHTML = '<p>加载中...</p>';

    try {
        const res = await apiFetch('/api/dashboard/health?window=240');
        if (!res.ok) {
            container.innerHTML = '<p class="error">加载失败：' + res.status + '</p>';
            return;
        }
        const data = await res.json();
        container.innerHTML = buildHealthHtml(data);
    } catch (e) {
        if (e.message !== 'UNAUTHORIZED') {
            container.innerHTML = '<p class="error">加载失败：' + escapeHtml(e.message) + '</p>';
        }
    }
}

function passBadge(pass) {
    return pass ? '<span class="pass">✅ 通过</span>' : '<span class="fail">❌ 未达标</span>';
}

function pct(v) {
    return (v * 100).toFixed(1) + '%';
}

function buildHealthHtml(d) {
    const s = d.stability;
    const sv = d.survival;
    const e = d.emergence;

    // 生存补给明细
    let supplyRows = '';
    if (sv.per_agent_supply && sv.per_agent_supply.length > 0) {
        supplyRows = sv.per_agent_supply.map(a =>
            `<tr><td>${a.agent_id.substring(0, 8)}</td>` +
            `<td>${a.supply_count}</td>` +
            `<td>${a.meets_threshold ? '✅' : '❌'}</td></tr>`
        ).join('');
    } else {
        supplyRows = '<tr><td colspan="3">无存活 agent</td></tr>';
    }

    return `
    <div class="health-grid">
        <h3>MVP 健康度看板（观测窗口 ${d.tick_start} – ${d.tick_end}，${d.window_ticks} tick）</h3>

        <div class="health-section">
            <h4>§6.1.1 运行稳定性 ${passBadge(s.pass && s.continuous_run_hours >= s.threshold_hours)}</h4>
            <table class="health-table">
                <tr><td>Tick 完成率</td><td>${pct(s.tick_completion_rate)} / 阈值 ${pct(s.threshold)}</td><td>${passBadge(s.pass)}</td></tr>
                <tr><td>连续运行时长</td><td>${s.continuous_run_hours.toFixed(2)}h / 阈值 ${s.threshold_hours}h</td><td>${passBadge(s.continuous_run_hours >= s.threshold_hours)}</td></tr>
                <tr><td>Tick 总数</td><td colspan="2">完成 ${s.ticks_completed} / 失败 ${s.ticks_failed} / 总 ${s.ticks_total}</td></tr>
                <tr><td>意图超时率（近似⚠️）</td><td>${pct(s.timeout_rate_approx)} / 阈值 ${pct(s.timeout_threshold)}</td><td>${passBadge(s.timeout_pass)}</td></tr>
            </table>
            <p class="note">⚠️ 超时率是近似值：server 端为实时流式处理，无 deadline 概念。
            此值 = 1 − (有动作提交的 agent 数 / 应参与 agent 数)，非 MVP 字面"30秒墙钟超时"。</p>
        </div>

        <div class="health-section">
            <h4>§6.1.2 生存能力 ${passBadge(sv.pass && sv.supply_pass)}</h4>
            <table class="health-table">
                <tr><td>存活 Agent 数</td><td>${sv.agents_alive} / 阈值 ≥ ${sv.min_survivors}</td><td>${passBadge(sv.pass)}</td></tr>
                <tr><td>人均补给达标</td><td colspan="2">${passBadge(sv.supply_pass)}（每人 ≥ ${sv.min_supply_count} 次）</td></tr>
            </table>
            <table class="health-table">
                <thead><tr><th>Agent</th><th>补给次数</th><th>达标</th></tr></thead>
                <tbody>${supplyRows}</tbody>
            </table>
        </div>

        <div class="health-section">
            <h4>§6.1.3 复杂交互（涌现） ${passBadge(e.pass)}</h4>
            <table class="health-table">
                <tr><td>Causal Emergence（因果验证通过）</td><td>${e.causal_emergence_count} / 阈值 ≥ ${e.threshold}</td><td>${passBadge(e.pass)}</td></tr>
                <tr><td>Co-occurrence（仅共现/存疑）</td><td colspan="2">${e.co_occurrence_count}</td></tr>
                <tr><td>候选事件总数</td><td colspan="2">${e.candidate_count}</td></tr>
            </table>
            <p class="note">Causal emergence = 通过"感知→处理→定向回应"因果闭环验证的事件链。
            Co-occurrence = 仅形态共现，无法证明因果互动。MVP 验收以 causal emergence 为准。</p>
            <button class="btn-secondary" onclick="toggleEmergenceDetail(${d.tick_start}, ${d.tick_end})" id="emergence-detail-btn">
                展开涌现事件详情 ▼
            </button>
            <div id="emergence-detail"></div>
        </div>
    </div>`;
}

// 涌现事件详情：按需拉取 /api/dashboard/emergence 的完整事件链
let emergenceDetailLoaded = false;

async function toggleEmergenceDetail(tickStart, tickEnd) {
    const detailDiv = document.getElementById('emergence-detail');
    const btn = document.getElementById('emergence-detail-btn');
    if (emergenceDetailLoaded) {
        detailDiv.innerHTML = '';
        btn.textContent = '展开涌现事件详情 ▼';
        emergenceDetailLoaded = false;
        return;
    }
    btn.textContent = '加载中...';
    try {
        const url = `/api/dashboard/emergence?start=${tickStart}&end=${tickEnd}`;
        const res = await apiFetch(url);
        if (!res.ok) {
            detailDiv.innerHTML = '<p class="note">加载失败：' + res.status + '</p>';
            btn.textContent = '展开涌现事件详情 ▼';
            return;
        }
        const data = await res.json();
        detailDiv.innerHTML = renderEmergenceDetail(data);
        btn.textContent = '收起涌现事件详情 ▲';
        emergenceDetailLoaded = true;
    } catch (e) {
        detailDiv.innerHTML = '<p class="note">加载失败：' + escapeHtml(e.message) + '</p>';
        btn.textContent = '展开涌现事件详情 ▼';
    }
}

function renderEmergenceDetail(data) {
    if (!data.events || data.events.length === 0) {
        return '<p class="note">本窗口未检测到涌现事件。</p>';
    }
    const items = data.events.map((e) => {
        const isCausal = e.category === 'causal_emergence';
        const label = isCausal ? '因果涌现' : '共现（存疑）';
        const cls = isCausal ? 'emergence-causal' : 'emergence-cooccur';
        const edges = (e.causal_edges || []).map((ed) => {
            const fn = (ed.from_agent || '').substring(0, 8);
            const tn = (ed.to_agent || '').substring(0, 8);
            return `<div class="emergence-edge">${escapeHtml(fn)} → ${escapeHtml(tn)}（${escapeHtml(ed.evidence || '')}）</div>`;
        }).join('');
        const participants = (e.participants || []).map((p) => escapeHtml((p || '').substring(0, 8))).join('、');
        return `<div class="emergence-item ${cls}">
            <span class="emergence-badge ${cls}">${escapeHtml(label)}</span>
            <span class="emergence-desc">tick ${escapeHtml(e.tick_start)}–${escapeHtml(e.tick_end)}，参与者 [${participants}]，${escapeHtml(e.action_count || 0)} 次互动（${escapeHtml((e.categories_covered || []).join('、'))}）</span>
            ${edges}
        </div>`;
    }).join('');
    return '<div class="emergence-list" style="margin-top:12px">' + items + '</div>';
}
