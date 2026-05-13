// ============================================================================
// Stats & Dashboard Functions
// ============================================================================

async function loadStats() {
    try {
        var res = await apiFetch(API.BASE + "/stats");
        if (res.ok) {
            var data = await res.json();

            smoothTimeConfig = {
                tickId: data.current_tick_id,
                tickDurationSecs: data.tick_duration_secs,
                ticksPerHour: data.ticks_per_hour,
                season: data.game_time.season,
                lastSyncTime: Date.now(),
            };

            if (!smoothTimeAnimationId) startSmoothTimeAnimation();

            var renderCard = function (label, value, sub, id) {
                return '<div class="stat-card"' + (id ? ' id="' + id + '"' : '') + '>' +
                    '<div class="stat-label">' + escapeHtml(label) + '</div>' +
                    '<div class="stat-value">' + escapeHtml(String(value)) + '</div>' +
                    (sub ? '<div class="stat-sub">' + escapeHtml(String(sub)) + '</div>' : '') +
                    '</div>';
            };

            var statsHtml = [
                renderCard("在线 Agent", data.current_active_agents, "总注册: " + data.total_registered_agents),
                renderCard("服务器运行时间", formatDuration(data.server_uptime_secs), data.server_running_days + " 天"),
                renderCard("游戏时间", data.game_time.text, data.game_time.season + " | Tick: " + data.current_tick_id, "game-time-card"),
                renderCard("日活跃用户 (DAU)", data.dau),
                renderCard("3日活跃", data.active_3d),
                renderCard("7日活跃", data.active_7d),
                renderCard("月活跃用户 (MAU)", data.mau),
                renderCard("年度活跃 (YAU)", data.yau),
                renderCard("TPS (Ticks/Sec)", (1.0 / data.tick_duration_secs).toFixed(2), "Tick周期: " + data.tick_duration_secs + "s"),
            ].join("");

            document.getElementById("stats-grid").innerHTML = statsHtml;
            document.getElementById("world-overview").textContent = data.world_overview || "暂无世界事件日志...";

            await Promise.all([loadStatusConfigs(), loadActionTypeMap(), loadAllAgents()]);
        }
    } catch (e) {
        if (e.name !== "ApiError") {
            console.error("Failed to load stats", e);
        }
    }
}

// ============================================================================
// Smooth Time Animation
// ============================================================================
// 注意：.stat-value 在 loadStats 时已由 data.game_time.text 填充，无需动画更新
function startSmoothTimeAnimation() {
    if (smoothTimeAnimationId) cancelAnimationFrame(smoothTimeAnimationId);

    function updateSmoothTime() {
        if (!smoothTimeConfig) return;

        var card = document.getElementById("game-time-card");
        if (card) {
            var subEl = card.querySelector(".stat-sub");
            if (subEl) subEl.textContent = (smoothTimeConfig.season || "") + " | Tick: " + smoothTimeConfig.tickId;
        }
        smoothTimeAnimationId = requestAnimationFrame(updateSmoothTime);
    }
    updateSmoothTime();
}

function stopSmoothTimeAnimation() {
    if (smoothTimeAnimationId) {
        cancelAnimationFrame(smoothTimeAnimationId);
        smoothTimeAnimationId = null;
    }
}

// ============================================================================
// Auto Refresh
// ============================================================================

function toggleAutoRefresh() {
    var checkbox = document.getElementById("auto-refresh");
    if (checkbox.checked) startAutoRefresh();
    else stopAutoRefresh();
}

function startAutoRefresh() {
    if (refreshInterval) clearInterval(refreshInterval);
    refreshInterval = setInterval(loadStats, 5000);
}

function stopAutoRefresh() {
    if (refreshInterval) clearInterval(refreshInterval);
    stopSmoothTimeAnimation();
}
