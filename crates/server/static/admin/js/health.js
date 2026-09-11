// ============================================================================
// 健康度看板（MVP 验收指标视图）
// ============================================================================
//
// 只读展示 MVP 验收四大维度：运行稳定性 / 生存能力 / 涌现 / 行为多样性。
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
        // 展示名映射就绪后再渲染（失败也不阻断，resolveTargetName 有兑底）
        await loadDisplayMap().catch(function () {});
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
    const b = d.behavior;

    // 生存补给明细
    let supplyRows = '';
    if (sv.per_agent_supply && sv.per_agent_supply.length > 0) {
        supplyRows = sv.per_agent_supply.map(a =>
            `<tr><td>${escapeHtml(resolveTargetName(a.agent_id))}</td>` +
            `<td>${a.supply_count}</td>` +
            `<td>${a.meets_threshold ? '✅' : '❌'}</td></tr>`
        ).join('');
    } else {
        supplyRows = '<tr><td colspan="3">无存活 agent</td></tr>';
    }

    return `
    <div class="health-grid">
        <h3>健康度看板（观测窗口 ${d.tick_start} – ${d.tick_end}，${d.window_ticks} tick）</h3>

        <div class="health-section">
            <h4>运行稳定性 ${passBadge(s.pass && s.continuous_run_hours >= s.threshold_hours)}</h4>
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
            <h4>生存能力 ${passBadge(sv.pass && sv.supply_pass)}</h4>
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
            <h4>复杂交互（涌现） ${passBadge(e.pass)}</h4>
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

        <div class="health-section">
            <h4>行为多样性（最频动作占比） ${passBadge(b.pass)}</h4>
            <table class="health-table">
                <tr><td>全员最频动作占比上界</td><td>${b.entropy_min.toFixed(2)} / 阈值 < ${b.min_entropy_ratio}</td><td>${passBadge(b.pass)}</td></tr>
                <tr><td>全员平均占比</td><td>${b.entropy_mean.toFixed(2)}</td><td>-</td></tr>
                <tr><td>饱食度紧迫阈值</td><td colspan="2">< ${b.satiation_urgent_below}（低于此值时高占比才计为卡死循环）</td></tr>
            </table>
            ${behaviorRows(b)}
            <p class="note">占比 = 窗口内最频动作的决策次数份额。
            判读交叉生存状态：饱食安稳下的高占比属合理情性（豁免），
            占比高且饱食度紧迫（< ${b.satiation_urgent_below}）才计为卡死循环。</p>
        </div>
    </div>`;
}

function behaviorRows(b) {
    if (!b.per_agent_behavior || b.per_agent_behavior.length === 0) {
        return '<p class="note">窗口内无决策数据</p>';
    }
    const rows = b.per_agent_behavior.map(a =>
        `<tr><td>${escapeHtml(resolveTargetName(a.agent_id))}</td>` +
        `<td>${escapeHtml(a.top_action)}</td>` +
        `<td>${(a.top_share * 100).toFixed(1)}%</td>` +
        `<td>${a.distinct_actions}</td>` +
        `<td>${a.total_decisions}</td>` +
        `<td>饱食 ${a.satiation > 900 ? '无快照' : a.satiation.toFixed(0)}</td>` +
        `<td>${a.exempted ? '豁免（饱食情性）' : (a.top_share >= b.max_top_share ? '❌ 卡死循环' : '✅')}</td></tr>`
    ).join('');
    return '<table class="health-table"><thead><tr><th>Agent</th><th>最频动作</th><th>占比</th><th>种类数</th><th>决策数</th><th>饱食度</th><th>判读</th></tr></thead>' +
        `<tbody>${rows}</tbody></table>`;
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
            const fn = resolveTargetName(ed.from_agent || '');
            const tn = resolveTargetName(ed.to_agent || '');
            return `<div class="emergence-edge">${escapeHtml(fn)} → ${escapeHtml(tn)}（${escapeHtml(ed.evidence || '')}）</div>`;
        }).join('');
        const participants = (e.participants || []).map((p) => escapeHtml(resolveTargetName(p || ''))).join('、');
        return `<div class="emergence-item ${cls}">
            <span class="emergence-badge ${cls}">${escapeHtml(label)}</span>
            <span class="emergence-desc">tick ${escapeHtml(e.tick_start)}–${escapeHtml(e.tick_end)}，参与者 [${participants}]，${escapeHtml(e.action_count || 0)} 次互动（${escapeHtml((e.categories_covered || []).join('、'))}）</span>
            ${edges}
        </div>`;
    }).join('');
    return '<div class="emergence-list" style="margin-top:12px">' + items + '</div>';
}
