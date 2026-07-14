// ============================================================
// history.js — History page logic (experiences, summaries, chronicles)
// ============================================================

// ---------- Utility: debounce ----------
function debounce(fn, ms) {
    let timer;
    return function (...args) {
        clearTimeout(timer);
        timer = setTimeout(() => fn.apply(this, args), ms);
    };
}

// ============================================================
// Top-level tab switching
// ============================================================
function switchTopTab(tab) {
    document
        .querySelectorAll(".top-tab-btn")
        .forEach((b) => b.classList.remove("active"));
    document
        .querySelectorAll(".tab-panel")
        .forEach((p) => p.classList.remove("active"));
    document
        .querySelector(`.top-tab-btn[data-tab="${tab}"]`)
        .classList.add("active");
    document.getElementById(`panel-${tab}`).classList.add("active");
    loadHistoryTab(tab);
}

function loadHistoryTab(tab) {
    if (tab === "experiences") return ensureExperiencesLoaded();
    if (tab === "summaries" && !summariesLoaded) return loadSummaries();
    if (tab === "chronicles" && !chroniclesLoaded) return loadChronicles();
}

function loadActiveHistoryTab() {
    const activeBtn = document.querySelector(".top-tab-btn.active");
    loadHistoryTab(activeBtn ? activeBtn.dataset.tab : "experiences");
}

// ============================================================
// Shared agent loading (DRY — used by exp filters & summary filters)
// ============================================================
let _agentsPromise = null;

async function fetchAndPopulateAgents(selectIds) {
    if (!_agentsPromise) {
        _agentsPromise = apiFetch(API.BASE + "/agents")
            .then((r) => r.json())
            .catch((e) => {
                console.warn("加载角色列表失败:", e);
                _agentsPromise = null; // 失败时重置，允许重试
                return [];
            });
    }
    const agents = await _agentsPromise;
    if (!agents.length) return;

    agents.forEach((a) => {
        allAgentsMap[a.id] = a;
        if (a.agent_id) allAgentsMap[a.agent_id] = a;
    });

    const locSet = new Set();
    selectIds.forEach((selId) => {
        const sel = document.getElementById(selId);
        if (!sel || sel.options.length > 1) return; // already populated
        const frag = document.createDocumentFragment();
        agents.forEach((a) => {
            const opt = document.createElement("option");
            opt.value = a.id;
            const shortId = a.id ? a.id.substring(0, 8) : "";
            opt.textContent = `${a.name} (${shortId}...)`;
            frag.appendChild(opt);
            if (a.location && a.location !== "unknown") locSet.add(a.location);
        });
        sel.appendChild(frag);
    });

    // Populate location filter for experiences
    const locSel = document.getElementById("filter-location");
    if (locSel && locSel.options.length <= 1) {
        const locFrag = document.createDocumentFragment();
        [...locSet].sort().forEach((loc) => {
            const opt = document.createElement("option");
            opt.value = loc;
            opt.textContent = loc;
            locFrag.appendChild(opt);
        });
        locSel.appendChild(locFrag);
    }
}

// ============================================================
// Chronicles Panel
// ============================================================
let chronicles = [];
let chroniclesLoaded = false;

async function loadChronicles() {
    const container = document.getElementById("chronicles-container");
    container.innerHTML = '<div class="loading">加载中...</div>';
    try {
        const [chrRes, pendingRes] = await Promise.all([
            apiFetch(API.BASE + "/chronicles"),
            apiFetch(API.BASE + "/chronicles/pending").catch(() => ({ ok: false })),
        ]);
        const chrData = await chrRes.json();
        chronicles = chrData.chronicles || [];

        let pendingMap = {};
        if (pendingRes.ok) {
            const pendingData = await pendingRes.json();
            for (const t of pendingData.tasks || []) {
                pendingMap[t.chronicle_id] = t;
            }
        }
        chronicles = chronicles.map((c) => ({
            ...c,
            _pending: pendingMap[c.chronicle_id] || null,
        }));
        chroniclesLoaded = true;
        renderChronicles(chronicles);
    } catch (e) {
        if (e.name === "ApiError") {
            container.innerHTML = `<div class="empty-state"><p>${e.message === "UNAUTHORIZED" ? "请先登录" : "请求失败，请稍后重试"}</p></div>`;
        } else {
            container.innerHTML = `<div class="empty-state"><p>加载失败: ${escapeHtml(e.message)}</p></div>`;
        }
    }
}

function renderChronicles(list) {
    const container = document.getElementById("chronicles-container");
    if (!list.length) {
        container.innerHTML = `
        <div class="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                <path d="M12 6.042A8.967 8.967 0 006 3.75c-1.052 0-2.062.18-3 .512v14.25A8.987 8.987 0 016 18c2.305 0 4.408.867 6 2.292m0-14.25a8.966 8.966 0 016-2.292c1.052 0 2.062.18 3 .512v14.25A8.987 8.987 0 0018 18a8.967 8.967 0 00-6 2.292m0-14.25v14.25"/>
            </svg>
            <p>暂无群像传记</p>
            <p style="font-size: 12px; margin-top: 8px; color: var(--text-subtle)">每7个游戏日会自动生成</p>
        </div>`;
        return;
    }
    container.innerHTML = list
        .map((c) => {
            const range = escapeHtml(formatChronicleRange(c.formatted_start_date, c.formatted_end_date));
            return `
            <div class="chronicle-card" tabindex="0" data-chronicle-id="${escapeHtml(c.chronicle_id)}" role="button" aria-label="查看传记 ${escapeHtml(c.chronicle_id)}">
                <div class="chronicle-header">
                    <span class="chronicle-id">${escapeHtml(c.season)} · ${range}</span>
                    <span class="chronicle-season">${escapeHtml(c.chronicle_id)}</span>
                </div>
                <div class="chronicle-stats">
                    <div class="chr-stat-item"><div class="chr-stat-value">${escapeHtml(c.agent_count)}</div><div class="chr-stat-label">江湖儿女</div></div>
                    <div class="chr-stat-item"><div class="chr-stat-value">${escapeHtml(c.actions_count)}</div><div class="chr-stat-label">行动次数</div></div>
                    <div class="chr-stat-item"><div class="chr-stat-value">${escapeHtml(c.deaths)}</div><div class="chr-stat-label">陨落</div></div>
                    <div class="chr-stat-item"><div class="chr-stat-value">${escapeHtml(c.births)}</div><div class="chr-stat-label">新生</div></div>
                </div>
                <div class="chronicle-preview">${escapeHtml(c.summary_preview || "")}</div>
                <span class="chronicle-status status-${escapeHtml(getChrStatus(c))}">${escapeHtml(getChrStatusText(c))}</span>
            </div>`;
        })
        .join("");
}

function getChrStatus(c) {
    if (c._pending) {
        const t = c._pending;
        if (t.supplement_status === "generating" || t.supplement_status === "pending")
            return "generating";
    }
    return c.status;
}

function getChrStatusText(c) {
    if (c._pending) {
        const t = c._pending;
        if (t.supplement_status === "generating") return "LLM 生成中";
        if (t.supplement_status === "pending") return "等待生成";
    }
    return (
        { template: "模板生成", llm: "LLM 版本", both: "完整版" }[c.status] || c.status
    );
}

const filterChronicles = debounce(function () {
    const q = document.getElementById("chr-search-input").value.toLowerCase();
    if (!q) {
        renderChronicles(chronicles);
        return;
    }
    renderChronicles(
        chronicles.filter(
            (c) =>
                (c.summary_preview || "").toLowerCase().includes(q) ||
                (c.summary || "").toLowerCase().includes(q) ||
                (c.chronicle_id || "").toLowerCase().includes(q) ||
                (c.season || "").toLowerCase().includes(q) ||
                (c.agent_summaries || []).some((a) => (a.name || "").toLowerCase().includes(q)),
        ),
    );
}, 300);

async function openChronicle(id) {
    try {
        const res = await apiFetch(API.BASE + "/chronicles/" + id);
        if (!res.ok) throw new Error("加载失败");
        showChrModal(await res.json());
    } catch (e) {
        if (e.name !== "ApiError") alert("详情加载失败: " + e.message);
    }
}

function showChrModal(c) {
    document.getElementById("modal-title").textContent =
        `${c.chronicle_id} · ${c.season}季`;
    document.getElementById("modal-body").innerHTML = `
    <div class="detail-section">
        <h3>基本统计</h3>
        <div class="detail-grid">
            <div class="detail-item"><div class="label">周期</div><div class="value">${escapeHtml(formatChronicleRange(c.formatted_start_date, c.formatted_end_date))}<div class="chr-tick-range">Tick ${escapeHtml(c.period_start)} – ${escapeHtml(c.period_end)}</div></div></div>
            <div class="detail-item"><div class="label">江湖儿女</div><div class="value">${escapeHtml(c.agent_count)}</div></div>
            <div class="detail-item"><div class="label">行动次数</div><div class="value">${escapeHtml(c.actions_count)}</div></div>
            <div class="detail-item"><div class="label">陨落人数</div><div class="value">${escapeHtml(c.deaths)}</div></div>
            <div class="detail-item"><div class="label">新生人数</div><div class="value">${escapeHtml(c.births)}</div></div>
            <div class="detail-item"><div class="label">状态</div><div class="value">${escapeHtml(getChrStatusText(c))}</div></div>
        </div>
    </div>
    <div class="detail-section">
        <div class="detail-tab-nav">
            <button class="detail-tab-btn active" data-detail-tab="template" onclick="switchDetailTab('template')">模板版本</button>
            ${c.summary_llm ? '<button class="detail-tab-btn" data-detail-tab="llm" onclick="switchDetailTab(\'llm\')">LLM 版本</button>' : ""}
        </div>
        <div id="dt-template" class="detail-tab-content active">
            <div class="narrative">${escapeHtml(c.summary || "")}</div>
        </div>
        ${c.summary_llm ? `<div id="dt-llm" class="detail-tab-content"><div class="narrative">${escapeHtml(c.summary_llm)}</div></div>` : ""}
    </div>
    ${c.highlights && c.highlights.length ? `
    <div class="detail-section">
        <h3>关键事件</h3>
        <div class="highlight-list">
            ${c.highlights.map((h) => `<div class="highlight-item"><span class="highlight-type type-${escapeHtml(h.event_type || "")}">${escapeHtml({ death: "陨落", dialogue: "对话", combat: "战斗", social: "交际" }[h.event_type] || h.event_type || "")}</span><span class="highlight-desc">${escapeHtml(h.description || "")}</span></div>`).join("")}
        </div>
    </div>` : ""}
    ${c.emergence_events && c.emergence_events.length ? `
    <div class="detail-section">
        <h3>因果涌现</h3>
        <div class="emergence-list">
            ${c.emergence_events.map((e) => {
                const isCausal = e.category === "causal_emergence";
                const label = isCausal ? "因果涌现" : "共现（存疑）";
                const cls = isCausal ? "emergence-causal" : "emergence-cooccur";
                const edges = (e.causal_edges || []).map((ed) => {
                    const fn = (ed.from_agent || "").substring(0, 8);
                    const tn = (ed.to_agent || "").substring(0, 8);
                    return `<div class="emergence-edge">${escapeHtml(fn)} → ${escapeHtml(tn)}（${escapeHtml(ed.evidence || "")}）</div>`;
                }).join("");
                return `<div class="emergence-item ${cls}">
                    <span class="emergence-badge ${cls}">${escapeHtml(label)}</span>
                    <span class="emergence-desc">tick ${escapeHtml(e.tick_start)}–${escapeHtml(e.tick_end)}，${escapeHtml(e.action_count || 0)} 次互动（${escapeHtml((e.categories_covered || []).join("、"))}）</span>
                    ${edges}
                </div>`;
            }).join("")}
        </div>
    </div>` : ""}
    ${c.agent_summaries && c.agent_summaries.length ? `
    <div class="detail-section">
        <h3>江湖群像</h3>
        <div class="agents-grid">
            ${c.agent_summaries.map((a) => `<div class="agent-card"><div class="agent-name">${escapeHtml(a.name || "")}</div><div class="agent-info"><div>位置: ${escapeHtml(getLocationName(a.location || "-"))}</div><div>行动: ${escapeHtml(a.actions_count || 0)}次</div>${a.died_this_period ? '<div class="agent-died">已于本周期陨落</div>' : ""}</div></div>`).join("")}
        </div>
    </div>` : ""}
    `;
    document.getElementById("detail-modal").classList.add("show");
    // Focus the modal for accessibility
    document.getElementById("detail-modal").focus();
}

function switchDetailTab(tab) {
    document.querySelectorAll(".detail-tab-btn").forEach((b) => b.classList.remove("active"));
    document.querySelectorAll(".detail-tab-content").forEach((c) => c.classList.remove("active"));
    document.querySelector(`.detail-tab-btn[data-detail-tab="${tab}"]`).classList.add("active");
    document.getElementById(`dt-${tab}`).classList.add("active");
}

function closeModal() {
    document.getElementById("detail-modal").classList.remove("show");
}

async function generateChronicle() {
    if (!confirm("确定要手动生成一份群像传记吗？")) return;
    const btn = document.querySelector(".chr-controls .btn-primary");
    btn.disabled = true;
    btn.textContent = "生成中...";
    try {
        const res = await apiFetch(API.BASE + "/chronicles/generate", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: "{}",
        });
        if (res.ok) {
            alert("生成成功");
            loadChronicles();
        } else alert("生成失败: " + (await res.text()));
    } catch (e) {
        if (e.name !== "ApiError") alert("生成失败: " + e.message);
    } finally {
        btn.disabled = false;
        btn.textContent = "手动生成";
    }
}

// ============================================================
// Experiences Panel
// ============================================================
let experiences = [];
let expTotal = 0, expPage = 1, expPageSize = 20;
let expFiltersLoaded = false;
let experiencesLoaded = false;

async function ensureExperiencesLoaded() {
    await Promise.allSettled([loadDisplayMap(), getLayerDisplay()]); // 展示名 + 天魂层名映射必须在渲染前就绪
    if (!expFiltersLoaded) await initExpFilters();
    // URL 参数 ?agent=xxx 自动预选角色筛选（来自详情页跳转）
    const urlAgent = new URLSearchParams(location.search).get("agent");
    if (urlAgent && !experiencesLoaded) {
        await new Promise((r) => setTimeout(r, 100)); // 等筛选下拉填充
        const sel = document.getElementById("filter-agent");
        if (sel) sel.value = urlAgent;
    }
    if (!experiencesLoaded) await loadExperiences();
    if (!Object.keys(locationNames).length) await initLocationMapping();
}

async function initExpFilters() {
    if (expFiltersLoaded) return;
    try {
        await fetchAndPopulateAgents(["filter-agent"]);
        const actRes = await apiFetch(API.BASE + "/actions-map");
        if (actRes.ok) {
            const actMap = await actRes.json();
            const sel = document.getElementById("filter-action");
            const actFrag = document.createDocumentFragment();
            Object.entries(actMap).forEach(([k, v]) => {
                const opt = document.createElement("option");
                opt.value = k;
                opt.textContent = v;
                actFrag.appendChild(opt);
            });
            sel.appendChild(actFrag);
        }
        expFiltersLoaded = true;
    } catch (e) {
        if (e.name !== "ApiError") console.warn("加载过滤器失败:", e);
    }
}

async function loadExperiences() {
    const loading = document.getElementById("exp-loading");
    const empty = document.getElementById("exp-empty");
    const cardsEl = document.getElementById("experiences-cards");
    loading.style.display = "flex";
    empty.style.display = "none";
    cardsEl.innerHTML = "";

    const params = new URLSearchParams();
    params.set("page", expPage);
    params.set("limit", expPageSize);
    const aid = document.getElementById("filter-agent").value;
    const loc = document.getElementById("filter-location").value;
    const act = document.getElementById("filter-action").value;
    const resultVal = document.getElementById("filter-result").value;
    const from = document.getElementById("filter-from-tick").value;
    const to = document.getElementById("filter-to-tick").value;
    if (aid) params.set("agent_id", aid);
    if (loc) params.set("location", loc);
    if (act) params.set("action_type", act);
    if (resultVal && resultVal !== "all") params.set("result", resultVal);
    if (from) params.set("from_tick", from);
    if (to) params.set("to_tick", to);

    try {
        const res = await apiFetch(API.BASE + "/experiences?" + params);
        const data = await res.json();
        experiences = data.entries || [];
        expTotal = data.total || 0;
        experiencesLoaded = true;
        renderExpCards();
        updateExpPagination();
    } catch (e) {
        if (e.name === "ApiError") {
            cardsEl.innerHTML = `<div class="empty-state"><p>${e.message === "UNAUTHORIZED" ? "请先登录" : "请求失败"}</p></div>`;
        } else {
            cardsEl.innerHTML = `<div class="empty-state"><p>加载失败: ${escapeHtml(e.message)}</p></div>`;
        }
    } finally {
        loading.style.display = "none";
    }
}

function renderExpCards() {
    const empty = document.getElementById("exp-empty");
    const cardsEl = document.getElementById("experiences-cards");
    if (!experiences.length) {
        empty.style.display = "flex";
        cardsEl.innerHTML = "";
        return;
    }
    empty.style.display = "none";

    cardsEl.innerHTML = experiences
        .map((e) => {
            const metadata = e.soul_cycle_metadata || {};
            const cycles = metadata.cycles || [];
            const executionResults = metadata.execution_results || null;
            const isSuccess = e.result === "success";
            const resultBadge = `<span class="result-badge ${isSuccess ? "result-success" : "result-failed"}">${isSuccess ? "成功" : "失败"}</span>`;

            const timeStr = e.formatted_time
                || (e.created_at
                    ? new Date(e.created_at).toLocaleString("zh-CN", {
                          month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
                      })
                    : "-");

            // 三魂内容（复用现有渲染逻辑）
            const renhunHtml = renderRenhunCell(cycles, e, executionResults);
            const dihunHtml = renderDihunCell(cycles);
            const tianhunHtml = renderTianhunCell(cycles, e);
            const modelId = (cycles[0] && cycles[0].model_id) || "-";

            // 动作摘要行（首条 pipeline action 或主 intent）
            let actionSummary = "-";
            if (cycles.length > 0) {
                const fi = cycles[0].final_intent;
                if (fi) {
                    if (fi.pipeline_actions && fi.pipeline_actions.length > 0) {
                        const pa = fi.pipeline_actions[0];
                        actionSummary = renderActionText(pa.action_type, parseActionData(pa.action_data));
                    } else if (fi.action_type) {
                        actionSummary = renderActionText(fi.action_type, parseActionData(fi.action_data));
                    }
                }
            }
            if (actionSummary === "-" && e.action_type) {
                actionSummary = renderActionText(e.action_type, parseActionData(e.action_data));
            }

            return (
                `<div class="exp-card">` +
                // 头部：Tick · 时间 · 角色 · 位置 · 结果
                `<div class="exp-card-header">` +
                `<span class="tick-badge">T${escapeHtml(e.tick_id || "-")}</span>` +
                `<span class="exp-card-time">${escapeHtml(timeStr)}</span>` +
                `<span class="exp-card-agent">${escapeHtml(e.agent_name || "-")}</span>` +
                `<span class="exp-card-loc">@ ${escapeHtml(getLocationName(e.location || "-"))}</span>` +
                resultBadge +
                `</div>` +
                // 动作摘要行
                `<div class="exp-card-action"><span class="exp-action-badge">${escapeHtml(e.action_type_display || e.action_type || "-")}</span> ${actionSummary}</div>` +
                // 三魂区块
                `<div class="exp-card-body">` +
                (renhunHtml !== "-" ? `<div class="exp-soul-block"><span class="exp-soul-label">人魂</span><div class="exp-soul-content">${renhunHtml}</div></div>` : "") +
                (dihunHtml !== "-" ? `<div class="exp-soul-block"><span class="exp-soul-label">地魂</span><div class="exp-soul-content">${dihunHtml}</div></div>` : "") +
                (tianhunHtml !== "-" ? `<div class="exp-soul-block"><span class="exp-soul-label">天魂</span><div class="exp-soul-content">${tianhunHtml}</div></div>` : "") +
                `</div>` +
                // 底部次要信息：模型 ID
                `<div class="exp-card-footer"><span class="mono-text">模型: ${escapeHtml(modelId)}</span></div>` +
                `</div>`
            );
        })
        .join("");
}

// 渲染人魂单元格（叙事 + 推理 + JSON action）
function renderRenhunCell(cycles, entry, executionResults) {
    let html = "";
    if (!cycles || cycles.length === 0) {
        if (!entry.thought_log) return "-";
        return `<div class="exp-meta-text" style="font-style:italic;color:var(--text-secondary);">${escapeHtml(entry.thought_log)}</div>`;
    }
    cycles.forEach((cycle, idx) => {
        if (cycles.length > 1) html += `<div class="tick-attempt-label">第${idx + 1}次</div>`;
        const rh = cycle.renhun;
        if (rh) {
            if (rh.narrative) html += `<div class="exp-meta-text">${escapeHtml(rh.narrative)}</div>`;
            if (rh.thought_log) html += `<div class="exp-meta-text" style="font-style:italic;color:var(--text-secondary);">${escapeHtml(rh.thought_log)}</div>`;
        }
        const fi = cycle.final_intent;
        if (fi) {
            if (fi.pipeline_actions && fi.pipeline_actions.length > 0) {
                fi.pipeline_actions.forEach((item, pidx) => {
                    const aType = item.action_type || "";
                    const aData = parseActionData(item.action_data);
                    html += `<div class="exp-meta-text" style="color:var(--text-subtle);">${renderSingleAction(aType, aData)}</div>`;
                    if (executionResults && executionResults[String(pidx)]) {
                        const er = executionResults[String(pidx)];
                        const ok = er.success;
                        html += `<div style="margin-top:2px;"><span class="result-badge ${ok ? "result-success" : "result-failed"}" style="font-size:11px;">${ok ? "成功" : (er.error || "失败")}</span></div>`;
                    }
                });
            } else if (fi.action_type) {
                const aType = fi.action_type || "";
                const aData = parseActionData(fi.action_data);
                html += `<div class="exp-meta-text" style="color:var(--text-subtle);">${renderSingleAction(aType, aData)}</div>`;
                if (executionResults && executionResults["0"]) {
                    const er = executionResults["0"];
                    const ok = er.success;
                    html += `<div style="margin-top:2px;"><span class="result-badge ${ok ? "result-success" : "result-failed"}" style="font-size:11px;">${ok ? "成功" : (er.error || "失败")}</span></div>`;
                }
            }
        }
        // 执行结果（仅已通过天魂审查的 cycle）
        const thApproved = cycle.tianhun && cycle.tianhun.result === "approved";
        if (thApproved && entry.result) {
            const isOk = entry.result === "success";
            html += `<div style="margin-top:2px;"><span class="result-badge ${isOk ? "result-success" : "result-failed"}" style="font-size:11px;">${isOk ? "成功" : "失败"}</span></div>`;
        }
    });
    return html || "-";
}

// 渲染天魂单元格
function renderTianhunCell(cycles, entry) {
    if (!cycles || cycles.length === 0) {
        if (!entry.result) return "-";
        const isApproved = entry.result === "success";
        return `<span class="soul-layer-tag ${isApproved ? "passed" : "failed"}">${isApproved ? "✓通过" : "✗驳回"}</span>`;
    }
    let html = "";
    cycles.forEach((cycle, idx) => {
        if (cycles.length > 1) html += `<div class="tick-attempt-label">第${idx + 1}次</div>`;
        const th = cycle.tianhun;
        if (!th) return;
        if (th.layers && th.layers.length > 0) {
            html += `<div class="soul-layers">`;
            th.layers.forEach((l) => {
                const passed = l.passed;
                const name = (_layerDisplayCache || LAYER_NAMES)[l.layer] || l.layer;
                html += `<span class="soul-layer-tag ${passed ? "passed" : "failed"}">${escapeHtml(name)}${passed ? "" : ": " + escapeHtml(l.detail || "")}</span>`;
            });
            html += `</div>`;
        }
        if (th.reason) html += `<div class="exp-meta-text" style="color:var(--text-secondary);">${escapeHtml(th.reason)}</div>`;
    });
    return html || "-";
}

// 地魂 action_type 中文映射（说话检测函数在 utils.js: isSpeakAtype / isWhisperAtype / isShoutAtype，纯 channel 字段判断）

function parseActionData(raw) {
    if (!raw) return {};
    if (typeof raw === "object") return raw;
    if (typeof raw === "string") {
        try { return JSON.parse(raw); } catch { return {}; }
    }
    return {};
}

// 渲染单个 action 的描述文本（统一渲染器在 utils.js: renderActionText）
function renderSingleAction(aType, aData) {
    return renderActionText(aType, aData);
}

// 渲染地魂单元格（地魂 tool calling 日志）
function renderDihunCell(cycles) {
    if (!cycles || cycles.length === 0) return "-";
    let calls = [];
    cycles.forEach((cycle) => {
        if (cycle.renhun && cycle.renhun.earth_tool_calls) {
            cycle.renhun.earth_tool_calls.forEach((tc) => { if (tc.success) calls.push(tc); });
        }
    });
    if (calls.length === 0) return "-";
    return calls.map((tc) => {
        let argsPreview = tc.arguments || "{}";
        try { argsPreview = Object.entries(JSON.parse(argsPreview)).map(([k, v]) => k + ': ' + String(v).substring(0, 20)).join(', '); } catch(e) {}
        let summary = tc.result_summary ? escapeHtml(tc.result_summary.substring(0, 40)) : '';
        return '<div style="font-size:11px;color:var(--text-secondary);">' +
            escapeHtml(tc.name) + '(' + escapeHtml(argsPreview) + ') ' +
            '<span style="color:var(--text-subtle);">→ ' + summary + '</span></div>';
    }).join('');
}

function updateExpPagination() {
    const totalPages = Math.ceil(expTotal / expPageSize);
    const pg = document.getElementById("exp-pagination");
    const info = document.getElementById("exp-page-info");
    if (expTotal === 0) { pg.style.display = "none"; return; }
    pg.style.display = "flex";
    info.textContent = `第 ${expPage} / ${totalPages} 页，共 ${expTotal} 条`;
    document.getElementById("exp-prev-btn").disabled = expPage <= 1;
    document.getElementById("exp-next-btn").disabled = expPage >= totalPages;
    document.getElementById("exp-page-size").value = expPageSize;
}

function changeExpPage(delta) {
    expPage = Math.max(1, expPage + delta);
    loadExperiences();
}

function changeExpPageSize() {
    expPageSize = parseInt(document.getElementById("exp-page-size").value);
    expPage = 1;
    loadExperiences();
}

function resetExpFilters() {
    document.getElementById("filter-agent").value = "";
    document.getElementById("filter-location").value = "";
    document.getElementById("filter-action").value = "";
    document.getElementById("filter-result").value = "all";
    document.getElementById("filter-from-tick").value = "";
    document.getElementById("filter-to-tick").value = "";
    expPage = 1;
    loadExperiences();
}

// ============================================================
// Daily Summaries Panel
// ============================================================
let summariesData = [];
let sumTotal = 0, sumPage = 1, sumPageSize = 20;
let summariesLoaded = false;
let _selectedSumAgentId = "";

// Combobox: 即时过滤角色列表
function filterAgentDropdown() {
    const input = document.getElementById("sum-agent-input");
    const dropdown = document.getElementById("sum-agent-dropdown");
    const q = input.value.toLowerCase().trim();

    const seen = new Set();
    const agents = Object.values(allAgentsMap).filter((a) => {
        if (seen.has(a.id)) return false;
        seen.add(a.id);
        return true;
    });
    const matched = q
        ? agents.filter(
              (a) =>
                  (a.name || "").toLowerCase().includes(q) ||
                  (a.id || "").toLowerCase().includes(q),
          )
        : agents;

    if (!matched.length) {
        dropdown.innerHTML = '<div class="combobox-empty">无匹配角色</div>';
        dropdown.classList.add("open");
        return;
    }

    dropdown.innerHTML = matched
        .slice(0, 50)
        .map((a) => {
            const short = a.id ? a.id.substring(0, 8) : "";
            return `<div class="combobox-option" data-agent-id="${escapeHtml(a.id)}">${escapeHtml(a.name)} <span class="timeline-agent-id">(${short}...)</span></div>`;
        })
        .join("");
    dropdown.classList.add("open");
}

function selectAgentOption(el) {
    const id = el.dataset.agentId;
    const agent = allAgentsMap[id];
    const name = agent ? agent.name : el.textContent.trim().split(" (")[0];
    document.getElementById("sum-agent-input").value = name;
    document.getElementById("sum-agent-id").value = id;
    _selectedSumAgentId = id;
    document.getElementById("sum-agent-dropdown").classList.remove("open");
    sumPage = 1;
    loadSummaries();
}

function onComboboxBlur() {
    setTimeout(() => {
        const dropdown = document.getElementById("sum-agent-dropdown");
        if (!dropdown.matches(":hover")) {
            dropdown.classList.remove("open");
            const input = document.getElementById("sum-agent-input");
            if (!input.value.trim()) {
                // 输入框清空 → 清除筛选
                document.getElementById("sum-agent-id").value = "";
                _selectedSumAgentId = "";
            } else if (_selectedSumAgentId) {
                // 有选中值但输入框被手动修改 → 恢复为已选 agent 的名字
                const agent = allAgentsMap[_selectedSumAgentId];
                if (agent && input.value !== agent.name) {
                    input.value = agent.name;
                }
            }
        }
    }, 150);
}

const debouncedSumTextFilter = debounce(function () {
    if (!summariesLoaded) return;
    applySumTextFilter();
}, 300);

async function loadSummaries() {
    const container = document.getElementById("summaries-container");
    container.innerHTML = '<div class="loading">加载中...</div>';

    await fetchAndPopulateAgents([]);

    const params = new URLSearchParams();
    params.set("page", sumPage);
    params.set("limit", sumPageSize);
    if (_selectedSumAgentId) params.set("agent_id", _selectedSumAgentId);

    try {
        const res = await apiFetch(API.BASE + "/agent-daily-summaries?" + params);
        const data = await res.json();
        summariesData = data.summaries || [];
        sumTotal = data.total || 0;
        summariesLoaded = true;
        applySumTextFilter();
        updateSumPagination();
    } catch (e) {
        if (e.name === "ApiError") {
            container.innerHTML = `<div class="empty-state"><p>${e.message === "UNAUTHORIZED" ? "请先登录" : "请求失败，请稍后重试"}</p></div>`;
        } else {
            container.innerHTML = `<div class="empty-state"><p>加载失败: ${escapeHtml(e.message)}</p></div>`;
        }
    }
}

// 客户端文本即时过滤（不影响 sumTotal/分页，仅在当前页内筛选显示）
function applySumTextFilter() {
    const q = document.getElementById("sum-search-input").value.toLowerCase().trim();
    if (q) {
        renderSummaries(
            summariesData.filter(
                (s) =>
                    (s.summary || "").toLowerCase().includes(q) ||
                    getAgentName(s.agent_id).toLowerCase().includes(q),
            ),
        );
    } else {
        renderSummaries(summariesData);
    }
}

function resetSumFilters() {
    document.getElementById("sum-search-input").value = "";
    document.getElementById("sum-agent-input").value = "";
    document.getElementById("sum-agent-id").value = "";
    _selectedSumAgentId = "";
    sumPage = 1;
    loadSummaries();
}

function getAgentName(agentId) {
    if (!agentId) return "未知角色";
    if (allAgentsMap && allAgentsMap[agentId])
        return allAgentsMap[agentId].name || `Agent ${agentId.substring(0, 8)}`;
    return `Agent ${agentId.substring(0, 8)}`;
}

function renderSummaries(list) {
    const container = document.getElementById("summaries-container");
    if (!list.length) {
        container.innerHTML = `
        <div class="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                <path d="M19.5 14.25v-2.625a3.375 3.375 0 00-3.375-3.375h-1.5A1.125 1.125 0 0113.5 7.125v-1.5a3.375 3.375 0 00-3.375-3.375H8.25m0 12.75h7.5m-7.5 3H12M10.5 2.25H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 00-9-9z"/>
            </svg>
            <p>暂无每日摘要</p>
            <p style="font-size: 12px; margin-top: 8px; color: var(--text-subtle)">游戏日结束后自动生成</p>
        </div>`;
        return;
    }
    container.innerHTML = list
        .map((s) => {
            const d = new Date(s.created_at);
            const dateStr = d.toLocaleString("zh-CN", {
                year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
            });
            const agentName = getAgentName(s.agent_id);
            const agentShortId = s.agent_id ? s.agent_id.substring(0, 8) : "";
            const calTime = s.formatted_time || "-";
            return `
        <div class="timeline-item">
            <div class="timeline-dot"></div>
            <div class="timeline-content">
                <div class="timeline-header">
                    <span class="timeline-calendar">${escapeHtml(calTime)}</span>
                    <span class="timeline-agent">
                        ${escapeHtml(agentName)}
                        <span class="timeline-agent-id">(${escapeHtml(agentShortId)}...)</span>
                    </span>
                </div>
                <div class="timeline-meta">存档时间: ${escapeHtml(dateStr)}</div>
                <div class="timeline-body" data-sum-expanded="false">${escapeHtml(s.summary || "")}</div>
                <button class="summary-expand-btn" onclick="toggleSummaryExpand(this)">展开全文</button>
            </div>
        </div>`;
        })
        .join("");
}

function updateSumPagination() {
    const totalPages = Math.ceil(sumTotal / sumPageSize);
    const pg = document.getElementById("sum-pagination");
    const info = document.getElementById("sum-page-info");
    if (sumTotal === 0) { pg.style.display = "none"; return; }
    pg.style.display = "flex";
    info.textContent = `第 ${sumPage} / ${totalPages} 页，共 ${sumTotal} 条`;
    document.getElementById("sum-prev-btn").disabled = sumPage <= 1;
    document.getElementById("sum-next-btn").disabled = sumPage >= totalPages;
    document.getElementById("sum-page-size").value = sumPageSize;
}

function changeSumPage(delta) {
    const totalPages = Math.ceil(sumTotal / sumPageSize);
    sumPage = Math.max(1, Math.min(totalPages, sumPage + delta));
    loadSummaries();
}

function changeSumPageSize() {
    sumPageSize = parseInt(document.getElementById("sum-page-size").value);
    sumPage = 1;
    loadSummaries();
}

function toggleSummaryExpand(btn) {
    const content = btn.previousElementSibling;
    const expanded = content.classList.toggle("expanded");
    btn.textContent = expanded ? "收起" : "展开全文";
}

// ============================================================
// Formatting helpers
// ============================================================

function formatChronicleRange(startDate, endDate) {
    // 服务端已保证两端均为完整的"x年x月x日"，直接拼接。
    // 跨年/跨月时两端都完整显示，符合"x年x月x日 至 x年x月x日"表述。
    return startDate + ' 至 ' + endDate;
}

// ============================================================
// Event listeners
// ============================================================
document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeModal();
});

document.addEventListener("click", (e) => {
    // Chronicle card click (event delegation)
    const card = e.target.closest(".chronicle-card[data-chronicle-id]");
    if (card) {
        openChronicle(card.dataset.chronicleId);
        return;
    }

    // Modal overlay click-to-close (only if clicking the overlay itself, not content)
    const modal = document.getElementById("detail-modal");
    if (e.target === modal) closeModal();

    // Combobox option click (event delegation)
    const option = e.target.closest(".combobox-option");
    if (option) {
        selectAgentOption(option);
        return;
    }
    // Click outside combobox → close dropdown
    const combobox = e.target.closest(".combobox");
    if (!combobox) {
        document.querySelectorAll(".combobox-dropdown.open").forEach((d) => d.classList.remove("open"));
    }
});

// Chronicle card keyboard activation
document.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
        const card = e.target.closest(".chronicle-card[data-chronicle-id]");
        if (card) {
            e.preventDefault();
            openChronicle(card.dataset.chronicleId);
        }
    }
});

// Load experiences on init (default tab)
ensureExperiencesLoaded();

// Combobox event binding
const _sumAgentInput = document.getElementById("sum-agent-input");
if (_sumAgentInput) {
    _sumAgentInput.addEventListener("focus", filterAgentDropdown);
    _sumAgentInput.addEventListener("input", filterAgentDropdown);
    _sumAgentInput.addEventListener("blur", onComboboxBlur);
}
