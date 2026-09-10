// ============================================================================
// Global State & Constants
// ============================================================================
const API = {
    BASE: "/api/dashboard",
    V1: "/api/v1",
    ADMIN: "/api/admin",
    CONFIG: "/api/config",
    HEALTH: "/health"
};

var allAgents = [];
var statusConfigs = {}; // data-driven status config
var attributeMeta = {}; // key -> { display_name, category } data-driven attribute config
var locationNames = {};
var authToken = localStorage.getItem("admin_token") || "";
var currentFile = null;
var authTokenType = localStorage.getItem("admin_token_type") || null;
var refreshInterval = null;
var smoothTimeConfig = null;
var smoothTimeAnimationId = null;

// 地魂 action_type 分类常量（数据驱动：说话类有 content 字段）
var SPEAK_TYPES = { "说话": true, speak: true };

// 说话动作多因子判断：action_type + channel
function isSpeakAtype(at, ad) {
    return at === "说话" && (!ad || !ad.channel || ad.channel === "public");
}
function isWhisperAtype(at, ad) {
    return at === "说话" && ad && ad.channel === "private";
}
function isShoutAtype(at, ad) {
    return at === "说话" && ad && ad.channel === "broadcast";
}

// authVerified may be declared in auth.js (loaded after utils.js), so window guard needed
var authVerified = typeof window.authVerified !== "undefined" ? window.authVerified : false;

// 直达链接支持（P1-20 安全边界内）：只接受 hash fragment 形式（#token=xxx）。
// hash 不会随请求发送到服务器 —— 不进 access log / CDN 缓存 / Referer；
// 提取后立即 replaceState 抹除，浏览器历史与复制出的链接均不再含 token。
// ?token=xxx 仍然禁止（token 会进入服务器日志与代理缓存，P1-20 移除）。
var hashBootstrapToken = "";
(function extractHashToken() {
    var hash = window.location.hash;
    if (hash.length > 1) {
        var t = new URLSearchParams(hash.slice(1)).get("token");
        if (t) {
            hashBootstrapToken = t;
            history.replaceState(null, "", window.location.pathname + window.location.search);
        }
    }
})();

if (new URLSearchParams(window.location.search).has("token")) {
    console.warn(
        "[Security] URL ?token= has been disabled (it leaks into server logs / CDN caches). " +
        "Use the #token= fragment form instead."
    );
    showToast("已禁用 ?token= 链接，请改用 #token= 直达链接", "error");
}

// ============================================================================
// Utility Functions
// ============================================================================
function escapeHtml(text) {
    if (text === null || text === undefined) return '';
    var div = document.createElement('div');
    div.textContent = String(text);
    return div.innerHTML;
}

function showToast(message, type) {
    type = type || "success";
    var toast = document.getElementById("toast");
    if (!toast) {
        toast = document.createElement("div");
        toast.id = "toast";
        toast.className = "toast";
        document.body.appendChild(toast);
    }
    toast.textContent = message;
    toast.className = "toast toast-" + type + " show";
    setTimeout(function () {
        toast.classList.remove("show");
    }, 3000);
}

function formatDuration(seconds) {
    var h = Math.floor(seconds / 3600);
    var m = Math.floor((seconds % 3600) / 60);
    return h + "小时 " + m + "分";
}

function formatTimeAgo(timestamp) {
    if (!timestamp) return "未知";
    var date = new Date(timestamp);
    var seconds = Math.floor((new Date() - date) / 1000);
    if (seconds < 60) return "刚刚";
    if (seconds < 3600) return Math.floor(seconds / 60) + "分钟前";
    if (seconds < 86400) return Math.floor(seconds / 3600) + "小时前";
    return Math.floor(seconds / 86400) + "天前";
}

function formatLastActive(date) {
    if (!date) return "-";
    var now = new Date();
    var then = new Date(date);
    var diffMs = now - then;
    var diffMins = Math.floor(diffMs / 60000);
    if (diffMins < 1) return "刚刚";
    if (diffMins < 60) return diffMins + "分钟前";
    var diffHours = Math.floor(diffMins / 60);
    if (diffHours < 24) return diffHours + "小时前";
    var diffDays = Math.floor(diffHours / 24);
    if (diffDays < 7) return diffDays + "天前";
    return then.toLocaleDateString();
}

function formatCreatedAt(date) {
    if (!date) return "-";
    return new Date(date).toLocaleDateString();
}

function getStatusText(status) {
    // Data-driven: prefer config, fallback
    if (statusConfigs[status]) {
        return statusConfigs[status].display_name;
    }
    var fallback = {
        "online": "在线", "offline": "离线", "dead": "死亡",
        "retired": "归隐", "active": "活跃"
    };
    return fallback[status] || status;
}

function getLocationName(locId) {
    if (locationNames[locId]) return locationNames[locId];
    return locId;
}

async function initLocationMapping() {
    try {
        var res = await apiFetch(API.CONFIG + "/locations.yaml");
        if (res.ok) {
            var data = await res.json();
            var doc = jsyaml.load(data.content);
            if (doc && doc.data && doc.data.nodes) {
                var nodeMap = {};
                doc.data.nodes.forEach(function (node) { nodeMap[node.node_id] = node; });
                doc.data.nodes.forEach(function (node) {
                    if (node.parent_id && nodeMap[node.parent_id] && node.type === "sub_scene") {
                        locationNames[node.node_id] = nodeMap[node.parent_id].name + " - " + node.name;
                    } else {
                        locationNames[node.node_id] = node.name;
                    }
                });
            }
        }
    } catch (e) {
        if (e.name !== "ApiError") {
            console.error("Failed to load locations mapping", e);
        }
    }
}

async function initAttributeMeta() {
    try {
        var res = await apiFetch(API.CONFIG + "/attributes.yaml");
        if (res.ok) {
            var data = await res.json();
            var doc = jsyaml.load(data.content);
            if (doc && doc.data) {
                // primary attributes
                if (doc.data.primary && doc.data.primary.attributes) {
                    Object.keys(doc.data.primary.attributes).forEach(function (key) {
                        var attr = doc.data.primary.attributes[key];
                        attributeMeta[key] = {
                            display_name: attr.display_name,
                            category: "primary"
                        };
                    });
                }
                // status attributes
                if (doc.data.status && doc.data.status.attributes) {
                    Object.keys(doc.data.status.attributes).forEach(function (key) {
                        var attr = doc.data.status.attributes[key];
                        attributeMeta[key] = {
                            display_name: attr.display_name,
                            category: "status"
                        };
                    });
                }
                // derived attributes
                if (doc.data.derived && doc.data.derived.attributes) {
                    Object.keys(doc.data.derived.attributes).forEach(function (key) {
                        var attr = doc.data.derived.attributes[key];
                        attributeMeta[key] = {
                            display_name: attr.display_name,
                            category: "derived"
                        };
                    });
                }
            }
        }
    } catch (e) {
        if (e.name !== "ApiError") {
            console.error("Failed to load attribute meta", e);
        }
    }
}

function formatAttributeShort(key, value, attrs) {
    var meta = attributeMeta[key];
    var name = meta ? meta.display_name : key;
    var maxKey = key + "_max";
    if (attrs && attrs[maxKey] !== undefined) {
        return name + ":" + value + "/" + attrs[maxKey];
    }
    return name + ":" + value;
}

// 天魂三层审查标签中文映射（history.html + agents.js 共用）
var LAYER_NAMES = {
    layer0: "目标校验",
    layer1: "动作审查",
    layer2: "规则校验",
    layer3: "意图审查",
};

// 从 API 获取天魂层展示名（数据驱动），返回 LAYER_NAMES 的超集
// 优先从 souls.yaml layer_display 配置读取，失败时降级到 LAYER_NAMES
var _layerDisplayCache = null;
async function getLayerDisplay() {
    if (_layerDisplayCache) return _layerDisplayCache;
    try {
        var resp = await fetch('/api/dashboard/layer-display');
        if (resp.ok) {
            var apiMap = await resp.json();
            _layerDisplayCache = Object.assign({}, LAYER_NAMES, apiMap);
            return _layerDisplayCache;
        }
    } catch (e) { console.warn("layer-display API 不可用，降级到硬编码默认值", e); }
    _layerDisplayCache = LAYER_NAMES;
    return _layerDisplayCache;
}

// 取动作 source_type 来源中文化（UI 标签，由 QuData.source_type 固定枚举决定）
var SOURCE_TYPE_NAMES = {
    ground: "地面",
    agent: "角色",
    resource: "资源点",
};

// ============================================================================
// Display Map（展示名映射缓存）
// 经历日志渲染前必须 await loadDisplayMap() 就绪，用于翻译 target_agent_id。
// 数据源：后端 /api/dashboard/display-map（items.yaml + agents 表），单一权威源。
// ============================================================================
var displayMapCache = { agents: {}, items: {}, item_uuids: {}, recipes: {}, recipe_uuids: {},
    itemsByUuid: {}, recipesByUuid: {}, _loaded: false, _promise: null };

function loadDisplayMap() {
    if (displayMapCache._loaded || displayMapCache._promise) return displayMapCache._promise;
    displayMapCache._promise = apiFetch(API.BASE + "/display-map")
        .then(function (r) { return r.json(); })
        .then(function (data) {
            displayMapCache.agents = data.agents || {};
            displayMapCache.items = data.items || {};
            displayMapCache.item_uuids = data.item_uuids || {};
            displayMapCache.recipes = data.recipes || {};
            displayMapCache.recipe_uuids = data.recipe_uuids || {};
            // uuid 反查表：新协议下动作数据携带完整 uuid，需反解回裸 id 取名
            displayMapCache.itemsByUuid = {};
            Object.keys(displayMapCache.item_uuids).forEach(function (rawId) {
                displayMapCache.itemsByUuid[displayMapCache.item_uuids[rawId]] = rawId;
            });
            displayMapCache.recipesByUuid = {};
            Object.keys(displayMapCache.recipe_uuids).forEach(function (rawId) {
                displayMapCache.recipesByUuid[displayMapCache.recipe_uuids[rawId]] = rawId;
            });
            displayMapCache._loaded = true;
        })
        .catch(function (e) {
            console.warn("加载展示名映射失败:", e);
            displayMapCache._promise = null; // 允许重试
        });
    return displayMapCache._promise;
}

// ============================================================================
// 统一展示格式：{名}[{短uuid 前 8 位}]
// 与服务端 crate::display 同源同格式，任何页面禁止自行拼接。
// ============================================================================

// 纯格式化：已有 name + id 在手时直接用（无映射查表）
function formatNameId(name, id) {
    var short = id ? String(id).substring(0, 8) : "";
    return (name || "未知") + "[" + short + "]";
}

// 解析 target_agent_id → 角色展示名（history.html + agents.js + health.js 共用）
// 优先查 displayMapCache.agents（权威映射），其次 allAgentsMap，兜底未知角色[短ID]。
function resolveTargetName(targetId) {
    if (!targetId) return "某人";
    // 权威映射优先（覆盖已死亡/离线角色）
    if (displayMapCache.agents[targetId]) {
        return formatNameId(displayMapCache.agents[targetId], targetId);
    }
    if (typeof allAgentsMap !== "undefined" && allAgentsMap && allAgentsMap[targetId]) {
        var agent = allAgentsMap[targetId];
        return formatNameId(agent.name, targetId);
    }
    // 兜底：仍未命中映射，给出中文化提示而非裸 UUID，便于排障
    return "未知角色[" + targetId.substring(0, 8) + "]";
}

// 解析 recipe_id → 配方展示名（制造/传授动作渲染共用）
// 兼容两种引用：完整 uuid（新协议）与裸 recipe_id（历史记录）；未知时退化为原始引用
function resolveRecipeName(recipeId) {
    if (!recipeId) return "";
    var rawId = recipeId;
    if (!displayMapCache.recipes[recipeId]) {
        rawId = displayMapCache.recipesByUuid[recipeId] || recipeId;
    }
    if (displayMapCache.recipes[rawId]) {
        var uuid = displayMapCache.recipe_uuids[rawId] || "";
        return displayMapCache.recipes[rawId] + "[" + (uuid ? uuid.substring(0, 8) : rawId) + "]";
    }
    return recipeId;
}

// 物品引用翻译（兼容 uuid 新协议与历史裸 item_id）
function resolveItemName(itemRef) {
    if (!itemRef) return "";
    var rawId = displayMapCache.itemsByUuid[itemRef] || itemRef;
    if (displayMapCache.items[rawId]) {
        var itemUuid = displayMapCache.item_uuids[rawId] || "";
        return displayMapCache.items[rawId] + "[" + (itemUuid ? itemUuid.substring(0, 8) : rawId) + "]";
    }
    if (UUID_RE.test(itemRef)) return "未知物品[" + itemRef.substring(0, 8) + "]";
    return itemRef;
}

// 来源类型中文化（UI 标签，非业务数据）
function resolveSourceName(sourceType) {
    if (!sourceType) return "";
    return SOURCE_TYPE_NAMES[sourceType] || sourceType;
}

// 判断字符串是否为 UUID 格式（用于识别 LLM 幻觉产生的无效 item_id）
var UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// ============================================================================
// 统一动作渲染器（替代 history.js renderSingleAction 和 agents.js 旧内联渲染）
// 返回 HTML-escaped 纯文本（不含标签），由调用方决定外层包裹。
// ============================================================================
function renderActionText(aType, aData) {
    if (!aData) aData = {};
    if (typeof aData === "string") { try { aData = JSON.parse(aData); } catch (e) { aData = {}; } }
    var content = aData.content || "";

    // 说话类：对目标 / 向在场众人 / 密语 / 大喊
    if (isSpeakAtype(aType, aData) && content)
        return "向在场众人说话：\"" + escapeHtml(content) + "\"";
    if (isWhisperAtype(aType, aData) && content)
        return "向" + escapeHtml(resolveTargetName(aData.target_agent_id)) + "密语：\"" + escapeHtml(content) + "\"";
    if (isShoutAtype(aType, aData) && content)
        return "大喊：\"" + escapeHtml(content) + "\"";

    var text = escapeHtml(getActionTypeDisplay(aType));

    // 物品类字段结构化展示
    if (aData.item_id) {
        text += " " + escapeHtml(resolveItemName(aData.item_id));
    }
    if (aData.quantity) text += " x" + aData.quantity;
    if (aData.recipe_id) text += " 配方：" + escapeHtml(resolveRecipeName(aData.recipe_id));
    if (aData.target_id) text += " → " + escapeHtml(resolveTargetName(aData.target_id));
    if (aData.source_id) {
        // source_id 语义随 source_type 变化：agent=角色，其余=物品
        var sourceDisplay = aData.source_type === "agent"
            ? resolveTargetName(aData.source_id)
            : resolveItemName(aData.source_id);
        text += "（来源：" + escapeHtml(sourceDisplay) + "）";
    }
    if (aData.recipient_id) {
        var recipientDisplay = aData.recipient_type === "agent"
            ? resolveTargetName(aData.recipient_id)
            : resolveItemName(aData.recipient_id);
        text += "（→" + escapeHtml(recipientDisplay) + "）";
    }
    if (aData.source_type && !aData.source_id) text += "（" + escapeHtml(resolveSourceName(aData.source_type)) + "）";
    if (aData.recipient_type && !aData.recipient_id) text += "（→" + escapeHtml(resolveSourceName(aData.recipient_type)) + "）";
    if (aData.content) text += " \"" + escapeHtml(content) + "\"";
    if (aData.target_agent_id) text += " → " + escapeHtml(resolveTargetName(aData.target_agent_id));
    if (aData.target_location) text += " → " + escapeHtml(aData.target_location);

    // 剩余未知字段：键值对中文化（非暴力 JSON.stringify）
    var known = ["content", "item_id", "quantity", "recipe_id", "source_type", "source_id",
                 "recipient_type", "recipient_id", "target_id", "target_agent_id", "target_location", "channel"];
    var extra = Object.keys(aData).filter(function (k) { return known.indexOf(k) === -1; });
    if (extra.length > 0) {
        text += " " + extra.map(function (k) {
            return escapeHtml(k) + ":" + escapeHtml(String(aData[k]).substring(0, 30));
        }).join("，");
    }
    return text;
}
