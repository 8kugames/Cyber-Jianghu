// ============================================================================
// Tab Navigation
// ============================================================================

function switchTab(tabId) {
    document.querySelectorAll(".nav-tab").forEach(function (t) { t.classList.remove("active"); });
    document.querySelectorAll(".view-section").forEach(function (c) { c.classList.remove("active"); });

    var tabEl = Array.from(document.querySelectorAll(".nav-tab")).find(function (t) {
        return t.getAttribute("data-tab-id") === tabId;
    });
    if (tabEl) tabEl.classList.add("active");

    document.getElementById(tabId).classList.add("active");

    if (tabId === "dashboard") {
        loadStats();
        startAutoRefresh();
    } else {
        stopAutoRefresh();
        stopSmoothTimeAnimation();
        loadConfigList();
    }
}

// ============================================================================
// World Overview Panel
// ============================================================================

function toggleWorldOverview() {
    var layout = document.querySelector(".dashboard-layout");
    if (!layout) return;

    var collapsed = layout.classList.toggle("world-overview-collapsed");
    var toggleBtn = document.getElementById("world-overview-toggle");
    if (toggleBtn) {
        toggleBtn.textContent = collapsed ? "«" : "»";
        toggleBtn.setAttribute("aria-expanded", collapsed ? "false" : "true");
        toggleBtn.setAttribute("title", collapsed ? "展开" : "最小化");
    }
}

// ============================================================================
// Version Badge
// ============================================================================

async function loadServerVersion() {
    try {
        var res = await apiFetch(API.HEALTH);
        if (!res.ok) return;
        var data = await res.json();
        var badge = document.getElementById("server-version");
        if (badge && data.version) {
            badge.textContent = "v" + data.version;
            badge.title = "Build: " + (data.build_timestamp || "Unknown");
        }
    } catch (e) {
        if (e.name !== "ApiError") {
            console.warn("Failed to load server version:", e);
        }
    }
}

// ============================================================================
// Bootstrap
// ============================================================================

async function bootstrap() {
    initAuth();
    if (authToken) {
        await Promise.all([initLocationMapping(), initAttributeMeta()]);
        loadStats();
        startAutoRefresh();
    }
    loadServerVersion();
}

bootstrap();
