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

        // 优先使用 soul_cycle_metadata（agent 上报的三魂完整链路）
        if (metadata && metadata.cycles && metadata.cycles.length > 0) {
            return renderSoulCycleExperience(exp, metadata, time);
        }

        // 兜底：使用旧数据（intent_history 无三魂链路时）
        return renderLegacyExperience(exp, time);
    }).join("");

    return '<div class="experience-list">' + expHtml + '</div>';
}

// 渲染三魂完整链路（优先使用）
function renderSoulCycleExperience(exp, metadata, time) {
    var attempts = metadata.cycles;
    var immediate = metadata.immediate_intents || [];
    var hasMultiple = attempts.length > 1;

    var html = '<div class="exp-item">' +
        '<div class="exp-header">' +
        '<span class="exp-tick-badge">T' + (exp.tick_id || '-') + '</span>' +
        '<span class="exp-time-info">' +
        '<span class="exp-world-time">' + escapeHtml(exp.action_type_display || exp.action_type || '-') + '</span>' +
        '<span class="exp-real-time">' + time + '</span></span>' +
        '<button class="exp-detail-btn" onclick="showSoulCycleDetailServer(this, ' + (exp.tick_id || 0) + ')">查看详情</button>' +
        '</div>';

    // 只显示第一个 attempt 的摘要（避免太长）
    var first = attempts[0];
    if (first.renhun && first.renhun.narrative) {
        html += '<div class="exp-renhun">' +
            '<span class="exp-soul-label">人魂</span>' +
            '<span class="exp-soul-content">' + escapeHtml(first.renhun.narrative) + '</span></div>';
    }
    if (first.tianhun && first.tianhun.action_type) {
        html += '<div class="exp-tianhun">' +
            '<span class="exp-soul-label">天魂</span>' +
            '<span class="exp-soul-content">' + escapeHtml(first.tianhun.action_type) + '</span></div>';
    }
    if (first.dihun) {
        html += '<div class="exp-dihun">' +
            '<span class="exp-soul-label">地魂</span>' +
            '<div class="exp-dihun-content">' +
            '<div class="exp-dihun-result">' + escapeHtml(first.dihun.result || '-') + '</div>';
        if (first.dihun.reason) {
            html += '<div class="exp-dihun-reason">' + escapeHtml(first.dihun.reason) + '</div>';
        }
        html += '</div></div>';
    }

    html += '</div>';

    // 将完整 metadata 存入元素 dataset
    html = html.replace('showSoulCycleDetailServer(this,', 'showSoulCycleDetailServer(this,') +
        '<script>window._soulMeta = window._soulMeta || {}; window._soulMeta[' + (exp.tick_id || 0) + '] = ' + JSON.stringify(metadata) + ';</script>';

    return html;
}

// 渲染旧数据（兜底）
function renderLegacyExperience(exp, time) {
    var actionData = exp.action_data || {};

    var renhunContent = '';
    var tianhunContent = '';
    var dihunContent = '';

    if (exp.thought_log) {
        renhunContent = '<div class="exp-renhun">' +
            '<span class="exp-soul-label">人魂</span>' +
            '<span class="exp-soul-content">' + escapeHtml(exp.thought_log) + '</span></div>';
    }

    if (exp.action_type || (actionData && Object.keys(actionData).length > 0)) {
        tianhunContent = '<div class="exp-tianhun">' +
            '<span class="exp-soul-label">天魂</span>' +
            '<span class="exp-soul-content">' + escapeHtml(exp.action_type || '-') + (actionData && Object.keys(actionData).length > 0 ? ' ' + escapeHtml(JSON.stringify(actionData)) : '') + '</span></div>';
    }

    if (exp.observer_thought) {
        var observerData = null;
        try {
            observerData = JSON.parse(exp.observer_thought);
        } catch (e) {
            observerData = null;
        }

        if (observerData) {
            dihunContent = '<div class="exp-dihun">' +
                '<span class="exp-soul-label">地魂</span>' +
                '<div class="exp-dihun-content">' +
                '<div class="exp-dihun-result">' + escapeHtml(observerData.result || '-') + '</div>' +
                (observerData.reason ? '<div class="exp-dihun-reason">' + escapeHtml(observerData.reason) + '</div>' : '') +
                (observerData.narrative ? '<div class="exp-dihun-narrative">' + escapeHtml(observerData.narrative) + '</div>' : '') +
                '</div></div>';
        } else {
            dihunContent = '<div class="exp-dihun">' +
                '<span class="exp-soul-label">地魂</span>' +
                '<span class="exp-soul-content">' + escapeHtml(exp.observer_thought) + '</span></div>';
        }
    }

    return '<div class="exp-item">' +
        '<div class="exp-header">' +
        '<span class="exp-tick-badge">T' + (exp.tick_id || '-') + '</span>' +
        '<span class="exp-time-info">' +
        '<span class="exp-world-time">' + escapeHtml(exp.action_type_display || exp.action_type || '-') + '</span>' +
        '<span class="exp-real-time">' + time + '</span></span>' +
        '</div>' +
        renhunContent + tianhunContent + dihunContent +
        '</div>';
}

// server-web 三魂详情弹窗（复用 agent-web 相同样式）
function showSoulCycleDetailServer(btn, tickId) {
    var modal = document.getElementById('soul-cycle-modal');
    var overlay = document.getElementById('soul-cycle-overlay');
    var body = document.getElementById('soul-cycle-body');

    if (!modal) {
        modal = document.createElement('div');
        modal.id = 'soul-cycle-modal';
        modal.className = 'dialog';
        overlay = document.createElement('div');
        overlay.id = 'soul-cycle-overlay';
        overlay.className = 'dialog-overlay';
        overlay.onclick = function() { closeSoulCycleModalServer(); };
        document.body.appendChild(overlay);
        document.body.appendChild(modal);
    }

    modal.innerHTML = `
        <div class="dialog-header">
            <h3>三魂详情 - T${tickId}</h3>
            <button class="dialog-close" onclick="closeSoulCycleModalServer()">&times;</button>
        </div>
        <div class="dialog-body" id="soul-cycle-body"><p class="loading-text">加载中...</p></div>
    `;
    modal.classList.add('show');
    overlay.classList.add('show');

    var metadata = (window._soulMeta || {})[tickId];
    body = document.getElementById('soul-cycle-body');

    if (!metadata || !metadata.cycles || metadata.cycles.length === 0) {
        body.innerHTML = '<p class="no-data">暂无三魂记录</p>';
        return;
    }

    var html = '';
    metadata.cycles.forEach(function(attempt, idx) {
        var attemptLabel = metadata.cycles.length > 1 ? ('<div class="soul-cycle-attempt-header">第 ' + (idx + 1) + ' 次尝试</div>') : '';
        html += '<div class="soul-cycle-attempt">' + attemptLabel;

        // 人魂
        html += '<div class="exp-renhun"><span class="exp-soul-label">人魂</span><div class="exp-soul-content">';
        if (attempt.renhun && attempt.renhun.narrative) {
            html += '<div class="soul-sub-title">叙事意图</div><div class="soul-text">' + escapeHtml(attempt.renhun.narrative) + '</div>';
        }
        if (attempt.renhun && attempt.renhun.thought_log) {
            html += '<div class="soul-sub-title">思考日志</div><div class="soul-text soul-thought">' + escapeHtml(attempt.renhun.thought_log) + '</div>';
        }
        html += '</div></div>';

        // 天魂
        html += '<div class="exp-tianhun"><span class="exp-soul-label">天魂</span><div class="exp-soul-content">';
        if (attempt.tianhun) {
            if (!attempt.tianhun.success) {
                html += '<div class="soul-error">翻译失败: ' + escapeHtml(attempt.tianhun.error || '未知错误') + '</div>';
            }
            if (attempt.tianhun.action_type) {
                html += '<div class="soul-sub-title">动作类型</div><div class="soul-text">' + escapeHtml(attempt.tianhun.action_type) + '</div>';
            }
            if (attempt.tianhun.action_data && Object.keys(attempt.tianhun.action_data).length > 0) {
                html += '<div class="soul-sub-title">动作参数</div><div class="soul-text">' + escapeHtml(JSON.stringify(attempt.tianhun.action_data)) + '</div>';
            }
            if (attempt.tianhun.speech_content) {
                html += '<div class="soul-sub-title">提取对话</div><div class="soul-text">' + escapeHtml(attempt.tianhun.speech_content) + '</div>';
            }
        }
        html += '</div></div>';

        // 地魂
        html += '<div class="exp-dihun"><span class="exp-soul-label">地魂</span><div class="exp-dihun-content">';
        html += '<div class="exp-dihun-result">' + escapeHtml(attempt.dihun && attempt.dihun.result ? attempt.dihun.result : '-') + '</div>';
        if (attempt.dihun && attempt.dihun.layers && attempt.dihun.layers.length > 0) {
            html += '<div class="soul-sub-title">三层审查</div>';
            attempt.dihun.layers.forEach(function(layer) {
                var status = layer.passed ? 'passed' : 'failed';
                var label = layer.passed ? '通过' : '驳回';
                html += '<div class="soul-layer ' + status + '">' +
                    '<span class="soul-layer-name">' + escapeHtml(layer.layer) + '</span>' +
                    '<span class="soul-layer-status">' + label + '</span>' +
                    (layer.detail ? '<span class="soul-layer-detail">' + escapeHtml(layer.detail) + '</span>' : '') +
                    '</div>';
            });
        }
        if (attempt.dihun && attempt.dihun.reason) {
            html += '<div class="exp-dihun-reason">驳回原因: ' + escapeHtml(attempt.dihun.reason) + '</div>';
        }
        if (attempt.dihun && attempt.dihun.narrative) {
            html += '<div class="exp-dihun-narrative">叙事化: ' + escapeHtml(attempt.dihun.narrative) + '</div>';
        }
        html += '</div></div>';

        // 最终 Intent
        if (attempt.final_intent) {
            html += '<div class="soul-final-intent">' +
                '<div class="soul-sub-title">最终 Intent</div>' +
                '<div class="soul-text">' + escapeHtml(attempt.final_intent.action_type || '-') + '</div>';
            if (attempt.final_intent.action_data) {
                html += '<div class="soul-text">' + escapeHtml(JSON.stringify(attempt.final_intent.action_data)) + '</div>';
            }
            html += '</div>';
        }

        html += '</div>';
    });

    // 即时意图
    if (metadata.immediate_intents && metadata.immediate_intents.length > 0) {
        html += '<div class="soul-immediate-section">' +
            '<div class="soul-section-title">即时通道（不占 Intent 配额）</div>';
        metadata.immediate_intents.forEach(function(imm) {
            html += '<div class="soul-immediate-item">' +
                '<span class="soul-action-type">' + escapeHtml(imm.action_type) + '</span>' +
                '<span class="soul-status ' + (imm.send_status === 'sent' ? 'sent' : 'failed') + '">' +
                (imm.send_status === 'sent' ? '已发送' : '失败') + '</span>';
            if (imm.speech_content) {
                html += '<div class="soul-text">' + escapeHtml(imm.speech_content) + '</div>';
            }
            if (imm.send_error) {
                html += '<div class="soul-error">错误: ' + escapeHtml(imm.send_error) + '</div>';
            }
            html += '</div>';
        });
        html += '</div>';
    }

    body.innerHTML = html;
}

function closeSoulCycleModalServer() {
    var modal = document.getElementById('soul-cycle-modal');
    var overlay = document.getElementById('soul-cycle-overlay');
    if (modal) modal.classList.remove('show');
    if (overlay) overlay.classList.remove('show');
}

function escapeHtml(text) {
    if (!text) return '';
    var div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

function formatActionSummary(actionType, actionData) {
    actionData = actionData || {};
    switch (actionType) {
        case "move":
            return "<strong>移动至:</strong> " + escapeHtml(getLocationName(actionData.target_location || actionData.target || "未知"));
        case "rest":
            return "<strong>休息恢复</strong>";
        case "eat":
            return "<strong>进食:</strong> " + escapeHtml(actionData.item_name || actionData.food || "食物");
        case "drink":
            return "<strong>饮水:</strong> " + escapeHtml(actionData.item_name || actionData.drink || "水");
        case "speak":
            var content = actionData.content || "...";
            var preview = content.length > 50 ? content.substring(0, 50) + "..." : content;
            return '<strong>对话:</strong> "' + escapeHtml(preview) + '"';
        case "idle":
            return "<strong>静待时机</strong>";
        case "craft":
            return "<strong>制作:</strong> " + escapeHtml(actionData.recipe_id || actionData.recipe || "物品");
        case "pickup":
            return "<strong>拾取:</strong> " + escapeHtml(actionData.item_name || actionData.item || "物品");
        case "drop":
            return "<strong>丢弃:</strong> " + escapeHtml(actionData.item_name || actionData.item || "物品");
        default:
            return "<strong>" + escapeHtml(actionType) + "</strong>";
    }
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
