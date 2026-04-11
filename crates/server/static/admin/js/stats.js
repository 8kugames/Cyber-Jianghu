// ============================================================================
// Stats & Dashboard Functions
// ============================================================================

async function loadStats() {
    try {
        var res = await fetch("/api/dashboard/stats", { headers: getAuthHeaders() });
        if (handleAuthError(res)) return;
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
                '<div class="stat-label">' + label + '</div>' +
                '<div class="stat-value">' + value + '</div>' +
                (sub ? '<div class="stat-sub">' + sub + '</div>' : '') +
                '</div>';
        };

        var statsHtml = [
            renderCard("在线 Agent", data.current_active_agents, "总注册: " + data.total_registered_agents),
            renderCard("服务器运行时间", formatDuration(data.server_uptime_secs), data.server_running_days + " 天"),
            renderCard("游戏时间", formatGameTimeDate(data.game_time), data.game_time.season + " | Tick: " + data.current_tick_id, "game-time-card"),
            renderCard("日活跃用户 (DAU)", data.dau),
            renderCard("月活跃用户 (MAU)", data.mau),
            renderCard("TPS (Ticks/Sec)", (1.0 / data.tick_duration_secs).toFixed(2), "Tick周期: " + data.tick_duration_secs + "s"),
        ].join("");

        document.getElementById("stats-grid").innerHTML = statsHtml;
        document.getElementById("world-overview").textContent = data.world_overview || "暂无世界事件日志...";

        await Promise.all([loadStatusConfigs(), loadActionTypeMap(), loadAllAgents()]);
    } catch (e) {
        console.error("Failed to load stats", e);
    }
}

function formatGameTimeDate(time) {
    var pad = function (n) { return String(n).padStart(2, "0"); };
    return time.year + "-" + pad(time.month) + "-" + pad(time.day) + " " +
        pad(time.hour) + ":" + pad(time.minute) + ":" + pad(time.second);
}

// ============================================================================
// Smooth Time Animation
// ============================================================================

function startSmoothTimeAnimation() {
    if (smoothTimeAnimationId) cancelAnimationFrame(smoothTimeAnimationId);

    function updateSmoothTime() {
        if (!smoothTimeConfig) return;
        var now = Date.now();
        var elapsedRealSecs = (now - smoothTimeConfig.lastSyncTime) / 1000;
        var tickProgress = Math.min(elapsedRealSecs / smoothTimeConfig.tickDurationSecs, 1.0);

        var baseGameHours = smoothTimeConfig.tickId / smoothTimeConfig.ticksPerHour;
        var fractionalHours = tickProgress / smoothTimeConfig.ticksPerHour;
        var totalGameHours = baseGameHours + fractionalHours;

        var hoursPerDay = 24, daysPerMonth = 30, monthsPerYear = 12;
        var hoursPerMonth = hoursPerDay * daysPerMonth;
        var hoursPerYear = hoursPerMonth * monthsPerYear;
        var totalHoursI = Math.floor(totalGameHours);
        var year = 1 + Math.floor(totalHoursI / hoursPerYear);
        var remainingAfterYear = totalHoursI % hoursPerYear;
        var month = 1 + Math.floor(remainingAfterYear / hoursPerMonth);
        var remainingAfterMonth = remainingAfterYear % hoursPerMonth;
        var day = 1 + Math.floor(remainingAfterMonth / hoursPerDay);
        var hour = remainingAfterMonth % hoursPerDay;
        var hourFrac = totalGameHours % 1;
        var minute = Math.floor(hourFrac * 60);
        var second = Math.round(((hourFrac * 60) % 1) * 60);

        var pad = function (n) { return String(n).padStart(2, "0"); };
        var dateStr = year + "-" + pad(month) + "-" + pad(day) + " " + pad(hour) + ":" + pad(minute) + ":" + pad(second);

        var card = document.getElementById("game-time-card");
        if (card) {
            var valueEl = card.querySelector(".stat-value");
            var subEl = card.querySelector(".stat-sub");
            if (valueEl) valueEl.textContent = dateStr;
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
