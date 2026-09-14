// ============================================================================
// 关系图谱（有向主观认知视图）
// ============================================================================
//
// 数据源：GET /api/dashboard/agent-relationships（一次取齐全部有向边，含 key_events）
//
// 方向性铁律：每行是一条有向边 source→target。A 对 B 的认知与 B 对 A 的认知是
// 两条独立数据（migration 022 主键为有向对，写入按 source 隔离覆盖），本页只
// 并排对比、绝不合并或平均——认知分歧本身是有向图谱的核心信息。
// ============================================================================

var relRows = [];
var relReverseMap = {};
var relFilterText = "";
var relExpandedIdx = null;

function relDisplayName(agentId, fallbackName) {
    if (displayMapCache.agents && displayMapCache.agents[agentId]) {
        return formatNameId(displayMapCache.agents[agentId], agentId);
    }
    if (fallbackName) return formatNameId(fallbackName, agentId);
    return resolveTargetName(agentId);
}

function relFavHtml(fav) {
    if (fav === null || fav === undefined) return "-";
    var color = fav > 0 ? "var(--success)" : fav < 0 ? "var(--error)" : "var(--text-secondary)";
    return '<span style="color:' + color + ';font-weight:600">' + escapeHtml(String(fav)) + "</span>";
}

function relSyncedAtHtml(ms) {
    if (!ms) return "-";
    return escapeHtml(new Date(ms).toLocaleString("zh-CN"));
}

// 属性值转义：escapeHtml 不转义双引号，title 等属性内嵌动态文本必须额外处理
function relEscapeAttr(v) {
    return escapeHtml(v).replace(/"/g, "&quot;");
}

async function loadRelationships() {
    var container = document.getElementById("relationships-content");
    if (!container) return;
    container.innerHTML = '<p class="loading">加载中...</p>';
    relExpandedIdx = null;
    try {
        var res = await apiFetch(API.BASE + "/agent-relationships");
        if (!res.ok) throw new Error("HTTP " + res.status);
        var data = await res.json();
        relRows = data.relationships || [];
        // 反查表：target→source→行，用于展示反向认知（独立数据，仅对比参照）
        relReverseMap = {};
        relRows.forEach(function (r) {
            var targetId = r.relationship.target_agent_id;
            if (!relReverseMap[targetId]) relReverseMap[targetId] = {};
            relReverseMap[targetId][r.source_agent_id] = r.relationship;
        });
        await loadDisplayMap().catch(function () {});
        renderRelationships();
    } catch (e) {
        container.innerHTML =
            '<p class="error">关系图谱加载失败：' + escapeHtml(e.message || e) + "</p>";
    }
}

function relFilteredRows() {
    var text = relFilterText.trim().toLowerCase();
    if (!text) return relRows;
    return relRows.filter(function (r) {
        var sourceName = relDisplayName(r.source_agent_id).toLowerCase();
        var targetName = relDisplayName(
            r.relationship.target_agent_id,
            r.relationship.target_name
        ).toLowerCase();
        return (
            sourceName.indexOf(text) >= 0 ||
            targetName.indexOf(text) >= 0 ||
            String(r.source_agent_id).toLowerCase().indexOf(text) >= 0 ||
            String(r.relationship.target_agent_id).toLowerCase().indexOf(text) >= 0
        );
    });
}

function relStatsHtml() {
    var holders = {};
    var mutualEdges = 0;
    relRows.forEach(function (r) {
        holders[r.source_agent_id] = true;
        var rev = relReverseMap[r.relationship.target_agent_id];
        if (rev && rev[r.source_agent_id]) mutualEdges += 1;
    });
    var mutualPairs = mutualEdges / 2;
    var oneWay = relRows.length - mutualEdges;

    var html = '<div class="stats-grid">';
    html +=
        '<div class="stat-card"><div class="stat-value">' +
        relRows.length +
        '</div><div class="stat-label">有向关系总数</div></div>';
    html +=
        '<div class="stat-card"><div class="stat-value">' +
        Object.keys(holders).length +
        '</div><div class="stat-label">关系持有者</div></div>';
    html +=
        '<div class="stat-card"><div class="stat-value">' +
        mutualPairs +
        '</div><div class="stat-label">互见关系对（双向均有认知）</div></div>';
    html +=
        '<div class="stat-card"><div class="stat-value">' +
        oneWay +
        '</div><div class="stat-label">单向认知（仅一方有记录）</div></div>';
    html += "</div>";
    return html;
}

function relEventsHtml(idx) {
    var rel = relRows[idx].relationship;
    var events = rel.key_events || [];
    if (events.length === 0) {
        return '<p class="empty" style="margin:8px 0">该关系暂无关键事件记录</p>';
    }
    var html = "";
    events.forEach(function (ev) {
        var delta = ev.favorability_delta > 0 ? "+" + ev.favorability_delta : String(ev.favorability_delta);
        var when = ev.timestamp ? new Date(ev.timestamp).toLocaleString("zh-CN") : "-";
        html +=
            '<div style="padding:6px 0;border-bottom:1px solid var(--border-color);font-size:12px">'
            + '<span class="result-badge">' + escapeHtml(ev.event_type) + "</span> "
            + escapeHtml(ev.description)
            + ' <span style="color:var(--text-secondary)">| Tick ' + escapeHtml(String(ev.tick_id))
            + " | 好感度 " + escapeHtml(delta)
            + " | " + escapeHtml(when) + "</span>"
            + "</div>";
    });
    return html;
}

function relRowHtml(item, idx) {
    var rel = item.relationship;
    var sourceName = relDisplayName(item.source_agent_id);
    var targetName = relDisplayName(rel.target_agent_id, rel.target_name);
    var rev = relReverseMap[rel.target_agent_id];
    var reverseRel = rev ? rev[item.source_agent_id] : null;

    var eventsCount = (rel.key_events || []).length;
    var expanded = relExpandedIdx === idx;

    var html = '<tr>';
    html += '<td>' + escapeHtml(sourceName) + '</td>';
    html += '<td style="color:var(--text-secondary)">→</td>';
    html += '<td>' + escapeHtml(targetName) + '</td>';
    html += '<td>' + relFavHtml(rel.favorability) + '</td>';
    if (reverseRel) {
        html +=
            '<td title="' + relEscapeAttr("目标对持有者的独立认知（有向边 " + targetName + " → " + sourceName + "）") + '">' +
            relFavHtml(reverseRel.favorability) +
            ' <span style="color:var(--text-secondary);font-size:11px">' +
            escapeHtml(reverseRel.self_description || "") + "</span></td>";
    } else {
        html +=
            '<td style="color:var(--text-secondary)" title="目标侧未上报对持有者的认知（单向关系）">— 无反向认知</td>';
    }
    html += '<td>' + escapeHtml(rel.self_description || "-") + '</td>';
    html += '<td>' + escapeHtml(String(rel.last_interaction_tick)) + '</td>';
    html += '<td>';
    if (eventsCount > 0) {
        html +=
            '<button class="btn btn-sm" id="rel-events-btn-' + idx + '" onclick="toggleRelEvents(' + idx + ')">' +
            (expanded ? "收起 ▲" : "展开 ▼") + " (" + eventsCount + ")</button>";
    } else {
        html += '<span style="color:var(--text-secondary)">0</span>';
    }
    html += '</td>';
    html += '<td>' + relSyncedAtHtml(rel.updated_at) + '</td>';
    html += '</tr>';

    html +=
        '<tr id="rel-events-' + idx + '" style="' + (expanded ? "" : "display:none") + '">'
        + '<td colspan="9" style="background:var(--bg-level-2);padding:4px 16px">'
        + relEventsHtml(idx)
        + "</td></tr>";

    return html;
}

function renderRelationships() {
    var container = document.getElementById("relationships-content");
    if (!container) return;

    // 标题/刷新/过滤框是 index.html 里的静态骨架，这里只填动态区，
    // 避免重渲染吞掉过滤输入框焦点（同 dashboard tab 模式）
    var html = "";

    if (relRows.length === 0) {
        html = '<p class="empty">暂无关系数据（agent 在游戏日结束时上报快照后此处展示）</p>';
        container.innerHTML = html;
        return;
    }

    html += relStatsHtml();

    var rows = relFilteredRows();
    if (rows.length === 0) {
        html += '<p class="empty">无匹配关系</p>';
        container.innerHTML = html;
        return;
    }

    html += '<table class="data-table"><thead><tr>'
        + "<th>持有者</th><th></th><th>目标</th><th>好感度</th>"
        + "<th>反向认知（目标 → 持有者）</th><th>叙事描述</th>"
        + "<th>最后交互 Tick</th><th>关键事件</th><th>同步时间</th>"
        + "</tr></thead><tbody>";

    rows.forEach(function (item) {
        // 展开态与过滤后行集对齐：以 relRows 中的原始下标为稳定 id
        var idx = relRows.indexOf(item);
        html += relRowHtml(item, idx);
    });

    html += "</tbody></table>";
    container.innerHTML = html;
}

function toggleRelEvents(idx) {
    relExpandedIdx = relExpandedIdx === idx ? null : idx;
    var expanded = relExpandedIdx === idx;
    var row = document.getElementById("rel-events-" + idx);
    if (row) row.style.display = expanded ? "" : "none";
    var btn = document.getElementById("rel-events-btn-" + idx);
    if (btn) btn.textContent = expanded ? "收起 ▲" : "展开 ▼";
}
