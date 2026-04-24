// ============================================================================
// Agent Functions
// ============================================================================

// 全局物品列表缓存（供 grant-items UI 使用）
var allItemsList = null;
// 当前打开 modal 的 agent ID
var currentModalAgentId = null;

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
                // 如果指定了 categoryFilter 且 meta 存在，则按 category 过滤
                // 注意：即使 meta 不存在也继续（fallback 显示原始 key）
                if (categoryFilter && meta && meta.category !== categoryFilter) return;
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
    currentModalAgentId = agentId;
    grantItemsBuffer = []; // 重置待注入列表

    try {
        var agentRes = await fetch("/api/dashboard/agent/" + agentId, { headers: getAuthHeaders() });
        if (handleAuthError(agentRes)) return;
        var agent = await agentRes.json();
        title.textContent = agent.name;

        document.getElementById("modal-tab-basic").innerHTML = renderBasicInfo(agent);
        document.getElementById("modal-tab-inventory").innerHTML = await renderInventoryManage(agent);

        var expRes = await fetch("/api/dashboard/agent/" + agentId + "/experiences?page=1&limit=20", { headers: getAuthHeaders() });
        if (handleAuthError(expRes)) {
            document.getElementById("modal-tab-experiences").innerHTML =
                '<div style="text-align: center; padding: 20px; color: var(--text-subtle);">无法加载经历日志</div>';
        } else {
            var expData = await expRes.json();
            document.getElementById("modal-tab-experiences").innerHTML = renderExperiences(expData);
        }
    } catch (e) {
        console.error("Failed to load agent details", e);
        document.getElementById("modal-agent-body").innerHTML =
            '<div style="text-align: center; padding: 20px; color: var(--text-subtle);">加载失败</div>';
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
    grantItemsBuffer = [];
    currentModalAgentId = null;
}

window.onclick = function (event) {
    var modal = document.getElementById("agent-modal");
    if (event.target == modal) closeAgentModal();
};

function renderBasicInfo(agent) {
    // 数据驱动：渲染属性列表
    // attrs: agent.attributes 对象
    // category: "primary" | "status" | "derived" | null (显示所有)
    function renderAttrSection(attrs, category, title) {
        var items = [];
        Object.keys(attrs).sort().forEach(function (k) {
            if (k.endsWith("_max")) return; // 跳过 max 后缀
            var meta = attributeMeta[k];
            if (category && meta && meta.category !== category) return;
            var name = meta ? meta.display_name : k;
            var val = attrs[k];
            var maxVal = attrs[k + "_max"];
            var display = maxVal !== undefined ? val + '/' + maxVal : val;
            items.push('<div class="detail-item"><span class="detail-label">' + name + ':</span> ' + display + '</div>');
        });
        if (items.length === 0) {
            return '<div class="detail-section"><div class="detail-title">' + title + '</div><div style="color: var(--text-subtle); font-size: 13px;">暂无数据</div></div>';
        }
        return '<div class="detail-section"><div class="detail-title">' + title + '</div><div class="detail-grid">' + items.join('') + '</div></div>';
    }

    return '<div class="basic-info-grid">' +
        '<div class="detail-section">' +
        '<div class="detail-title">基本信息 <span class="detail-label">ID:</span> <span style="font-family: monospace; font-size: 12px;">' + agent.id + '</span></div>' +
        '<div class="detail-grid">' +
        '<div class="detail-item"><span class="detail-label">位置:</span> ' + getLocationName(agent.location) + '</div>' +
        '<div class="detail-item"><span class="detail-label">状态:</span> <span class="status-badge ' + (agent.is_alive ? "status-alive" : "status-dead") + '">' + (agent.is_alive ? "存活" : "死亡") + '</span></div>' +
        '<div class="detail-item"><span class="detail-label">创建时间:</span> ' + new Date(agent.created_at).toLocaleString() + '</div>' +
        '<div class="detail-item"><span class="detail-label">最后活跃:</span> ' + (agent.last_active ? new Date(agent.last_active).toLocaleString() : "从未") + '</div>' +
        '</div></div>' +

        renderAttrSection(agent.attributes || {}, "status", "生理状态") +
        renderAttrSection(agent.attributes || {}, "primary", "先天属性") +
        renderAttrSection(agent.attributes || {}, "derived", "派生属性") +

        '<div class="detail-section">' +
        '<div class="detail-title">人设 Prompt</div>' +
        '<div style="font-size: 12px; color: var(--text-secondary); background: var(--bg-level-1); padding: 10px; border-radius: var(--radius-sm); line-height: 1.4; max-height: 150px; overflow-y: auto;">' +
        escapeHtml(agent.system_prompt || "") +
        '</div></div>' +
        '</div>';
}

function renderExperiences(data) {
    if (!data.experiences || data.experiences.length === 0) {
        return '<div style="text-align: center; padding: 40px; color: var(--text-subtle);">暂无经历记录</div>';
    }

    var expHtml = data.experiences.map(function (exp) {
        var time = exp.created_at ? new Date(exp.created_at).toLocaleString() : "Tick #" + exp.tick_id;
        var metadata = exp.soul_cycle_metadata;

        // 优先使用三魂完整链路渲染（cycles 数组存在且非空）
        if (metadata?.cycles?.length > 0) {
            return renderTickCard(exp, metadata, time);
        }

        // Fallback: 显示基本信息（当 soul_cycle_metadata 为空时）
        var actionType = getActionTypeDisplay(exp.action_type || '');
        var resultText = exp.result === 'success' ? '成功' : (exp.result === 'failed' ? '失败' : (exp.result || '-'));
        var resultCls = exp.result === 'success' ? 'approved' : (exp.result === 'failed' ? 'rejected' : '');

        var html = '<div class="tick-card">' +
            '<div class="tick-card-header">' +
            '<span class="tick-badge">T' + (exp.tick_id || '-') + '</span>' +
            '<span class="tick-real-time">' + time + '</span>' +
            '</div>';

        html += '<div class="tick-section"><div class="tick-section-title">行动</div>';

        // 伪装人魂：叙事与思考
        var renhunHtml = '';
        if (exp.narrative) renhunHtml += '<div class="soul-text">' + escapeHtml(exp.narrative) + '</div>';
        if (exp.thought_log) renhunHtml += '<div class="soul-thought">' + escapeHtml(exp.thought_log) + '</div>';
        if (renhunHtml) html += renderServerSoulInline('人魂', { narrative: exp.narrative, thought_log: exp.thought_log }, 'renhun');

        // 伪装天魂：审查结果
        var tianhunHtml = '';
        if (exp.result) {
            tianhunHtml += '<div class="soul-result ' + resultCls + '">' + resultText + '</div>';
        }
        if (tianhunHtml) html += '<div class="exp-tianhun"><span class="exp-soul-label">天魂</span><div class="exp-soul-content">' + tianhunHtml + '</div></div>';

        // 伪装地魂：动作
        html += '<div class="exp-action"><span class="exp-soul-label">地魂</span><div class="exp-soul-content">';
        html += '<div class="soul-text">' + escapeHtml(actionType) + '</div>';
        html += '</div></div>';

        html += '</div></div>';
        return html;
    }).join("");

    return '<div class="experience-list">' + expHtml + '</div>';
}

// 渲染 Tick 卡片（三魂完整链路，与 agent 端保持一致）
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
    html += '<div class="tick-attempts-container">';
    attempts.forEach(function(attempt, idx) {
        html += '<div class="tick-attempt-box">';
        if (attempts.length > 1) {
            html += '<div class="tick-attempt-label">第 ' + (idx + 1) + ' 次尝试</div>';
        }
        html += renderServerSoulInline('人魂', attempt.renhun, 'renhun');
        html += renderServerSoulInline('天魂', attempt.tianhun, 'tianhun');
        if (attempt.final_intent) {
            html += renderServerSoulInline('地魂', attempt.final_intent, 'action');
        }
        html += '</div>';
    });
    html += '</div></div>';

    // 即时分区
    if (immediate.length > 0) {
        html += '<div class="tick-section tick-section-immediate"><div class="tick-section-title">即时</div>';
        immediate.forEach(function(imm) {
            var speakerLabel = imm.from_agent_name ? escapeHtml(imm.from_agent_name) + ' ' : '';
            html += '<div class="imm-item">' +
                '<div class="exp-immediate"><span class="exp-soul-label">即时</span>' +
                '<span class="exp-soul-content">' + speakerLabel + escapeHtml(getActionTypeDisplay(imm.action_type)) +
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

// 天魂三层审查标签中文映射
var LAYER_NAMES = { layer1: '动作审查', layer2: '规则校验', layer3: '意图审查' };

// 渲染单魂/行动内联区块（server 版本，与 agent 端保持一致）
function renderServerSoulInline(label, data, type) {
    if (!data) return '';
    var html = '<div class="exp-' + type + '"><span class="exp-soul-label">' + label + '</span><div class="exp-soul-content">';

    if (type === 'renhun') {
        // 人魂：叙事 + 思考过程
        if (data.narrative) html += '<div class="soul-text">' + escapeHtml(data.narrative) + '</div>';
        if (data.thought_log) html += '<div class="soul-thought">' + escapeHtml(data.thought_log) + '</div>';
    } else if (type === 'tianhun') {
        // 天魂：审查结果 + 三层详情 + 理由
        if (data.result) {
            var isApproved = data.result === 'approved';
            html += '<div class="soul-result ' + (isApproved ? 'approved' : 'rejected') + '">' +
                (isApproved ? '通过' : '驳回') + '</div>';
        }
        if (data.layers && data.layers.length > 0) {
            html += '<div class="soul-layers">';
            data.layers.forEach(function(l) {
                var cls = l.passed ? 'passed' : 'failed';
                var name = LAYER_NAMES[l.layer] || l.layer;
                html += '<span class="soul-layer-tag ' + cls + '">' + name +
                    (l.passed ? '' : ': ' + escapeHtml(l.detail || '')) + '</span>';
            });
            html += '</div>';
        }
        if (data.reason) html += '<div class="soul-reason">' + escapeHtml(data.reason) + '</div>';
        if (data.narrative) html += '<div class="soul-narrative">' + escapeHtml(data.narrative) + '</div>';
    } else if (type === 'action') {
        // 地魂：最终行动，speak/whisper 特殊展示
        if (data.action_type) {
            var at = data.action_type;
            var ad = (data.action_data && typeof data.action_data === 'object') ? data.action_data : {};
            var content = ad.content || '';
            var targetId = ad.target_agent_id;

            if (at === 'speak') {
                var speakLabel = targetId ? '对某人说话' : '向众人说话';
                html += '<div class="soul-text">' + escapeHtml(speakLabel) + '："' + escapeHtml(content) + '"</div>';
            } else if (at === 'whisper') {
                var targetName = targetId || '某人';
                // 尝试从 allAgents 列表中查找目标名称（如果存在）
                if (typeof allAgents !== 'undefined' && allAgents && allAgents.length > 0) {
                    var found = allAgents.find(function(a) { return a.id === targetId || a.agent_id === targetId; });
                    if (found && found.name) targetName = found.name;
                }
                html += '<div class="soul-text">向 ' + escapeHtml(targetName) + ' 密语："' + escapeHtml(content) + '"</div>';
            } else if (at === 'shout') {
                html += '<div class="soul-text">大声喊道："' + escapeHtml(content) + '"</div>';
            } else {
                html += '<div class="soul-text">' + escapeHtml(getActionTypeDisplay(at));
                if (Object.keys(ad).length > 0) {
                    html += ' <span class="soul-params">' + escapeHtml(JSON.stringify(ad)) + '</span>';
                }
                html += '</div>';
            }
        }
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

// ============================================================================
// Inventory Management (grant-items UI)
// ============================================================================

async function loadAllItems() {
    if (allItemsList) return allItemsList;
    try {
        var res = await fetch("/api/dashboard/items");
        if (res.ok) {
            allItemsList = await res.json();
        }
    } catch (e) {
        console.error("Failed to load items list:", e);
    }
    return allItemsList || [];
}

async function renderInventoryManage(agent) {
    var inventoryHtml = (!agent.inventory || agent.inventory.length === 0)
        ? '<div style="color: var(--text-subtle); font-size: 13px; text-align: center; padding: 10px;">空空如也</div>'
        : '<div class="inventory-grid">' +
        agent.inventory.map(function (item) {
            return '<div class="inventory-item">' +
                '<div style="margin-bottom: 2px;">' + escapeHtml(item.name) + '</div>' +
                '<div style="font-weight: 600; color: var(--text-secondary);">x' + item.count + '</div></div>';
        }).join("") + '</div>';

    var html = '<div class="detail-section">' +
        '<div class="detail-title">当前背包</div>' +
        inventoryHtml +
        '</div>';

    // grant-items UI: 仅 write token 可见
    if (authTokenType === "write") {
        var items = await loadAllItems();
        
        var inputHtml = '';
        if (items.length > 0) {
            var optionsHtml = items.map(function (item) {
                return '<option value="' + escapeHtml(item.item_id) + '">' +
                    escapeHtml(item.name) + ' (' + escapeHtml(item.item_type) + ')</option>';
            }).join("");
            inputHtml = '<select id="grant-item-select" class="form-input" style="width: 100%;">' + optionsHtml + '</select>';
        } else {
            inputHtml = '<input type="text" id="grant-item-select" class="form-input" placeholder="输入物品 ID..." style="width: 100%;" />';
        }

        html += '<div class="detail-section">' +
            '<div class="detail-title">注入物品</div>' +
            '<div style="display: flex; gap: 10px; align-items: flex-end; flex-wrap: wrap;">' +
            '<div style="flex: 1; min-width: 150px;">' +
            '<label style="font-size: 12px; color: var(--text-secondary); display: block; margin-bottom: 4px;">物品</label>' +
            inputHtml +
            '</div>' +
            '<div style="width: 100px;">' +
            '<label style="font-size: 12px; color: var(--text-secondary); display: block; margin-bottom: 4px;">数量</label>' +
            '<input type="number" id="grant-item-qty" class="form-input" value="1" min="1" max="9999" style="width: 100%;" />' +
            '</div>' +
            '<button class="btn btn-success" onclick="addGrantItem()" id="grant-item-btn">添加</button>' +
            '</div>' +
            '<div id="grant-items-list" style="margin-top: 10px;"></div>' +
            '</div>';
    } else {
        html += '<div class="detail-section">' +
            '<div style="font-size: 12px; color: var(--text-subtle); text-align: center; padding: 10px;">需要编辑权限才能注入物品</div>' +
            '</div>';
    }

    // Vendor 补货规则配置（仅 write token 可编辑）
    html += await renderVendorRefillSection(agent.agent_id);

    return html;
}

// 待注入物品列表
var grantItemsBuffer = [];

function addGrantItem() {
    var select = document.getElementById("grant-item-select");
    var qtyInput = document.getElementById("grant-item-qty");
    if (!select || !qtyInput) return;

    var itemId = select.value;
    var qty = parseInt(qtyInput.value, 10);
    if (!itemId || isNaN(qty) || qty <= 0) {
        showToast("请选择物品并输入有效数量", "error");
        return;
    }

    var itemName = select.tagName === "SELECT" ? select.options[select.selectedIndex].text : itemId;
    grantItemsBuffer.push({ item_id: itemId, name: itemName, quantity: qty });
    renderGrantItemsBuffer();
}

function removeGrantItem(index) {
    grantItemsBuffer.splice(index, 1);
    renderGrantItemsBuffer();
}

function renderGrantItemsBuffer() {
    var container = document.getElementById("grant-items-list");
    if (!container) return;

    if (grantItemsBuffer.length === 0) {
        container.innerHTML = "";
        return;
    }

    var html = '<div style="border: 1px solid var(--border-color); border-radius: var(--radius-sm); padding: 8px;">' +
        '<div style="font-size: 12px; color: var(--text-secondary); margin-bottom: 6px;">待注入列表:</div>' +
        grantItemsBuffer.map(function (item, idx) {
            return '<div style="display: flex; justify-content: space-between; align-items: center; padding: 4px 0; border-bottom: 1px solid var(--border-color);">' +
                '<span style="font-size: 13px;">' + escapeHtml(item.name) + ' x' + item.quantity + '</span>' +
                '<button class="btn btn-secondary" style="padding: 2px 8px; font-size: 11px;" onclick="removeGrantItem(' + idx + ')">移除</button>' +
                '</div>';
        }).join("") +
        '<button class="btn btn-success" style="margin-top: 8px; width: 100%;" onclick="grantItemsToAgent()">确认注入 (' + grantItemsBuffer.length + ' 种物品)</button>' +
        '</div>';
    container.innerHTML = html;
}

async function grantItemsToAgent() {
    if (!currentModalAgentId) return;

    var select = document.getElementById("grant-item-select");
    var qtyInput = document.getElementById("grant-item-qty");
    if (!select || !qtyInput) return;

    // 如果 buffer 为空，自动添加当前选中的物品
    if (grantItemsBuffer.length === 0) {
        var itemId = select.value;
        var qty = parseInt(qtyInput.value, 10);
        if (!itemId || isNaN(qty) || qty <= 0) {
            showToast("请选择物品并输入有效数量", "error");
            return;
        }
        grantItemsBuffer.push({ item_id: itemId, quantity: qty });
    }

    var items = grantItemsBuffer.map(function (item) {
        return { item_id: item.item_id, quantity: item.quantity };
    });

    try {
        var res = await fetch("/api/v1/agent/grant-items", {
            method: "POST",
            headers: getAuthHeaders(),
            body: JSON.stringify({
                agent_id: currentModalAgentId,
                items: items,
            }),
        });

        if (res.ok) {
            var data = await res.json();
            if (data.success) {
                showToast("成功注入 " + data.granted_count + " 个物品", "success");
                grantItemsBuffer = [];
                renderGrantItemsBuffer();
                // 仅刷新库存 tab
                var agentRes = await fetch("/api/dashboard/agent/" + currentModalAgentId, { headers: getAuthHeaders() });
                if (agentRes.ok) {
                    var agent = await agentRes.json();
                    document.getElementById("modal-tab-inventory").innerHTML = await renderInventoryManage(agent);
                }
            } else {
                showToast("注入失败: " + data.message, "error");
            }
        } else {
            var errData = await res.json().catch(function () { return null; });
            var errMsg = errData && errData.message ? errData.message : "HTTP " + res.status;
            showToast("注入失败: " + errMsg, "error");
        }
    } catch (e) {
        console.error("Grant items failed:", e);
        showToast("网络请求失败", "error");
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

// ============================================================================
// Vendor 补货规则管理
// ============================================================================

async function renderVendorRefillSection(agentId) {
    var rules = [];
    try {
        var res = await fetch("/api/dashboard/agent/" + agentId + "/vendor-refill", { headers: getAuthHeaders() });
        if (res.ok) rules = await res.json();
    } catch (e) { /* ignore */ }

    var isWrite = authTokenType === "write";
    var html = '<div class="detail-section">' +
        '<div class="detail-title">自动补货规则</div>';

    if (rules.length === 0) {
        html += '<div style="color: var(--text-subtle); font-size: 13px; text-align: center; padding: 10px;">未配置补货规则</div>';
    } else {
        html += '<table style="width:100%; font-size:13px; border-collapse:collapse;">' +
            '<tr style="color:var(--text-secondary); border-bottom:1px solid var(--border-color);">' +
            '<th style="text-align:left; padding:4px 8px;">物品</th>' +
            '<th style="text-align:center; padding:4px;">触发</th>' +
            '<th style="text-align:center; padding:4px;">补到</th>' +
            '<th style="text-align:center; padding:4px;">预算%</th>' +
            '<th style="text-align:center; padding:4px;">状态</th>' +
            (isWrite ? '<th style="text-align:center; padding:4px;">操作</th>' : '') +
            '</tr>';
        rules.forEach(function (r) {
            var itemName = r.item_id;
            html += '<tr style="border-bottom:1px solid var(--border-color);">' +
                '<td style="padding:4px 8px;">' + escapeHtml(itemName) + '</td>' +
                '<td style="text-align:center; padding:4px;">' + r.threshold + '</td>' +
                '<td style="text-align:center; padding:4px;">' + r.refill_to + '</td>' +
                '<td style="text-align:center; padding:4px;">' + r.budget_ratio + '%</td>' +
                '<td style="text-align:center; padding:4px;">' + (r.enabled ? "启用" : "停用") + '</td>' +
                (isWrite ? '<td style="text-align:center; padding:4px;">' +
                    '<button class="btn btn-secondary" style="padding:2px 6px; font-size:11px;" onclick="deleteRefillRule(\'' + agentId + '\',\'' + escapeHtml(r.item_id) + '\')">删除</button>' +
                    '</td>' : '') +
                '</tr>';
        });
        html += '</table>';
    }

    if (isWrite) {
        var items = await loadAllItems();
        var inputHtml = '';
        if (items.length > 0) {
            var optionsHtml = items.map(function (item) {
                return '<option value="' + escapeHtml(item.item_id) + '">' + escapeHtml(item.name) + '</option>';
            }).join("");
            inputHtml = '<select id="refill-item-select" class="form-input" style="width:100%;">' + optionsHtml + '</select>';
        } else {
            inputHtml = '<input type="text" id="refill-item-select" class="form-input" placeholder="输入物品 ID..." style="width:100%;" />';
        }

        html += '<div style="display:flex; gap:8px; align-items:flex-end; margin-top:10px; flex-wrap:wrap;">' +
            '<div style="flex:1; min-width:120px;">' +
            '<label style="font-size:11px; color:var(--text-secondary); display:block; margin-bottom:2px;">物品</label>' +
            inputHtml + '</div>' +
            '<div style="width:70px;">' +
            '<label style="font-size:11px; color:var(--text-secondary); display:block; margin-bottom:2px;">触发</label>' +
            '<input type="number" id="refill-threshold" class="form-input" value="10" min="1" style="width:100%;" /></div>' +
            '<div style="width:70px;">' +
            '<label style="font-size:11px; color:var(--text-secondary); display:block; margin-bottom:2px;">补到</label>' +
            '<input type="number" id="refill-refill-to" class="form-input" value="50" min="1" style="width:100%;" /></div>' +
            '<div style="width:70px;">' +
            '<label style="font-size:11px; color:var(--text-secondary); display:block; margin-bottom:2px;">预算%</label>' +
            '<input type="number" id="refill-budget" class="form-input" value="50" min="1" max="100" style="width:100%;" /></div>' +
            '<button class="btn btn-success" style="padding:4px 12px;" onclick="addRefillRule(\'' + agentId + '\')">添加</button>' +
            '</div>';
    }

    html += '</div>';
    return html;
}

async function addRefillRule(agentId) {
    var itemId = document.getElementById("refill-item-select").value;
    var threshold = parseInt(document.getElementById("refill-threshold").value, 10);
    var refillTo = parseInt(document.getElementById("refill-refill-to").value, 10);
    var budget = parseInt(document.getElementById("refill-budget").value, 10);

    if (!itemId || isNaN(threshold) || isNaN(refillTo) || isNaN(budget) || threshold <= 0 || refillTo <= threshold || budget <= 0 || budget > 100) {
        showToast("参数不合法: 触发>0, 补到>触发, 预算1-100", "error");
        return;
    }

    try {
        var res = await fetch("/api/dashboard/agent/" + agentId + "/vendor-refill", {
            method: "PUT",
            headers: getAuthHeaders(),
            body: JSON.stringify({ item_id: itemId, threshold: threshold, refill_to: refillTo, budget_ratio: budget }),
        });
        if (res.ok) {
            showToast("补货规则已添加", "success");
            var agentRes = await fetch("/api/dashboard/agent/" + currentModalAgentId, { headers: getAuthHeaders() });
            if (agentRes.ok) {
                var agent = await agentRes.json();
                document.getElementById("modal-tab-inventory").innerHTML = await renderInventoryManage(agent);
            }
        } else {
            var err = await res.json().catch(function () { return {}; });
            showToast("添加失败: " + (err.error || "HTTP " + res.status), "error");
        }
    } catch (e) {
        showToast("网络请求失败", "error");
    }
}

async function deleteRefillRule(agentId, itemId) {
    if (!confirm("确定删除 " + itemId + " 的补货规则？")) return;
    try {
        var res = await fetch("/api/dashboard/agent/" + agentId + "/vendor-refill/" + encodeURIComponent(itemId), {
            method: "DELETE",
            headers: getAuthHeaders(),
        });
        if (res.ok) {
            showToast("补货规则已删除", "success");
            var agentRes = await fetch("/api/dashboard/agent/" + currentModalAgentId, { headers: getAuthHeaders() });
            if (agentRes.ok) {
                var agent = await agentRes.json();
                document.getElementById("modal-tab-inventory").innerHTML = await renderInventoryManage(agent);
            }
        } else {
            showToast("删除失败", "error");
        }
    } catch (e) {
        showToast("网络请求失败", "error");
    }
}
