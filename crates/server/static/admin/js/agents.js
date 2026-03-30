// ============================================================================
// Agent Functions
// ============================================================================

// Load status configs (data-driven)
async function loadStatusConfigs() {
    try {
        var res = await fetch("/api/dashboard/status-configs", { headers: getAuthHeaders() });
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
        return agent.name.toLowerCase().includes(filterText) ||
            agent.location.toLowerCase().includes(filterText);
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
            '<div class="agent-name">' + agent.name + '</div>' +
            '<div class="location">' + getLocationName(agent.location) + '</div>' +
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

    document.querySelector("#dashboard h2").textContent = "所有角色 (" + counts.total + ")";
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
    event.target.classList.add("active");
    document.querySelectorAll(".modal-tab-content").forEach(function (c) { c.classList.remove("active"); });
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
    var inventoryHtml = agent.inventory.length === 0
        ? '<div style="color: #999; font-size: 13px; text-align: center; padding: 10px;">空空如也</div>'
        : '<div class="inventory-grid">' +
        agent.inventory.map(function (item) {
            return '<div class="inventory-item ' + (item.is_equipped ? "equipped" : "") + '">' +
                '<div style="margin-bottom: 2px;">' + item.name + '</div>' +
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
        (agent.system_prompt || "") +
        '</div></div>' +
        '</div>';
}

function renderExperiences(data) {
    if (!data.experiences || data.experiences.length === 0) {
        return '<div style="text-align: center; padding: 40px; color: #999;">暂无经历记录</div>';
    }

    var expHtml = data.experiences.map(function (exp) {
        var time = exp.created_at ? new Date(exp.created_at).toLocaleString() : "Tick #" + exp.tick_id;
        var actionData = exp.action_data || {};
        var actionSummary = formatActionSummary(exp.action_type, actionData);
        return '<div style="border-left: 3px solid var(--accent-color); padding: 12px; margin-bottom: 12px; background: #f8f9fa; border-radius: 0 8px 8px 0;">' +
            '<div style="display: flex; justify-content: space-between; margin-bottom: 8px;">' +
            '<span style="font-weight: 600; color: var(--primary-color);">' + exp.action_type + '</span>' +
            '<span style="font-size: 12px; color: #999;">' + time + '</span></div>' +
            '<div style="font-size: 14px; margin-bottom: 6px;">' + actionSummary + '</div>' +
            (exp.result ? '<div style="font-size: 13px; color: #27ae60; font-style: italic; background: white; padding: 8px; border-radius: 4px;"><strong>结果:</strong> ' + exp.result + '</div>' : '') +
            '</div>';
    }).join("");

    return '<div class="experiences-list">' + expHtml + '</div>';
}

function formatActionSummary(actionType, actionData) {
    switch (actionType) {
        case "move":
            return "<strong>移动至:</strong> " + getLocationName(actionData.target_location || actionData.target || "未知");
        case "rest":
            return "<strong>休息恢复</strong>";
        case "eat":
            return "<strong>进食:</strong> " + (actionData.item_name || actionData.food || "食物");
        case "drink":
            return "<strong>饮水:</strong> " + (actionData.item_name || actionData.drink || "水");
        case "speak":
            var content = actionData.content || "...";
            var preview = content.length > 50 ? content.substring(0, 50) + "..." : content;
            return '<strong>对话:</strong> "' + preview + '"';
        case "idle":
            return "<strong>静待时机</strong>";
        case "craft":
            return "<strong>制作:</strong> " + (actionData.recipe_id || actionData.recipe || "物品");
        case "pickup":
            return "<strong>拾取:</strong> " + (actionData.item_name || actionData.item || "物品");
        case "drop":
            return "<strong>丢弃:</strong> " + (actionData.item_name || actionData.item || "物品");
        default:
            return "<strong>" + actionType + "</strong>";
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
