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
            opt.textContent = formatNameId(a.name, a.id);
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
            ${c.highlights.map((h) => `<div class="highlight-item"><span class="highlight-type type-${escapeHtml(h.event_type || "")}">${escapeHtml({ death: "陨落", retire: "归隐", dialogue: "对话", combat: "战斗", social: "交际" }[h.event_type] || h.event_type || "")}</span><span class="highlight-desc">${escapeHtml(h.description || "")}</span></div>`).join("")}
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
                    const fn = resolveTargetName(ed.from_agent || "");
                    const tn = resolveTargetName(ed.to_agent || "");
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
            ${c.agent_summaries.map((a) => {
                const fateCls = a.retired_this_period ? " agent-retired-card" : a.died_this_period ? " agent-died-card" : "";
                const fateHtml = a.retired_this_period
                    ? '<div class="agent-retired">已于本周期归隐</div>'
                    : a.died_this_period
                      ? '<div class="agent-died">已于本周期陨落</div>'
                      : "";
                return `<div class="agent-card${fateCls}"><div class="agent-name">${escapeHtml(a.name || "")}</div><div class="agent-info"><div>位置: ${escapeHtml(getLocationName(a.location || "-"))}</div><div>行动: ${escapeHtml(a.actions_count || 0)}次</div>${fateHtml}</div></div>`;
            }).join("")}
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
    // 展示名映射 + 天魂层名 + 全量 agent 列表必须在渲染前就绪：
    // - loadDisplayMap: target_agent_id → 角色名（权威源）
    // - loadAllAgents: 全量 agent 填充 allAgentsMap（displayMap 失败时的第二查表路径）
    // history 页与 agents 页共用 agents.js，但原 history 页从不调用 loadAllAgents，
    // 导致 allAgentsMap 恒为空对象、resolveTargetName 第二查表失效。此处补齐。
    await Promise.allSettled([loadDisplayMap(), getLayerDisplay(), loadAllAgents()]);
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
    // 必须无条件发送 result：服务端缺省值是 success，若选「全部」时不发该参数，
    // 用户看到的「全部」实际只是「仅成功」，失败卡片永远不可见
    params.set("result", resultVal || "all");
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

    cardsEl.innerHTML =
        `<div class="exp-list">` + experiences.map(renderExpCard).join("") + `</div>`;
}

// 属性上下文转义：escapeHtml 走 textContent→innerHTML，不处理引号，
// 直接放进 title="..." 会被参数里的引号截断属性，故补一层引号转义
function escapeAttr(text) {
    return escapeHtml(text).replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

// 现实时间（服务端落库时刻）：固定 YYYY-MM-DD HH:mm:ss，本地时区，便于与排障时间对齐
function formatRealTime(iso) {
    if (!iso) return "-";
    const d = new Date(iso);
    if (isNaN(d.getTime())) return "-";
    const p = (n) => String(n).padStart(2, "0");
    return (
        d.getFullYear() + "-" + p(d.getMonth() + 1) + "-" + p(d.getDate()) +
        " " + p(d.getHours()) + ":" + p(d.getMinutes()) + ":" + p(d.getSeconds())
    );
}

// 单张 tick 卡片
//
// 结构对齐 agent-web 经历页（时间轴节点 + 卡片头 + 行动尝试盒子 + 三魂色条块）：
// 人魂块 = 叙事/推理/地魂工具调用，天魂块 = 四层审查，地魂块 = 最终行动与执行结果。
// server 端已按 (agent_id, tick_id) 聚合分页，故一张卡片即一个 tick。
//
// 与 agent-web 的三处有意偏离（其余按 agent-web 视觉语言为准）：
// 1. 模型为空时始终渲染"模型未上报"芯片，agent-web 是 if (modelId) 整块省略。
//    流水页需要区分"未上报"与"没显示"，故保留占位文案。
// 2. 卡片外壳用 dashboard.css 的 .tick-card，agent-web 侧是 .tl-content
//    （两个静态目录各自加载自己的样式表，类名不同但视觉等价）。
// 3. 现实时间固定格式 YYYY-MM-DD HH:mm:ss，agent-web 用 toLocaleString('zh-CN')；
//    流水页逐行比对时间，固定格式更易读且不随 locale 变化。
function renderExpCard(e) {
    const metadata = e.soul_cycle_metadata || {};
    const cycles = metadata.cycles || [];
    const executionResults = metadata.execution_results || null;
    // 徽章用服务端给的卡片级成败（与 result 筛选同口径，故徽章与筛选视图必然一致）。
    // 不在前端从 execution_results 自行聚合：无元数据的卡片没有该字段，
    // 只能退回主行 result，会出现"仅失败"视图里顶着成功徽章。
    const isSuccess = e.card_success === true;
    const resultBadge =
        `<span class="result-badge ${isSuccess ? "result-success" : "result-failed"}">` +
        `${isSuccess ? "成功" : "失败"}</span>`;

    // 卡片主体复用 dashboard.css 的 .tick-card（与 agent-web 经历卡片同一套视觉语言）
    let html = `<div class="tl-item"><div class="tl-dot"></div><div class="tick-card">`;

    // 卡片头：Tick · 游戏内时间 · 模型 · 现实时间
    html += `<div class="tick-card-header">`;
    html += `<span class="tick-badge">T${escapeHtml(e.tick_id || "-")}</span>`;
    html += `<span class="tick-world-time">${escapeHtml(e.formatted_time || "-")}</span>`;
    html += `<span class="tick-model">${escapeHtml(e.model_id || "模型未上报")}</span>`;
    html += `<span class="tick-real-time" title="服务端记录时刻">${escapeHtml(formatRealTime(e.created_at))}</span>`;
    html += `</div>`;

    // 卡片元信息行（admin 专有：全局流水需标注角色与位置）
    html += `<div class="exp-card-meta">`;
    html += `<span class="exp-card-agent">${escapeHtml(e.agent_name ? formatNameId(e.agent_name, e.agent_id) : "-")}</span>`;
    html += `<span class="exp-card-loc">@ ${escapeHtml(getLocationName(e.location || "-"))}</span>`;
    html += resultBadge;
    html += `</div>`;

    // 行动区：每个 attempt 一个盒子（三魂并列）
    html += `<div class="tick-section"><div class="tick-section-title">行动</div>`;
    html += `<div class="tick-attempts-container">`;
    if (cycles.length === 0) {
        html += renderDegradedAttemptBox(e);
    } else {
        cycles.forEach((cycle, idx) => {
            html += renderAttemptBox(cycle, idx, cycles.length, executionResults);
        });
    }
    html += `</div></div>`;

    html += `</div></div>`;
    return html;
}

// 单个 attempt 盒子：人魂 / 天魂 / 地魂
function renderAttemptBox(cycle, idx, total, executionResults) {
    let html = `<div class="tick-attempt-box">`;
    if (total > 1) html += `<div class="tick-attempt-label">行动 ${idx + 1}</div>`;
    html += renderRenhunBlock(cycle);
    html += renderTianhunBlock(cycle);
    html += renderDihunBlock(cycle, executionResults);
    html += `</div>`;
    return html;
}

// 元数据缺失时的降级盒子：用扁平行字段拼出同样的三魂结构，避免空白卡片
function renderDegradedAttemptBox(e) {
    let html = `<div class="tick-attempt-box">`;

    // 叙事与推理各自独立成行：老实现只在 narrative 缺失时才退回 thought_log，
    // 会把同时存在的推理文本丢掉
    let renhunInner = "";
    if (e.narrative) renhunInner += `<div class="soul-text">${escapeHtml(e.narrative)}</div>`;
    if (e.thought_log) renhunInner += `<div class="soul-thought">${escapeHtml(e.thought_log)}</div>`;
    if (renhunInner) {
        html += `<div class="exp-renhun"><span class="exp-soul-label">人魂</span>` +
            `<div class="exp-soul-content">${renhunInner}</div></div>`;
    }

    if (e.result) {
        const ok = e.result === "success";
        // 无元数据卡片没有四层审查结论，此处只有主行的 Server 执行结果，
        // 故标注为「主行执行」而非「天魂审查通过/驳回」；并显式说明整卡判定，
        // 否则会出现"头部失败徽章 + 卡体天魂通过"的自相矛盾
        html += `<div class="exp-tianhun"><span class="exp-soul-label">天魂</span>` +
            `<div class="exp-soul-content"><div class="soul-result ${ok ? "approved" : "rejected"}">` +
            `主行执行${ok ? "成功" : "失败"}</div>` +
            (e.card_success === false && ok
                ? `<div class="soul-reason">该 tick 另有动作失败，整卡判定为失败</div>`
                : "") +
            (e.reflector_thought ? `<div class="soul-reason">${escapeHtml(e.reflector_thought)}</div>` : "") +
            `</div></div>`;
    }

    if (e.action_type) {
        html += `<div class="exp-action"><span class="exp-soul-label">地魂</span>` +
            `<div class="exp-soul-content">` +
            renderSingleAction(e.action_type, parseActionData(e.action_data)) +
            (e.result_message ? `<div class="soul-reason">${escapeHtml(e.result_message)}</div>` : "") +
            `</div></div>`;
    }

    // 该 tick 的其余失败动作：降级卡片没有 execution_results 可渲染，
    // 不列出的话用户只看到主行，与卡片级徽章的口径不一致
    (e.other_failed_actions || []).forEach((a) => {
        const name = a.action_type_display || a.action_type;
        html += `<div class="exp-action"><span class="exp-soul-label">地魂</span>` +
            `<div class="exp-soul-content">` +
            `<div class="exp-failed-action">${escapeHtml(name)}</div>` +
            (a.result_message
                ? `<div class="soul-reason">${escapeHtml(a.result_message)}</div>`
                : "") +
            `</div></div>`;
    });

    html += `</div>`;
    return html;
}

// 人魂块：叙事 + 推理 + 地魂工具调用（工具调用嵌入人魂推理循环，故同块展示）
function renderRenhunBlock(cycle) {
    const rh = cycle.renhun || {};
    const tools = rh.earth_tool_calls || [];
    let inner = "";
    if (rh.narrative) inner += `<div class="soul-text">${escapeHtml(rh.narrative)}</div>`;
    if (rh.thought_log) inner += `<div class="soul-thought">${escapeHtml(rh.thought_log)}</div>`;
    tools.forEach((t) => {
        const full = `${t.name}(${t.arguments || ""}) → ${t.success ? "成功" : "失败"}: ${t.result_summary || ""}`;
        const brief = `${t.name}(${String(t.arguments || "").substring(0, 60)}) → ${t.success ? "成功" : "失败"}`;
        inner += `<div class="soul-tool" title="${escapeAttr(full)}">${escapeHtml(brief)}</div>`;
    });
    if (!inner) return "";
    return `<div class="exp-renhun"><span class="exp-soul-label">人魂</span>` +
        `<div class="exp-soul-content">${inner}</div></div>`;
}

// 天魂块：审查结论 + 四层标签
function renderTianhunBlock(cycle) {
    const th = cycle.tianhun;
    if (!th) return "";
    let inner = "";
    if (th.result) {
        const isApproved = th.result === "approved";
        inner += `<div class="soul-result ${isApproved ? "approved" : "rejected"}">` +
            `${isApproved ? "通过" : "驳回"}</div>`;
    }
    // 多意图逐意图审查（新格式）优先；旧数据回退平铺 layers
    if (th.per_intent_layers && th.per_intent_layers.length > 0) {
        th.per_intent_layers.forEach((pil) => {
            inner += `<div class="soul-intent-label">意图「${escapeHtml(pil.intent || "-")}」</div>`;
            inner += renderLayerTagsAdmin(pil.layers || []);
        });
    } else if (th.layers && th.layers.length > 0) {
        inner += renderLayerTagsAdmin(th.layers);
    }
    if (th.reason) inner += `<div class="soul-reason">${escapeHtml(th.reason)}</div>`;
    if (!inner) return "";
    return `<div class="exp-tianhun"><span class="exp-soul-label">天魂</span>` +
        `<div class="exp-soul-content">${inner}</div></div>`;
}

// 地魂块：最终行动（含多意图流水）与逐条执行结果
function renderDihunBlock(cycle, executionResults) {
    const fi = cycle.final_intent;
    if (!fi) return "";
    let inner = "";

    const pipeline = fi.pipeline_actions;
    if (pipeline && pipeline.length > 0) {
        const multi = pipeline.length > 1;
        pipeline.forEach((item, pidx) => {
            if (multi) inner += `<div class="soul-intent-label">意图 ${pidx + 1}</div>`;
            inner += renderSingleAction(item.action_type || "", parseActionData(item.action_data));
            inner += renderExecutionBadgeAdmin(executionResults, pidx);
        });
    } else if (fi.action_type) {
        inner += renderSingleAction(fi.action_type, parseActionData(fi.action_data));
        inner += renderExecutionBadgeAdmin(executionResults, 0);
    }

    if (!inner) return "";
    return `<div class="exp-action"><span class="exp-soul-label">地魂</span>` +
        `<div class="exp-soul-content">${inner}</div></div>`;
}

// 执行结果徽章：execution_results 以 pipe_seq 为键，与 pipeline 下标一一对应
function renderExecutionBadgeAdmin(executionResults, pipeSeq) {
    if (!executionResults) return "";
    const er = executionResults[String(pipeSeq)];
    if (!er) return "";
    const ok = er.success;
    const text = ok ? "执行成功" : (er.error || "执行失败");
    return `<div class="soul-exec-badge"><span class="result-badge ` +
        `${ok ? "result-success" : "result-failed"}">${escapeHtml(text)}</span></div>`;
}

// 天魂层标签组渲染（renderTianhunBlock 内部复用）
function renderLayerTagsAdmin(layers) {
    let html = `<div class="soul-layers">`;
    layers.forEach((l) => {
        // 历史快照 JSONB 中 skip 曾被误判为 passed=false，
        // 渲染时以 skip 文本为准强制按通过展示
        const passed = l.passed || isLlmSkipDetail(l.detail);
        const name = (_layerDisplayCache || LAYER_NAMES)[l.layer] || l.layer;
        const detail = l.detail ? ": " + escapeHtml(layerDetailText(l.detail)) : "";
        html += `<span class="soul-layer-tag ${passed ? "passed" : "failed"}">${escapeHtml(name)}${detail}</span>`;
    });
    html += `</div>`;
    return html;
}

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

function updateExpPagination() {
    const totalPages = Math.ceil(expTotal / expPageSize);
    const pg = document.getElementById("exp-pagination");
    const info = document.getElementById("exp-page-info");
    if (expTotal === 0) { pg.style.display = "none"; return; }
    pg.style.display = "flex";
    info.textContent = `第 ${expPage} / ${totalPages} 页，共 ${expTotal} 个 tick`;
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
            return `<div class="combobox-option" data-agent-id="${escapeHtml(a.id)}">${escapeHtml(formatNameId(a.name, a.id))}</div>`;
        })
        .join("");
    dropdown.classList.add("open");
}

function selectAgentOption(el) {
    const id = el.dataset.agentId;
    const agent = allAgentsMap[id];
    const name = agent ? agent.name : el.textContent.trim().split("[")[0];
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
    if (allAgentsMap && allAgentsMap[agentId] && allAgentsMap[agentId].name)
        return formatNameId(allAgentsMap[agentId].name, agentId);
    return `未知角色[${agentId.substring(0, 8)}]`;
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
            const calTime = s.formatted_time || "-";
            return `
        <div class="timeline-item">
            <div class="timeline-dot"></div>
            <div class="timeline-content">
                <div class="timeline-header">
                    <span class="timeline-calendar">${escapeHtml(calTime)}</span>
                    <span class="timeline-agent">
                        ${escapeHtml(agentName)}
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
