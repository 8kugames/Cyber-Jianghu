// ============================================================================
// Agent Functions
// ============================================================================

// Load status configs (data-driven)
async function loadStatusConfigs() {
    try {
        var res = await fetch("/api/dashboard/status-configs", { headers: getAuthHeaders() });
        if (handleAuthError(res)) return;
        if (res.ok) {
            var configs = await res.json();
            configs.forEach(function (cfg) {
                statusConfigs[cfg.key] = cfg;
            });
        }
    } catch (e) {
        console.error("Failed to load status configs:", e);
    }
}

// Action type display name mapping (loaded from server)
var actionTypeMap = {};
async function loadActionTypeMap() {
    try {
        var res = await fetch("/api/dashboard/actions-map");
        if (res.ok) {
            actionTypeMap = await res.json();
        }
    } catch (e) {
        console.warn("[actions] Failed to load action type map:", e);
    }
}
function getActionTypeDisplay(actionType) {
    return actionTypeMap[actionType] || actionType;
}

async function loadAllAgents() {
    try {
        var res = await fetch("/api/dashboard/agents", { headers: getAuthHeaders() });
        if (handleAuthError(res)) return;
        allAgents = await res.json();
        agentPage = 1;
        renderAgents();
    } catch (e) {
        console.error("Failed to load agents", e);
        showToast("加载 Agent 列表失败", "error");
    }
}

var agentPageSize = 20;
var agentPage = 1;
var lastAgentFilterText = "";

function renderAgents() {
    var listEl = document.getElementById("agents-list");
    var filterText = document.getElementById("agent-filter").value.toLowerCase();
    if (filterText !== lastAgentFilterText) {
        agentPage = 1;
        lastAgentFilterText = filterText;
    }

    var sourceData = allAgents || [];

    if (!sourceData || sourceData.length === 0) {
        listEl.innerHTML = '<div class="agent-list-empty">暂无 Agent</div>';
        updateAgentCounts();
        updateAgentPagination(0, 1);
        return;
    }

    var filteredAgents = sourceData.filter(function (agent) {
        return (agent.name || "").toLowerCase().includes(filterText) ||
            (agent.location || "").toLowerCase().includes(filterText);
    });

    if (filteredAgents.length === 0) {
        listEl.innerHTML = '<div class="agent-list-empty">没有匹配的 Agent</div>';
        updateAgentCounts();
        updateAgentPagination(0, 1);
        return;
    }

    var totalPages = Math.max(1, Math.ceil(filteredAgents.length / agentPageSize));
    if (agentPage > totalPages) agentPage = totalPages;
    var startIndex = (agentPage - 1) * agentPageSize;
    var pageAgents = filteredAgents.slice(startIndex, startIndex + agentPageSize);

    // Grid header
    var headerHtml = '<div class="agent-list-header">' +

        '<div>设备 ID</div>' +
        '<div>Agent ID</div>' +
        '<div>名称</div>' +
        '<div>位置</div>' +
        '<div>状态</div>' +
        '<div>最后活跃</div>' +
        '<div>最后 Tick</div>' +
        '<div>创建时间</div>' +
        '<div>状态值</div>' +
        '<div>先天属性</div>' +
        '<div></div>' +
        '</div>';

    var rowsHtml = pageAgents.map(function (agent) {
        var deviceIdShort = agent.device_id ? agent.device_id.substring(0, 4) + '..' + agent.device_id.substring(agent.device_id.length - 4) : '-';
        var agentIdShort = agent.id ? agent.id.substring(0, 4) + '..' + agent.id.substring(agent.id.length - 4) : '-';

        // 数据驱动：格式化属性为 pretty JSON（排序、中文 display_name、curr/max 配对）
        function formatAttrsPretty(attrs, categoryFilter) {
            var parts = [];
            // 排序后只取基础 key（跳过 _max），按需过滤 category
            Object.keys(attrs).sort().forEach(function (k) {
                if (k.endsWith("_max")) return;
                var meta = attributeMeta[k];
                if (categoryFilter && meta && meta.category !== categoryFilter) return;
                if (categoryFilter === "status" && (!meta || meta.category !== "status")) return;
                var name = meta ? meta.display_name : k;
                var val = attrs[k];
                var maxVal = attrs[k + "_max"];
                if (maxVal !== undefined) {
                    parts.push('"' + name + '": "' + val + '/' + maxVal + '"');
                } else {
                    parts.push('"' + name + '": "' + val + '"');
                }
            });
            return parts.length > 0 ? '{ ' + parts.join(', ') + ' }' : '-';
        }

        var statusText = formatAttrsPretty(agent.attributes || {}, "status");
        var birthText = formatAttrsPretty(agent.birth_attributes || {}, "primary");

        return '<div class="agent-item" onclick="openAgentModal(\'' + agent.id + '\')">' +

            '<div class="device-id">' + deviceIdShort + '</div>' +
            '<div class="agent-id">' + agentIdShort + '</div>' +
            '<div class="agent-name">' + escapeHtml(agent.name) + '</div>' +
            '<div class="location">' + escapeHtml(getLocationName(agent.location)) + '</div>' +
            '<div class="status"><span class="status-badge status-' + agent.status + '">' + getStatusText(agent.status) + '</span></div>' +
            '<div class="last-active">' + formatLastActive(agent.last_active) + '</div>' +
            '<div class="last-tick">' + (agent.last_tick_id || '-') + '</div>' +
            '<div class="created-at">' + formatCreatedAt(agent.created_at) + '</div>' +
            '<div class="status-attrs">' + statusText + '</div>' +
            '<div class="birth-attrs">' + birthText + '</div>' +
            '<div class="detail-btn">详情</div>' +
            '</div>';
    }).join("");

    listEl.innerHTML = headerHtml + rowsHtml;
    updateAgentCounts();
    updateAgentPagination(filteredAgents.length, totalPages);
}

function updateAgentCounts() {
    var counts = allAgents ? {
        total: allAgents.length,
        online: allAgents.filter(function (a) { return a.status === "online"; }).length,
        offline: allAgents.filter(function (a) { return a.status === "offline"; }).length,
        dead: allAgents.filter(function (a) { return a.status === "dead"; }).length
    } : { total: 0, online: 0, offline: 0, dead: 0 };

    document.getElementById("agents-total-title").textContent = "所有角色 (" + counts.total + ")";
    document.getElementById("online-count").textContent = counts.online;
    document.getElementById("offline-count").textContent = counts.offline;
    document.getElementById("dead-count").textContent = counts.dead;
}

function updateAgentPagination(totalItems, totalPages) {
    var infoEl = document.getElementById("agent-page-info");
    var prevBtn = document.getElementById("agent-page-prev");
    var nextBtn = document.getElementById("agent-page-next");
    if (infoEl) infoEl.textContent = "第 " + agentPage + " / " + totalPages + " 页 · 共 " + totalItems + " 条";
    if (prevBtn) prevBtn.disabled = agentPage <= 1;
    if (nextBtn) nextBtn.disabled = agentPage >= totalPages;
}

function changeAgentPage(delta) {
    var nextPage = agentPage + delta;
    if (nextPage < 1) nextPage = 1;
    agentPage = nextPage;
    renderAgents();
}

// ============================================================================
// Agent Modal
// ============================================================================

async function openAgentModal(agentId) {
    var modal = document.getElementById("agent-modal");
    var title = document.getElementById("modal-agent-name");
    modal.classList.add("show");
    switchModalTab("basic");

    try {
        var agentRes = await fetch("/api/dashboard/agent/" + agentId, { headers: getAuthHeaders() });
        if (handleAuthError(agentRes)) return;
        var agent = await agentRes.json();
        title.textContent = agent.name;

        document.getElementById("modal-tab-basic").innerHTML = renderBasicInfo(agent);

        var expRes = await fetch("/api/dashboard/agent/" + agentId + "/experiences?page=1&limit=20", { headers: getAuthHeaders() });
        if (handleAuthError(expRes)) {
            document.getElementById("modal-tab-experiences").innerHTML =
                '<div style="text-align: center; padding: 20px; color: #999;">无法加载经历日志</div>';
        } else {
            var expData = await expRes.json();
            document.getElementById("modal-tab-experiences").innerHTML = renderExperiences(expData);
        }
    } catch (e) {
        console.error("Failed to load agent details", e);
        document.getElementById("modal-agent-body").innerHTML =
            '<div style="text-align: center; padding: 20px; color: #999;">加载失败</div>';
    }
}

function switchModalTab(tab) {
    document.querySelectorAll(".modal-tab").forEach(function (t) { t.classList.remove("active"); });
    document.querySelectorAll(".modal-tab-content").forEach(function (c) { c.classList.remove("active"); });
    var tabBtn = document.querySelector('.modal-tab[data-tab="' + tab + '"]');
    if (tabBtn) tabBtn.classList.add("active");
    document.getElementById("modal-tab-" + tab).classList.add("active");
}

function closeAgentModal() {
    document.getElementById("agent-modal").classList.remove("show");
}

window.onclick = function (event) {
    var modal = document.getElementById("agent-modal");
    if (event.target == modal) closeAgentModal();
};

function renderBasicInfo(agent) {
    var inventoryHtml = (agent.inventory || []).length === 0
        ? '<div style="color: #999; font-size: 13px; text-align: center; padding: 10px;">空空如也</div>'
        : '<div class="inventory-grid">' +
        agent.inventory.map(function (item) {
            return '<div class="inventory-item ' + (item.is_equipped ? "equipped" : "") + '">' +
                '<div style="margin-bottom: 2px;">' + escapeHtml(item.name) + '</div>' +
                '<div style="font-weight: 600; color: #666;">x' + item.count + '</div></div>';
        }).join("") + '</div>';

    return '<div class="basic-info-grid">' +
        '<div class="detail-section">' +
        '<div class="detail-title">基本信息 <span class="detail-label">ID:</span> <span style="font-family: monospace; font-size: 12px;">' + agent.id + '</span></div>' +
        '<div class="detail-grid">' +
        '<div class="detail-item"><span class="detail-label">位置:</span> ' + getLocationName(agent.location) + '</div>' +
        '<div class="detail-item"><span class="detail-label">状态:</span> <span class="status-badge ' + (agent.is_alive ? "status-alive" : "status-dead") + '">' + (agent.is_alive ? "存活" : "死亡") + '</span></div>' +
        '<div class="detail-item"><span class="detail-label">创建时间:</span> ' + new Date(agent.created_at).toLocaleString() + '</div>' +
        '<div class="detail-item"><span class="detail-label">最后活跃:</span> ' + (agent.last_active ? new Date(agent.last_active).toLocaleString() : "从未") + '</div>' +
        '</div></div>' +

        '<div class="detail-section">' +
        '<div class="detail-title">生理状态</div>' +
        '<div class="detail-grid">' +
        '<div class="detail-item"><span class="detail-label">生命值 (HP):</span> ' + agent.hp + '/' + agent.max_hp + '</div>' +
        '<div class="detail-item"><span class="detail-label">体力 (Stamina):</span> ' + agent.stamina + '/' + agent.max_stamina + '</div>' +
        '<div class="detail-item"><span class="detail-label">饱食度 (Hunger):</span> ' + agent.hunger + '/' + agent.max_hunger + '</div>' +
        '<div class="detail-item"><span class="detail-label">口渴度 (Thirst):</span> ' + agent.thirst + '/' + agent.max_thirst + '</div>' +
        '</div></div>' +

        '<div class="detail-section">' +
        '<div class="detail-title">先天属性</div>' +
        '<div class="detail-grid">' +
        '<div class="detail-item"><span class="detail-label">力量 (STR):</span> ' + ((agent.attributes || {}).strength || 0) + '/' + ((agent.attributes || {}).strength_max || 100) + '</div>' +
        '<div class="detail-item"><span class="detail-label">敏捷 (AGI):</span> ' + ((agent.attributes || {}).agility || 0) + '/' + ((agent.attributes || {}).agility_max || 100) + '</div>' +
        '<div class="detail-item"><span class="detail-label">根骨 (CON):</span> ' + ((agent.attributes || {}).constitution || 0) + '/' + ((agent.attributes || {}).constitution_max || 100) + '</div>' +
        '<div class="detail-item"><span class="detail-label">悟性 (INT):</span> ' + ((agent.attributes || {}).intelligence || 0) + '/' + ((agent.attributes || {}).intelligence_max || 100) + '</div>' +
        '<div class="detail-item"><span class="detail-label">魅力 (CHA):</span> ' + ((agent.attributes || {}).charisma || 0) + '</div>' +
        '<div class="detail-item"><span class="detail-label">福缘 (LUK):</span> ' + ((agent.attributes || {}).luck || 0) + '</div>' +
        '</div></div>' +

        '<div class="detail-section">' +
        '<div class="detail-title">背包物品</div>' +
        inventoryHtml +
        '</div>' +

        '<div class="detail-section">' +
        '<div class="detail-title">人设 Prompt</div>' +
        '<div style="font-size: 12px; color: #555; background: #f8f9fa; padding: 10px; border-radius: 4px; line-height: 1.4; max-height: 150px; overflow-y: auto;">' +
        escapeHtml(agent.system_prompt || "") +
        '</div></div>' +
        '</div>';
}

function renderExperiences(data) {
    if (!data.experiences || data.experiences.length === 0) {
        return '<div style="text-align: center; padding: 40px; color: #999;">暂无经历记录</div>';
    }

    var expHtml = data.experiences.map(function (exp) {
        var time = exp.created_at ? new Date(exp.created_at).toLocaleString() : "Tick #" + exp.tick_id;
        var metadata = exp.soul_cycle_metadata;

        if (metadata && metadata.cycles && metadata.cycles.length > 0) {
            return renderTickCard(exp, metadata, time);
        }

        return '';
    }).join("");

    return '<div class="experience-list">' + expHtml + '</div>';
}

// 渲染 Tick 卡片（三魂完整链路）
function renderTickCard(exp, metadata, time) {
    var attempts = metadata.cycles || [];
    var immediate = metadata.immediate_intents || [];
    var worldTimeDisplay = metadata.world_time || '-';

    var html = '<div class="tick-card">' +
        '<div class="tick-card-header">' +
        '<span class="tick-badge">T' + (exp.tick_id || '-') + '</span>' +
        '<span class="tick-world-time">' + escapeHtml(worldTimeDisplay) + '</span>' +
        '<span class="tick-real-time">' + time + '</span>' +
        '</div>';

    // 行动分区
    html += '<div class="tick-section"><div class="tick-section-title">行动</div>';
    attempts.forEach(function(attempt, idx) {
        if (attempts.length > 1) {
            html += '<div class="tick-attempt-label">第 ' + (idx + 1) + ' 次尝试</div>';
        }
        html += renderServerSoulInline('人魂', attempt.renhun, 'renhun');
        html += renderServerSoulInline('天魂', attempt.tianhun, 'tianhun');
        html += renderServerSoulInline('地魂', attempt.dihun, 'dihun');
    });
    html += '</div>';

    // 即时分区
    if (immediate.length > 0) {
        html += '<div class="tick-section tick-section-immediate"><div class="tick-section-title">即时</div>';
        immediate.forEach(function(imm) {
            html += '<div class="imm-item">' +
                '<div class="exp-tianhun"><span class="exp-soul-label">天魂</span>' +
                '<span class="exp-soul-content">' + escapeHtml(imm.action_type) +
                (imm.speech_content ? ': ' + escapeHtml(imm.speech_content) : '') +
                '</span></div>' +
                '<span class="imm-status ' + (imm.send_status === 'sent' ? 'sent' : 'failed') + '">' +
                (imm.send_status === 'sent' ? '已发送' : '失败') + '</span>' +
                (imm.send_error ? '<span class="imm-error">' + escapeHtml(imm.send_error) + '</span>' : '') +
                '</div>';
        });
        html += '</div>';
    }

    html += '</div>';
    return html;
}

// 渲染单魂内联区块（server 版本）
function renderServerSoulInline(label, data, type) {
    if (!data) return '';
    var html = '<div class="exp-' + type + '"><span class="exp-soul-label">' + label + '</span><div class="exp-soul-content">';
    if (data.narrative) html += '<div class="exp-dihun-narrative">' + escapeHtml(data.narrative) + '</div>';
    if (type === 'renhun') {
        if (data.narrative) html += '<div class="soul-text">' + escapeHtml(data.narrative) + '</div>';
        if (data.thought_log) html += '<div class="soul-thought">' + escapeHtml(data.thought_log) + '</div>';
    } else if (type === 'tianhun') {
        if (!data.success) html += '<div class="soul-error">翻译失败: ' + escapeHtml(data.error || '未知错误') + '</div>';
        if (data.action_type) {
            html += '<div class="soul-text">' + escapeHtml(getActionTypeDisplay(data.action_type));
            if (data.action_data && Object.keys(data.action_data).length > 0) {
                html += ' <span class="soul-params">' + escapeHtml(JSON.stringify(data.action_data)) + '</span>';
            }
            html += '</div>';
        }
        if (data.speech_content) html += '<div class="soul-speech">' + escapeHtml(data.speech_content) + '</div>';
    } else if (type === 'dihun') {
        html += '<div class="exp-dihun-result ' + (data.result || '') + '">' + escapeHtml(data.result || '-') + '</div>';
        if (data.layers && data.layers.length > 0) {
            html += '<div class="soul-layers">';
            data.layers.forEach(function(l) {
                var cls = l.passed ? 'passed' : 'failed';
                html += '<span class="soul-layer-tag ' + cls + '">' + l.layer +
                    (l.passed ? '' : ': ' + escapeHtml(l.detail || '')) + '</span>';
            });
            html += '</div>';
        }
        if (data.reason) html += '<div class="exp-dihun-reason">' + escapeHtml(data.reason) + '</div>';

    }

    html += '</div></div>';
    return html;
}

function escapeHtml(text) {
    if (!text) return '';
    var div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

async function cleanupOfflineAgents() {
    if (!confirm("确定要清理长期离线的 Agent 吗？这将直接从数据库中删除它们。")) return;
    try {
        var res = await fetch("/api/dashboard/agents/cleanup", {
            method: "POST",
            headers: getAuthHeaders(),
        });
        if (handleAuthError(res)) return;
        if (res.ok) {
            var data = await res.json();
            showToast("清理成功！共删除了 " + data.deleted_count + " 个离线 Agent。", "success");
            loadStats();
        } else {
            var errorText = await res.text();
            showToast("清理失败: " + errorText, "error");
        }
    } catch (e) {
        console.error("Failed to cleanup agents", e);
        showToast("网络请求失败", "error");
    }
}
