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

// authVerified may be declared in auth.js (loaded after utils.js), so window guard needed
var authVerified = typeof window.authVerified !== "undefined" ? window.authVerified : false;

// Check for token in URL
var urlParams = new URLSearchParams(window.location.search);
if (urlParams.has("token")) {
    authToken = urlParams.get("token");
    localStorage.setItem("admin_token", authToken);
    window.history.replaceState({}, document.title, window.location.pathname);
    // Resolve token type async; only mark as verified after server confirms token is valid
    fetch(API.ADMIN + "/session", { headers: { Authorization: "Bearer " + authToken } })
        .then(function (r) { return r.ok ? r.json() : {}; })
        .then(function (data) {
            if (data.authenticated) {
                authVerified = true;
                if (typeof window !== "undefined") window.authVerified = true;
            }
            if (data.token_type) {
                authTokenType = data.token_type;
                localStorage.setItem("admin_token_type", authTokenType);
            }
        })
        .catch(function (e) {
            console.error("[Session] URL token validation failed:", e);
        });
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
