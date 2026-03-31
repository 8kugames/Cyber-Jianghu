// ============================================================================
// Tab Navigation
// ============================================================================

function switchTab(tabId) {
    document.querySelectorAll(".nav-tab").forEach(function (t) { t.classList.remove("active"); });
    document.querySelectorAll(".view-section").forEach(function (c) { c.classList.remove("active"); });

    var tabEl = Array.from(document.querySelectorAll(".nav-tab")).find(function (t) {
        return t.innerText.includes(tabId === "dashboard" ? "监控" : "配置");
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
        var res = await fetch("/health");
        if (!res.ok) return;
        var data = await res.json();
        var el = document.getElementById("server-version");
        if (el && data && data.version) {
            el.textContent = "v" + data.version;
        }
    } catch (e) {
        console.warn("Failed to load server version", e);
    }
}

// ============================================================================
// Bootstrap
// ============================================================================

async function bootstrap() {
    await Promise.all([initLocationMapping(), initAttributeMeta()]);
    await initAuth();
    loadStats();
    startAutoRefresh();
    loadServerVersion();
}

bootstrap();
