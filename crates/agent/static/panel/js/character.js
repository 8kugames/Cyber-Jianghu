// 角色信息页逻辑

let currentPage = 1;
let memoryPage = 1;
let dreamRecordPage = 1;
const PAGE_LIMIT = 20;
let hasMore = false;
let hasMoreMemories = false;
let hasMoreDreamRecords = false;
let allCharacters = [];
let attributeMeta = null; // 从 /api/v1/attribute-meta 加载的属性分类
let _expLoadSeq = 0; // 经历日志请求序号，防竞态

// 每日摘要
let summaryPage = 1;
let hasMoreSummary = false;

function switchExpSubTab(sub) {
  document.querySelectorAll('.sub-tab-btn').forEach(b => b.classList.remove('active'));
  document.querySelectorAll('.sub-tab-content').forEach(c => c.classList.remove('active'));
  document.querySelector(`.sub-tab-btn[data-sub-tab="${sub}"]`).classList.add('active');
  document.getElementById('exp-sub-' + sub).classList.add('active');
  if (sub === 'summary') {
    loadDailySummaries();
  }
}

async function loadDailySummaries(page = 1) {
  const listEl = document.getElementById('daily-summaries');
  const loadMoreEl = document.getElementById('load-more-summary');
  if (page === 1) {
    listEl.innerHTML = '<p class="loading-text">加载中...</p>';
  }
  try {
    const data = await apiGet(`/api/v1/memory/daily-summaries?page=${page}&limit=${PAGE_LIMIT}`);
    hasMoreSummary = data.has_more;
    summaryPage = page;
    if (page === 1) listEl.innerHTML = '';
    if (data.summaries && data.summaries.length > 0) {
      data.summaries.forEach(s => {
        const div = document.createElement('div');
        div.className = 'summary-item';
        const tickId = s.tick_id || '-';
        const content = s.content || '';
        // 从内容中提取游戏日（如 "游戏日 505 摘要：" -> "505"）
        const dayMatch = content.match(/游戏日\s*(\d+)/);
        const gameDay = dayMatch ? `第${dayMatch[1]}日` : '';
        div.innerHTML = `
          <div class="summary-item-header">
            <span class="summary-item-day">${gameDay} Tick ${tickId}</span>
          </div>
          <div class="summary-item-content">${escapeHtml(content)}</div>`;
        listEl.appendChild(div);
      });
    } else if (page === 1) {
      listEl.innerHTML = '<p class="no-data">暂无每日摘要</p>';
    }
    setVisible(loadMoreEl, hasMoreSummary);
  } catch (err) {
    listEl.innerHTML = `<p class="error-text">加载失败: ${err.message}</p>`;
  }
}

function loadMoreDailySummaries() {
  const btn = document.getElementById('load-more-summary-btn');
  if (btn) { btn.disabled = true; btn.textContent = '加载中...'; }
  loadDailySummaries(summaryPage + 1).finally(() => {
    if (btn) { btn.disabled = false; btn.textContent = '加载更多'; }
  });
}

const STATUS_MAP = {
  alive: { label: "存活", treeLabel: "存活", tag: "" },
  dead: { label: "死亡", treeLabel: "已故", tag: " [已故]" },
  retired: { label: "归隐", treeLabel: "归隐", tag: " [归隐]" },
};
function statusOf(s) {
  return STATUS_MAP[s] || { label: s, treeLabel: s, tag: "" };
}

let actionTypeMap = {};
async function loadActionTypeMap() {
  try {
    const data = await apiGet("/api/v1/actions");
    if (data && typeof data === "object") {
      actionTypeMap = data;
    }
  } catch (e) {
    console.warn("[actions] Failed to load action type map:", e);
  }
}
function getActionTypeDisplay(actionType) {
  return actionTypeMap[actionType] || actionType;
}

// formatWorldTime / getShichen 由 shared.js 提供

function formatRealTime(ts) {
  if (!ts) return "-";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return String(ts);
  return d.toLocaleString("zh-CN");
}

// 加载属性元数据（分类信息，从 narrative_config 解析）
async function loadAttributeMeta() {
  try {
    const res = await fetch("/api/v1/attribute-meta");
    // 503 = config 未就绪，保持 null 以便重试机制工作
    if (res.status === 503) return;
    const data = await res.json();
    // 仅缓存非空结果，避免 {} 绕过 !attributeMeta 守卫
    if (data && data.display_names && Object.keys(data.display_names).length > 0) {
      attributeMeta = data;
    }
  } catch (err) {
    console.error("加载属性元数据失败:", err);
  }
}

// 加载所有角色列表
async function loadCharacterList() {
  try {
    const data = await apiGet("/api/v1/characters");
    allCharacters = data.characters || [];
    const selectEl = document.getElementById("character-select");
    const serverEl = document.getElementById("current-server");
    const selectorSection = document.getElementById(
      "character-selector-section",
    );

    serverEl.textContent = data.current_server_url || "-";

    const aliveCharacters = allCharacters.filter((c) => c.status === "alive");
    if (aliveCharacters.length <= 1) {
      selectorSection.classList.add("hidden");
    } else {
      selectEl.innerHTML = allCharacters
        .map((c) => {
          const statusText = statusOf(c.status).tag;
          const serverInfo = c.server_url
            ? ` (${c.server_url.replace(/^https?:\/\//, "").split("/")[0]})`
            : "";
          const selected = c.is_current ? "selected" : "";
          const disabled = c.status !== "alive" ? "disabled" : "";
          return `<option value="${c.agent_id || ""}" ${selected} ${disabled}>${escapeHtml(c.name)}${statusText}${serverInfo}</option>`;
        })
        .join("");

      selectorSection.classList.remove("hidden");
    }

    renderWorldTree();
  } catch (err) {
    console.error("加载角色列表失败:", err);
  }
}

// 渲染世界树
function renderWorldTree() {
  const listEl = document.getElementById("world-tree-list");
  if (!allCharacters || allCharacters.length === 0) {
    listEl.innerHTML = '<p class="no-data">暂无角色记录</p>';
    return;
  }

  // 按服务器分组
  const serverGroups = {};
  allCharacters.forEach((c) => {
    const serverKey = c.server_url || "unknown";
    if (!serverGroups[serverKey]) {
      serverGroups[serverKey] = [];
    }
    serverGroups[serverKey].push(c);
  });

  // 生成服务器分组HTML
  let html = "";
  Object.entries(serverGroups).forEach(([serverKey, chars]) => {
    const serverName = serverKey.replace(/^https?:\/\//, "").split("/")[0];
    const firstChar = chars[0];
    const lastRealTime = firstChar.last_connected_real_time
      ? new Date(firstChar.last_connected_real_time).toLocaleString("zh-CN")
      : "-";
    const lastWorldTime = formatWorldTime(firstChar.last_connected_world_time);

    html += `
            <div class="server-group">
                <div class="server-group-header">
                    <span class="server-name">${escapeHtml(serverName)}</span>
                    <span class="server-meta">
                        <span class="meta-item" title="最近连接">最近连接: ${lastRealTime} ｜</span>
                        <span class="meta-item" title="游戏时间">游戏时间: ${escapeHtml(lastWorldTime)}</span>
                    </span>
                </div>
                <div class="server-group-chars">
        `;

    chars.forEach((c) => {
      const statusClass = c.status;
      const statusText = statusOf(c.status).treeLabel;
      const currentLabel = c.is_current
        ? '<span class="current-label">当前</span>'
        : "";
      const registeredAt = c.registered_at
        ? new Date(c.registered_at).toLocaleDateString("zh-CN")
        : "";
      html += `
                <div class="world-tree-card ${c.is_current ? "current" : ""}" data-agent-id="${c.agent_id || ""}">
                    <div class="char-name">
                        ${escapeHtml(c.name || "未知")}
                        ${currentLabel}
                    </div>
                    <div class="char-status ${statusClass}">${statusText}</div>
                    <div class="char-meta">${registeredAt}</div>
                </div>
            `;
    });

    html += `
                </div>
            </div>
        `;
  });

  listEl.innerHTML = html;

  listEl.querySelectorAll(".world-tree-card").forEach((card) => {
    card.addEventListener("click", () => {
      const agentId = card.dataset.agentId;
      const char = allCharacters.find((c) => c.agent_id === agentId);
      if (!char) return;
      openCharacterDrawer(char);
    });
  });
}

// 渲染抽屉内的经历日志（与经历日志 Tab 保持一致）
function renderDrawerSoulCycles(recordsMap, immMap) {
  const tickIds = Object.keys(recordsMap).sort((a, b) => Number(b) - Number(a));
  if (tickIds.length === 0) {
    return '<p class="no-data">暂无经历记录</p>';
  }

  let html = '<div class="drawer-experiences">';
  tickIds.forEach((tickId) => {
    const attempts = recordsMap[tickId];
    const first = attempts[0];
    const worldTimeText = formatWorldTime(first.world_time);
    const realTimeText = formatRealTime(first.created_at);

    html += `<div class="tick-card">
            <div class="tick-card-header">
                <span class="tick-badge">T${tickId}</span>
                <span class="tick-world-time">${escapeHtml(worldTimeText)}</span>
                <span class="tick-real-time">${escapeHtml(realTimeText)}</span>
            </div>`;

    // 行动分区
    html +=
      '<div class="tick-section"><div class="tick-section-title">行动</div>';
    attempts.forEach((a, idx) => {
      if (attempts.length > 1) {
        html += `<div class="tick-attempt-label">行动 ${idx + 1}</div>`;
      }
      html += renderSoulInline("人魂", a.renhun, "renhun");
      html += renderSoulInline("天魂", a.tianhun, "tianhun");
      if (a.final_intent) {
        html += renderSoulInline("地魂", a.final_intent, "action");
      }
    });
    html += "</div>";

    // 即时分区
    const immIntents = immMap[tickId] || [];
    if (immIntents.length > 0) {
      html += '<div class="tick-section tick-section-immediate">';
      html += '<div class="tick-section-title">即时</div>';
      immIntents.forEach((imm) => {
        html += `<div class="imm-item">
                    <div class="exp-immediate"><span class="exp-soul-label">即时</span><span class="exp-soul-content">${escapeHtml(getActionTypeDisplay(imm.action_type))}${imm.speech_content ? ": " + escapeHtml(imm.speech_content) : ""}</span></div>
                    <span class="imm-status ${imm.send_status === "sent" ? "sent" : "failed"}">${imm.send_status === "sent" ? "已发送" : "失败"}</span>
                    ${imm.send_error ? `<span class="imm-error">${escapeHtml(imm.send_error)}</span>` : ""}
                </div>`;
      });
      html += "</div>";
    }

    html += "</div>";
  });
  html += "</div>";
  return html;
}

// 打开角色抽屉
async function openCharacterDrawer(char) {
  const drawer = document.getElementById("character-drawer");
  const overlay = document.getElementById("character-drawer-overlay");
  const body = document.getElementById("char-drawer-body");
  const title = document.getElementById("char-drawer-title");

  body.innerHTML = '<p class="loading-text">加载中...</p>';
  title.textContent = "角色信息";

  drawer.classList.add("open");
  overlay.classList.add("open");

  try {
    await loadCharacterIntoDrawer(char);
  } catch (err) {
    body.innerHTML = `<p class="error-text">加载失败: ${err.message}</p>`;
  }
}

// 关闭角色抽屉
function closeCharacterDrawer() {
  const drawer = document.getElementById("character-drawer");
  const overlay = document.getElementById("character-drawer-overlay");
  drawer.classList.remove("open");
  overlay.classList.remove("open");
}

// 加载角色数据到抽屉
async function loadCharacterIntoDrawer(char) {
  const body = document.getElementById("char-drawer-body");
  const isCurrent = char.is_current;

  let charData = char;

  // 当前角色从 /api/v1/character 取完整数据，非当前角色从 /api/v1/characters/:id 取
  try {
    if (isCurrent) {
      charData = await apiGet("/api/v1/character");
    } else if (char.agent_id) {
      charData = await apiGet(`/api/v1/characters/${char.agent_id}`);
    }
  } catch (err) {
    console.warn("获取角色详情失败，使用列表数据:", err);
  }

  const statusClass = charData.status || "alive";
  const statusText = statusOf(charData.status).label;
  const registeredAt = charData.registered_at
    ? formatRealTime(charData.registered_at)
    : "未知";
  const serverName = charData.server_url
    ? charData.server_url.replace(/^https?:\/\//, "").split("/")[0]
    : "-";

  // 在线状态
  const isStale = charData.is_stale;
  const onlineStatus =
    charData.status === "alive"
      ? isStale
        ? '<span class="online-tag offline">离线</span>'
        : '<span class="online-tag online">在线</span>'
      : "";

  // 位置
  const location = charData.location || "-";

  let html = `
        <div class="character-hero">
            <div class="hero-main">
                <div class="hero-avatar" aria-hidden="true">魂</div>
                <div class="hero-text">
                    <div class="hero-name">${escapeHtml(charData.name || "未知")}</div>
                    <div class="hero-status">
                        <span class="status-badge ${statusClass}"><span class="status-dot"></span>${statusText}</span>
                        ${onlineStatus}
                    </div>
                    <div class="hero-meta">
                        <span class="hero-meta-label">性别</span>
                        <span class="hero-meta-value">${escapeHtml(charData.gender || "-")}</span>
                        <span class="hero-meta-sep">·</span>
                        <span class="hero-meta-label">年龄</span>
                        <span class="hero-meta-value">${charData.age || "-"}</span>
                    </div>
                </div>
            </div>
            <div class="hero-stats">
                <div class="hero-stat">
                    <span class="hero-stat-label">Agent ID</span>
                    <span class="hero-stat-value" style="font-family: monospace; font-size: 0.9em;">${escapeHtml(charData.agent_id || "-")}</span>
                </div>
                <div class="hero-stat">
                    <span class="hero-stat-label">服务器</span>
                    <span class="hero-stat-value">${escapeHtml(serverName)}</span>
                </div>
                <div class="hero-stat">
                    <span class="hero-stat-label">位置</span>
                    <span class="hero-stat-value">${escapeHtml(location)}</span>
                </div>
                <div class="hero-stat">
                    <span class="hero-stat-label">注册时间</span>
                    <span class="hero-stat-value">${registeredAt}</span>
                </div>
            </div>
        </div>
    `;

  // 传记：已死亡/归隐角色展示纪传体传记
  const isDeadOrRetired = charData.status === "dead" || charData.status === "retired";
  if (isDeadOrRetired) {
    html += await loadBiographySection(char.agent_id);
  }

  if (charData.appearance || charData.identity) {
    html += `
            <section class="drawer-section">
                <div class="drawer-section-title">人物画像</div>
                ${charData.appearance ? `<div class="info-item full-width"><span class="label">外貌</span><span class="value">${escapeHtml(charData.appearance)}</span></div>` : ""}
                ${charData.identity ? `<div class="info-item full-width"><span class="label">身份背景</span><span class="value">${escapeHtml(charData.identity)}</span></div>` : ""}
            </section>
        `;
  }

  if (
    (charData.personality && charData.personality.length > 0) ||
    (charData.values && charData.values.length > 0)
  ) {
    html += `
            <section class="drawer-section">
                <div class="drawer-section-title">性格与价值观</div>
                ${charData.personality && charData.personality.length > 0 ? `<div class="tag-list" style="margin-top: 8px;">${charData.personality.map((p) => `<span class="info-tag">${escapeHtml(p)}</span>`).join("")}</div>` : ""}
                ${charData.values && charData.values.length > 0 ? `<div class="tag-list" style="margin-top: 8px;">${charData.values.map((v) => `<span class="info-tag">${escapeHtml(v)}</span>`).join("")}</div>` : ""}
            </section>
        `;
  }

  // 已掌握技能
  if (charData.skills && charData.skills.length > 0) {
    html += `
            <section class="drawer-section">
                <div class="drawer-section-title">已习得技能</div>
                <div class="tag-list" style="margin-top: 8px;">${charData.skills.map((s) => `<span class="info-tag skill-tag" title="${escapeHtml(s.skill_id)}">${escapeHtml(s.name)}</span>`).join("")}</div>
            </section>
        `;
  }

  // 属性（使用 generateAttributesHtml 统一渲染）
  if (charData.attributes) {
    const attrHtml = generateAttributesHtml(
      charData.attributes,
      charData.derived_attributes,
    );
    if (attrHtml) {
      html += `
                <section class="drawer-section">
                    <div class="drawer-section-title">属性</div>
                    ${attrHtml}
                </section>
            `;
    }
  }

  // 记忆关系
  if (isCurrent) {
    try {
      const relData = await apiGet("/api/v1/relationship/list");
      if (relData.relationships && relData.relationships.length > 0) {
        const relList = relData.relationships
          .slice(0, 5)
          .map((r) => {
            const level = r.relationship_label || "陌生人";
            const fav = r.favorability ?? 0;
            return `<div class="rel-mini-item">
                        <span class="rel-name">${escapeHtml(r.target_name || "未知")}</span>
                        <span class="rel-level ${r.relationship_level || "neutral"}">${escapeHtml(level)}</span>
                        <span class="rel-fav">${fav > 0 ? "+" : ""}${fav}</span>
                    </div>`;
          })
          .join("");
        html += `
                    <section class="drawer-section">
                        <div class="drawer-section-title">记忆关系</div>
                        <div class="rel-mini-list">${relList}</div>
                    </section>
                `;
      }
    } catch (err) {
      console.warn("加载记忆关系失败:", err);
    }
  }

  // 经历日志（所有角色均可查看，通过 agent_id 加载各自 SQLite）
  try {
    const expData = await apiGet(
      "/api/v1/character/soul-cycles?agent_id=" +
        char.agent_id +
        "&page=1&limit=50",
    );
    const recordsMap = expData.records || {};
    const immMap = expData.immediate_intents || {};
    const hasMore = expData.has_more;
    const total = expData.total || 0;
    let expHtml = renderDrawerSoulCycles(recordsMap, immMap);
    if (hasMore) {
      expHtml += `<p class="no-data" style="margin-top:8px;">另有 ${total - 50} 条更早记录（完整记录在历史页）</p>`;
    }
    html += `
            <section class="drawer-section">
                <div class="drawer-section-title">经历日志</div>
                ${expHtml}
            </section>
        `;
  } catch (err) {
    console.warn("加载经历日志失败:", err);
    html += `
            <section class="drawer-section">
                <div class="drawer-section-title">经历日志</div>
                <p class="no-data">暂无经历记录</p>
            </section>
        `;
  }

  // 托梦记录
  if (isCurrent) {
    try {
      const dreamData = await apiGet(
        "/api/v1/character/dream/records?page=1&limit=3",
      );
      if (dreamData.records && dreamData.records.length > 0) {
        const dreamList = dreamData.records
          .map((d) => {
            const time = d.injected_at ? formatRealTime(d.injected_at) : "";
            const content = d.thought || "-";
            return `<div class="dream-mini-item">
                        <span class="dream-tick">${time}</span>
                        <span class="dream-content">${escapeHtml(content.substring(0, 25))}${content.length > 25 ? "..." : ""}</span>
                    </div>`;
          })
          .join("");
        html += `
                    <section class="drawer-section">
                        <div class="drawer-section-title">托梦记录</div>
                        <div class="dream-mini-list">${dreamList}</div>
                    </section>
                `;
      }
    } catch (err) {
      console.warn("加载托梦记录失败:", err);
    }
  }

  // 持有物品（非当前角色仅在有数据时显示）
  if (charData.inventory) {
    if (Array.isArray(charData.inventory) && charData.inventory.length > 0) {
      html += `
                <section class="drawer-section">
                    <div class="drawer-section-title">持有物品</div>
                    <div class="inventory-list">
                        ${charData.inventory
                          .map(
                            (item) => `
                            <div class="inv-item">
                                <span class="inv-name">${escapeHtml(item.name || item.item_id || "未知物品")}</span>
                                <span class="inv-qty">x${item.quantity || 1}</span>
                            </div>
                        `,
                          )
                          .join("")}
                    </div>
                </section>
            `;
    }
  } else if (!isCurrent) {
    html += `
            <section class="drawer-section">
                <div class="drawer-section-title">持有物品</div>
                <p class="no-data">非当前角色，无实时物品数据</p>
            </section>
        `;
  }

  body.innerHTML = html;
}

// 加载传记区块 HTML（用于 drawer 中 dead/retired 角色）
async function loadBiographySection(agentId) {
  const placeholder = `
    <section class="drawer-section biography-section" id="biography-section">
      <div class="drawer-section-title">传记</div>
      <div class="biography-loading">
        <span class="biography-loading-dot"></span>
        <span class="biography-loading-dot"></span>
        <span class="biography-loading-dot"></span>
        <span style="margin-left:8px;color:var(--text-muted);font-size:12px;">正在撰写传记...</span>
      </div>
    </section>
  `;

  // 先返回占位，延迟异步填充（占位 HTML 尚未挂载 DOM，需等渲染后执行）
  requestAnimationFrame(function () { loadBiographyAsync(agentId); });
  return placeholder;
}

// 异步加载/生成传记并更新 DOM
async function loadBiographyAsync(agentId) {
  const section = document.getElementById("biography-section");
  if (!section) return;

  try {
    // 先尝试获取缓存
    let data = await apiGet(`/api/v1/character/biography?agent_id=${agentId}`);
    if (!data.biography) {
      // 无缓存，触发生成
      try {
        data = await apiPost(`/api/v1/character/biography?agent_id=${agentId}`, {});
      } catch (genErr) {
        section.innerHTML = `<div class="drawer-section-title">传记</div><p class="no-data">传记生成失败：${escapeHtml(genErr.message)}</p>`;
        return;
      }
    }

    if (data.biography) {
      section.innerHTML = `
        <div class="drawer-section-title">传记</div>
        <div class="biography-content">${escapeHtml(data.biography)}</div>
      `;
    } else {
      section.innerHTML = `<div class="drawer-section-title">传记</div><p class="no-data">暂无传记数据</p>`;
    }
  } catch (err) {
    section.innerHTML = `<div class="drawer-section-title">传记</div><p class="no-data">传记加载失败</p>`;
  }
}

// 切换角色
async function switchCharacter() {
  const selectEl = document.getElementById("character-select");
  const agentId = selectEl.value;
  if (!agentId) return;

  const currentChar = allCharacters.find((c) => c.agent_id === agentId);
  if (currentChar && currentChar.is_current) return;

  try {
    const data = await apiPost("/api/v1/characters/switch", {
      agent_id: agentId,
    });
    if (data.success) {
      loadCharacter();
      loadRelationships();
    } else {
      showError(data.message || "切换角色失败");
      const currentChar = allCharacters.find((c) => c.is_current);
      if (currentChar) selectEl.value = currentChar.agent_id;
    }
  } catch (err) {
    showError("切换角色失败: " + err.message);
  }
}

// 加载角色信息
async function loadCharacter() {
  hide(".error");
  hide("#character-info");
  hide("#experiences-section");
  show("#loading");

  try {
    const data = await apiGet("/api/v1/character");

    // 基本信息
    document.getElementById("name").textContent = data.name || "-";
    document.getElementById("age").textContent = data.age || "-";
    document.getElementById("gender").textContent = data.gender || "-";
    document.getElementById("identity").textContent = data.identity || "-";
    document.getElementById("appearance").textContent = data.appearance || "-";
    document.getElementById("location").textContent = data.location || "-";
    document.getElementById("tick-id").textContent = data.tick_id || "-";
    document.getElementById("agent-id").textContent = data.agent_id || "-";
    document.getElementById("server-url").textContent = data.server_url
      ? data.server_url.replace(/^https?:\/\//, "").split("/")[0]
      : "-";

    if (data.status) {
      const statusEl = document.getElementById("status");
      const text = statusOf(data.status).label;
      const onlineTag =
        data.status === "alive"
          ? data.is_stale
            ? '<span class="online-tag offline">离线</span>'
            : '<span class="online-tag online">在线</span>'
          : "";
      statusEl.innerHTML =
        '<span class="status-badge ' +
        data.status +
        '"><span class="status-dot"></span>' +
        text +
        "</span>" +
        onlineTag;

      // 死亡角色显示常驻提示气泡
      const deathNotice = document.getElementById("death-notice");
      if (deathNotice) {
        if (data.status === "dead") {
          deathNotice.classList.remove("hidden");
        } else {
          deathNotice.classList.add("hidden");
        }
      }
    }

    // 注册时间
    if (data.registered_at) {
      document.getElementById("registered-at").textContent = formatDateTime(
        data.registered_at,
      );
    }

    // 游戏时间
    if (data.world_time) {
      document.getElementById("world-time").textContent = formatWorldTime(
        data.world_time,
      );
    }

    // 性格标签
    const personalityEl = document.getElementById("personality");
    personalityEl.innerHTML =
      data.personality && data.personality.length > 0
        ? data.personality
            .map((p) => `<span class="info-tag">${escapeHtml(p)}</span>`)
            .join("")
        : "-";

    // 价值观标签
    const valuesEl = document.getElementById("values");
    valuesEl.innerHTML =
      data.values && data.values.length > 0
        ? data.values
            .map((v) => `<span class="info-tag">${escapeHtml(v)}</span>`)
            .join("")
        : "-";

    // 技能标签
    const skillsEl = document.getElementById("skills");
    skillsEl.innerHTML =
      data.skills && data.skills.length > 0
        ? data.skills
            .map((s) => `<span class="info-tag skill-tag" title="${escapeHtml(s.skill_id || "")}">${escapeHtml(s.name)}</span>`)
            .join("")
        : "-";

    // 属性
    renderAttributes(data.attributes, data.derived_attributes);

    // 物品（修复 XSS）
    renderInventory(data.inventory);

    hide("#loading");
    show("#character-info");
    show("#experiences-section");
    loadExperiences();
  } catch (err) {
    hide("#loading");
    // 角色未注册（转生后或首次访问），显示提示并切到世界树
    if (err.message.includes("角色尚未注册") || err.message.includes("412")) {
      document.getElementById("character-info").innerHTML = `
                <div class="form-section">
                    <h2>当前无活跃角色</h2>
                    <p class="section-desc">角色已归隐或尚未创建。</p>
                    <div class="form-actions">
                        <a href="create.html" class="nav-link">创建新角色</a>
                    </div>
                </div>
            `;
      show("#character-info");
      // 切到世界树 tab
      document
        .querySelectorAll(".page-tab")
        .forEach((t) => t.classList.remove("active"));
      document.querySelector('[data-tab="worldtree"]').classList.add("active");
      document
        .querySelectorAll(".tab-content")
        .forEach((c) => c.classList.remove("active"));
      document.getElementById("tab-worldtree").classList.add("active");
    } else {
      document.getElementById("error-message").textContent = err.message;
      show(".error");
    }
  }
}

// 渲染单个属性行
function renderAttrItem(key, attr, withMax) {
  let name = key;
  // attributeMeta.display_names 是显示名的权威来源，优先于 attr.name
  if (
    attributeMeta &&
    attributeMeta.display_names &&
    attributeMeta.display_names[key]
  ) {
    name = attributeMeta.display_names[key];
  } else if (attr && attr.name) {
    name = attr.name;
  }

  if (attr && typeof attr === "object" && attr.current !== undefined) {
    if (withMax && attr.max !== undefined && attr.max !== null) {
      const pct =
        attr.max > 0 ? Math.round((attr.current / attr.max) * 100) : 0;
      const cls =
        pct > 70 ? "attr-high" : pct > 30 ? "attr-medium" : "attr-low";
      return `<div class="attr-item ${cls}" title="${escapeHtml(attr.description || "")}"><span class="attr-name">${escapeHtml(name)}</span><span class="attr-value">${attr.current}/${attr.max}</span></div>`;
    }
    const displayVal =
      typeof attr.current === "number" && !Number.isInteger(attr.current)
        ? attr.current.toFixed(3)
        : attr.current;
    return `<div class="attr-item" title="${escapeHtml(attr.description || "")}"><span class="attr-name">${escapeHtml(name)}</span><span class="attr-value">${displayVal}</span></div>`;
  }
  // 兜底：原始数值型属性（非 enriched）
  if (attr !== undefined && attr !== null && typeof attr !== "object") {
    const displayVal =
      typeof attr === "number" && !Number.isInteger(attr)
        ? attr.toFixed(3)
        : attr;
    return `<div class="attr-item"><span class="attr-name">${escapeHtml(name)}</span><span class="attr-value">${displayVal}</span></div>`;
  }
  return "";
}

function generateAttributesHtml(attributes, derivedAttributes) {
  if (!attributes) return "";

  let html = "";

  // Helper to get category
  const getCategory = (key) => {
    if (attributeMeta && attributeMeta.categories) {
      for (const [cat, keys] of Object.entries(attributeMeta.categories)) {
        if (keys.includes(key)) return cat;
      }
    }
    // attributeMeta 未加载时默认归入 primary，避免 warn 且保持功能正常
    return "primary";
  };

  const primary = [];
  const status = [];
  const derived = [];
  const other = [];

  // Process attributes
  Object.entries(attributes).forEach(([key, val]) => {
    if (key.endsWith("_max")) return;
    const cat = getCategory(key);
    if (cat === "primary") primary.push([key, val]);
    else if (cat === "status") status.push([key, val]);
    else if (cat === "derived") derived.push([key, val]);
    else other.push([key, val]);
  });

  // Process derivedAttributes
  if (derivedAttributes) {
    Object.entries(derivedAttributes).forEach(([key, val]) => {
      const cat = getCategory(key);
      if (cat === "primary") primary.push([key, val]);
      else if (cat === "status") status.push([key, val]);
      else derived.push([key, val]);
    });
  }

  if (primary.length > 0) {
    html +=
      '<div class="attr-section"><h4>先天属性</h4><div class="attr-group">';
    primary.forEach(([k, v]) => {
      html += renderAttrItem(k, v, v && v.max !== undefined);
    });
    html += "</div></div>";
  }
  if (status.length > 0) {
    html +=
      '<div class="attr-section"><h4>状态属性</h4><div class="attr-group">';
    status.forEach(([k, v]) => {
      html += renderAttrItem(k, v, true);
    });
    html += "</div></div>";
  }
  if (derived.length > 0) {
    html +=
      '<div class="attr-section"><h4>派生属性</h4><div class="attr-group">';
    derived.forEach(([k, v]) => {
      html += renderAttrItem(k, v, false);
    });
    html += "</div></div>";
  }
  if (other.length > 0) {
    html +=
      '<div class="attr-section"><h4>其他属性</h4><div class="attr-group">';
    other.forEach(([k, v]) => {
      html += renderAttrItem(k, v, v && v.max !== undefined);
    });
    html += "</div></div>";
  }

  return html;
}

// 渲染属性（含分类和无分类兜底）
function renderAttributes(attributes, derivedAttributes) {
  const attrsEl = document.getElementById("attributes");
  if (!attributes) {
    attrsEl.innerHTML = '<p class="no-data">暂无属性数据</p>';
    return;
  }
  attrsEl.innerHTML = generateAttributesHtml(attributes, derivedAttributes);
}

// 渲染物品（XSS 修复）
function renderInventory(inventory) {
  const invEl = document.getElementById("inventory");
  if (!inventory || inventory.length === 0) {
    invEl.innerHTML = '<p class="no-data">暂无物品</p>';
    return;
  }

  // 使用 textContent 避免 XSS
  invEl.innerHTML = "";
  inventory.forEach((item) => {
    const div = document.createElement("div");
    div.className = "inv-item";
    const nameSpan = document.createElement("span");
    nameSpan.className = "inv-name";
    nameSpan.textContent = item.name || item.item_id;
    const qtySpan = document.createElement("span");
    qtySpan.className = "inv-qty";
    qtySpan.textContent = `x${item.quantity || 1}`;
    div.appendChild(nameSpan);
    div.appendChild(qtySpan);
    invEl.appendChild(div);
  });
}

// 加载经历日志（按 Tick 卡片展示，三魂数据内联）
async function loadExperiences(page = 1) {
  const seq = ++_expLoadSeq;
  const expEl = document.getElementById("experiences");
  const loadMoreEl = document.getElementById("load-more");

  if (page === 1) {
    expEl.innerHTML = '<p class="loading-text">加载中...</p>';
  }

  try {
    const data = await apiGet(
      `/api/v1/character/soul-cycles?page=${page}&limit=${PAGE_LIMIT}`,
    );
    if (seq !== _expLoadSeq) return; // 过期请求，丢弃
    hasMore = data.has_more;
    currentPage = page;

    if (page === 1) expEl.innerHTML = "";

    const recordsMap = data.records || {};
    const immMap = data.immediate_intents || {};
    const tickIds = Object.keys(recordsMap).sort(
      (a, b) => Number(b) - Number(a),
    );

    if (tickIds.length > 0) {
      tickIds.forEach((tickId) => {
        const attempts = recordsMap[tickId];
        const div = document.createElement("div");
        div.className = "tick-card";

        const first = attempts[0];
        const worldTimeText = formatWorldTime(first.world_time);
        const realTimeText = formatRealTime(first.created_at);

        let html = `<div class="tick-card-header">
                    <span class="tick-badge">T${tickId}</span>
                    <span class="tick-world-time">${escapeHtml(worldTimeText)}</span>
                    <span class="tick-real-time">${escapeHtml(realTimeText)}</span>
                </div>`;

        // 行动分区
        html += `<div class="tick-section">
                    <div class="tick-section-title">行动</div>`;

        attempts.forEach((a, idx) => {
          if (attempts.length > 1) {
            html += `<div class="tick-attempt-label">行动 ${idx + 1}</div>`;
          }

          // 人魂：感知与思考
          html += renderSoulInline("人魂", a.renhun, "renhun");
          // 天魂：三层审查
          html += renderSoulInline("天魂", a.tianhun, "tianhun");
          // 最终行动
          if (a.final_intent) {
            html += renderSoulInline("地魂", a.final_intent, "action");
          }
        });

        html += `</div>`;

        // 即时分区
        const immIntents = immMap[tickId] || [];
        if (immIntents.length > 0) {
          html += `<div class="tick-section tick-section-immediate">
                        <div class="tick-section-title">即时</div>`;
          immIntents.forEach((imm) => {
            html += `<div class="imm-item">
                            <div class="exp-immediate"><span class="exp-soul-label">即时</span><span class="exp-soul-content">${escapeHtml(getActionTypeDisplay(imm.action_type))}${imm.speech_content ? ": " + escapeHtml(imm.speech_content) : ""}</span></div>
                            <span class="imm-status ${imm.send_status === "sent" ? "sent" : "failed"}">${imm.send_status === "sent" ? "已发送" : "失败"}</span>
                            ${imm.send_error ? `<span class="imm-error">${escapeHtml(imm.send_error)}</span>` : ""}
                        </div>`;
          });
          html += `</div>`;
        }

        div.innerHTML = html;
        expEl.appendChild(div);
      });
    } else if (page === 1) {
      expEl.innerHTML = '<p class="no-data">暂无经历记录</p>';
    }

    setVisible(loadMoreEl, hasMore);
  } catch (err) {
    expEl.innerHTML = `<p class="error-text">加载失败: ${err.message}</p>`;
  }
}

// 天魂三层审查标签中文映射
const LAYER_NAMES = {
  layer1: "动作审查",
  layer2: "规则校验",
  layer3: "意图审查",
};

// 渲染单魂/行动内联区块
function renderSoulInline(label, data, type) {
  if (!data) return "";
  let html = `<div class="exp-${type}"><span class="exp-soul-label">${label}</span><div class="exp-soul-content">`;

  if (type === "renhun") {
    // 人魂：叙事 + 思考过程
    if (data.narrative) {
      html += `<div class="soul-text">${escapeHtml(data.narrative)}</div>`;
    }
    if (data.thought_log) {
      html += `<div class="soul-thought">${escapeHtml(data.thought_log)}</div>`;
    }
  } else if (type === "tianhun") {
    // 天魂：审查结果 + 三层详情 + 理由
    if (data.result) {
      const isApproved = data.result === "approved";
      html += `<div class="soul-result ${isApproved ? "approved" : "rejected"}">${isApproved ? "通过" : "驳回"}</div>`;
    }
    if (data.layers && data.layers.length > 0) {
      html += `<div class="soul-layers">`;
      data.layers.forEach((l) => {
        const cls = l.passed ? "passed" : "failed";
        const name = LAYER_NAMES[l.layer] || l.layer;
        html += `<span class="soul-layer-tag ${cls}">${name}${l.passed ? "" : ": " + escapeHtml(l.detail || "")}</span>`;
      });
      html += `</div>`;
    }
    if (data.reason) {
      html += `<div class="soul-reason">${escapeHtml(data.reason)}</div>`;
    }
    if (data.narrative) {
      html += `<div class="soul-narrative">${escapeHtml(data.narrative)}</div>`;
    }
  } else if (type === "action") {
    // 地魂：最终行动，speak/whisper 特殊展示
    if (data.action_type) {
      const at = data.action_type;
      const ad =
        data.action_data && typeof data.action_data === "object"
          ? data.action_data
          : {};
      const content = ad.content || "";
      const targetId = ad.target_agent_id;
      const resolveName = (id) => {
        if (!id) return null;
        const c = allCharacters.find((c) => c.agent_id === id);
        const shortId = id.substring(0, 8);
        return c ? `${c.name}（${shortId}）` : `${shortId}...`;
      };

      if (at === "speak") {
        const targetName = targetId ? resolveName(targetId) : "众人";
        html += `<div class="soul-text">对 ${escapeHtml(targetName)} 说话："${escapeHtml(content)}"</div>`;
      } else if (at === "whisper") {
        const targetName = targetId ? resolveName(targetId) : "某人";
        html += `<div class="soul-text">向 ${escapeHtml(targetName)} 密语："${escapeHtml(content)}"</div>`;
      } else if (at === "shout") {
        html += `<div class="soul-text">大声喊道："${escapeHtml(content)}"</div>`;
      } else {
        html += `<div class="soul-text">${escapeHtml(getActionTypeDisplay(at))}`;
        if (Object.keys(ad).length > 0) {
          html += ` <span class="soul-params">${escapeHtml(JSON.stringify(ad))}</span>`;
        }
        html += `</div>`;
      }
      // 混沌标记徽章
      if (data.chaos_marker) {
        const cm = data.chaos_marker;
        const chaosLabel =
          cm.type === "Sanity" ? "陷入混乱(低理智)" : "陷入混乱(LLM配额耗尽)";
        html += `<div class="chaos-badge" style="margin-top:4px;"><span class="chaos-tag">${escapeHtml(chaosLabel)}</span></div>`;
      }
      // 托梦影响徽章
      if (data.dream_marker) {
        const thought = data.dream_marker.thought || '';
        html += `<div class="dream-badge" style="margin-top:4px;"><span class="dream-tag">受托梦影响</span>${thought ? ' <span style="color:#8b949e;font-size:12px;">' + escapeHtml(thought) + '</span>' : ''}</div>`;
      }
    }
  }
  html += `</div></div>`;
  return html;
}

function loadMoreExperiences() {
  const btn = document.getElementById('load-more-experiences-btn');
  if (btn) { btn.disabled = true; btn.textContent = '加载中...'; }
  loadExperiences(currentPage + 1).finally(() => {
    if (btn) { btn.disabled = false; btn.textContent = '加载更多'; }
  });
}

// 加载关系列表（紧凑卡片）
async function loadRelationships() {
  const relEl = document.getElementById("relationships");
  relEl.innerHTML = '<p class="loading-text">加载中...</p>';
  try {
    const data = await apiGet("/api/v1/relationship/list");
    if (data.relationships && data.relationships.length > 0) {
      relEl._relationships = data.relationships;
      renderRelationshipCards(relEl, data.relationships);
    } else {
      relEl.innerHTML = '<p class="no-data">暂无关系记录</p>';
    }
  } catch (err) {
    relEl.innerHTML = '<p class="error-text">加载关系失败</p>';
  }
}

// 统一名字解析：卡片和 Modal 共用
function resolveDisplayName(rel) {
  const name = rel.target_name || "";
  const label = rel.relationship_label || "陌生人";
  const isGeneric = !name || name === "陌生人" || name === "未知";
  const labelIsStranger = label === "陌生人" || label === "未知";
  if (!isGeneric) return name;
  if (labelIsStranger) {
    return rel.target_agent_id
      ? "陌生人 " + String(rel.target_agent_id).substring(0, 8)
      : "未知";
  }
  return rel.target_agent_id
    ? String(rel.target_agent_id).substring(0, 8) + "..."
    : "未知";
}

function renderRelationshipCards(container, rels) {
  container.innerHTML = rels
    .map((rel, idx) => {
      const fav = rel.favorability ?? 0;
      const level = rel.relationship_level || "neutral";
      const label = rel.relationship_label || "陌生人";
      const pct = Math.max(0, Math.min(100, Math.round(((fav + 100) / 200) * 100)));
      const displayName = resolveDisplayName(rel);
      const initial = (rel.target_name && rel.target_name !== "陌生人" && rel.target_name !== "未知")
        ? rel.target_name.charAt(0) : "?";
      return `
        <div class="rel-card" data-rel-id="${rel.target_agent_id || idx}">
            <div class="rel-card-avatar ${level}">${escapeHtml(initial)}</div>
            <div class="rel-card-name" title="${escapeHtml(displayName)}">${escapeHtml(displayName)}</div>
            <span class="rel-card-label ${level}">${escapeHtml(label)}</span>
            <div class="rel-card-favor">
                <div class="rel-card-favor-bar">
                    <div class="rel-card-favor-fill ${level}" style="width:${pct}%"></div>
                </div>
                <span class="rel-card-favor-value">${fav}</span>
            </div>
        </div>`;
    }).join("");

  container.querySelectorAll(".rel-card").forEach((card) => {
    card.addEventListener("click", () => {
      const id = card.dataset.relId;
      const rel = container._relationships.find(
        (r) => (r.target_agent_id || "") === id,
      );
      if (rel) openRelationshipModal(rel);
    });
  });
}

// 打开关系详情 Modal
async function openRelationshipModal(rel) {
  if (!rel) return;

  const fav = rel.favorability ?? 0;
  const level = rel.relationship_level || "neutral";
  const label = rel.relationship_label || "陌生人";
  const pct = Math.max(0, Math.min(100, Math.round(((fav + 100) / 200) * 100)));

  // 基本信息
  document.getElementById("modal-rel-name").textContent = resolveDisplayName(rel);
  const labelEl = document.getElementById("modal-rel-label");
  labelEl.textContent = label;
  labelEl.className = "drawer-label " + level;

  // Agent ID
  const agentIdEl = document.getElementById("modal-rel-agent-id");
  agentIdEl.textContent = rel.target_agent_id || "-";

  // 好感度
  const fillEl = document.getElementById("modal-rel-favor-fill");
  fillEl.style.width = pct + "%";
  fillEl.className = "favorability-fill " + level;
  document.getElementById("modal-rel-favor-value").textContent = fav;

  // 关系描述
  document.getElementById("modal-rel-description").textContent =
    rel.self_description || "暂无描述";

  // 沟通记录：从 soul-cycles 提取 whisper/speak 针对该目标的内容
  await loadDialogueHistory(rel.target_agent_id);

  // 关键事件
  const eventsEl = document.getElementById("modal-rel-events");
  const events = rel.key_events || [];
  if (events.length > 0) {
    const sorted = [...events].sort(
      (a, b) => (b.tick_id || 0) - (a.tick_id || 0),
    );
    eventsEl.innerHTML = sorted
      .map((evt) => {
        const delta = evt.favorability_delta || 0;
        const deltaCls =
          delta > 0 ? "positive" : delta < 0 ? "negative" : "neutral";
        const deltaSign = delta > 0 ? "+" : "";
        return `
            <div class="drawer-event">
                <div class="drawer-event-header">
                    <span class="drawer-event-type">${escapeHtml(evt.event_type || "事件")}</span>
                    <span class="drawer-event-delta ${deltaCls}">${deltaSign}${delta}</span>
                </div>
                <div class="drawer-event-desc">${escapeHtml(evt.description || "")}</div>
                <div class="drawer-event-tick">Tick ${evt.tick_id || "-"}</div>
            </div>`;
      })
      .join("");
  } else {
    eventsEl.innerHTML = '<p class="no-data">暂无关键事件</p>';
  }

  // 打开 Modal
  document.getElementById("relationship-modal-overlay").classList.add("open");
}

// 从 soul-cycles 提取与某目标的沟通记录（whisper/speak）
async function loadDialogueHistory(targetAgentId) {
  const section = document.getElementById("modal-dialogue-section");
  const listEl = document.getElementById("modal-dialogue-list");
  if (!targetAgentId) {
    section.style.display = "none";
    return;
  }

  listEl.innerHTML = '<p class="loading-text">加载中...</p>';
  section.style.display = "";

  try {
    const data = await apiGet(
      "/api/v1/character/soul-cycles?page=1&limit=200"
    );
    const recordsMap = data.records || {};
    const tickIds = Object.keys(recordsMap).sort((a, b) => Number(b) - Number(a));

    const dialogues = [];
    tickIds.forEach((tickId) => {
      const attempts = recordsMap[tickId];
      attempts.forEach((a) => {
        // 检查最终行动中的 whisper/speak
        if (a.final_intent) {
          const at = a.final_intent.action_type;
          const ad = (a.final_intent.action_data && typeof a.final_intent.action_data === "object")
            ? a.final_intent.action_data : {};
          const content = ad.content || "";
          const tid = ad.target_agent_id;

          // whisper 必须匹配 target_agent_id；speak 是公共说话，无 target，不纳入二人沟通记录
          if (at === "whisper" && tid === targetAgentId) {
            dialogues.push({
              tick_id: tickId,
              action_type: at,
              content,
              direction: "sent",
            });
          }
        }

        // 也检查即时意图
        // (immediate_intents 单独返回，此处暂不处理)
      });
    });

    if (dialogues.length > 0) {
      listEl.innerHTML = dialogues.slice(0, 50).map((d) => {
        const actionLabel = d.action_type === "whisper" ? "密语" : "说话";
        return `
          <div class="dialogue-item">
            <div class="dialogue-item-header">
              <span class="dialogue-tick">T${d.tick_id}</span>
              <span class="dialogue-action">${escapeHtml(actionLabel)}</span>
            </div>
            <div class="dialogue-content">${escapeHtml(d.content)}</div>
          </div>`;
      }).join("");
    } else {
      listEl.innerHTML = '<p class="no-data">暂无沟通记录</p>';
      section.style.display = "none";
    }
  } catch (err) {
    listEl.innerHTML = '<p class="no-data">暂无沟通记录</p>';
    section.style.display = "none";
  }
}

function closeRelationshipModal() {
  document.getElementById("relationship-modal-overlay").classList.remove("open");
}

// 加载近期记忆
async function loadMemories(page = 1) {
  const memEl = document.getElementById("memories");
  const loadMoreEl = document.getElementById("load-more-memories");

  if (page === 1) {
    memEl.innerHTML = '<p class="loading-text">加载中...</p>';
  }

  try {
    const data = await apiGet(
      `/api/v1/memory/recent?page=${page}&limit=${PAGE_LIMIT}`,
    );
    hasMoreMemories = data.has_more;
    memoryPage = page;

    if (page === 1) memEl.innerHTML = "";

    if (data.memories && data.memories.length > 0) {
      data.memories.forEach((mem) => {
        const div = document.createElement("div");
        div.className = "mem-item";
        const tickSpan = document.createElement("span");
        tickSpan.className = "mem-tick";
        tickSpan.textContent = `Tick ${mem.tick_id || "-"}`;
        const contentDiv = document.createElement("div");
        contentDiv.className = "mem-content";
        contentDiv.textContent = mem.content || "";
        div.appendChild(tickSpan);
        div.appendChild(contentDiv);
        memEl.appendChild(div);
      });
    } else if (page === 1) {
      memEl.innerHTML = '<p class="no-data">暂无记忆记录</p>';
    }

    setVisible(loadMoreEl, hasMoreMemories);
  } catch (err) {
    memEl.innerHTML = `<p class="error-text">加载失败: ${err.message}</p>`;
  }
}

function loadMoreMemories() {
  const btn = document.getElementById('load-more-memories-btn');
  if (btn) { btn.disabled = true; btn.textContent = '加载中...'; }
  loadMemories(memoryPage + 1).finally(() => {
    if (btn) { btn.disabled = false; btn.textContent = '加载更多'; }
  });
}

function loadMoreDreamRecords() {
  loadDreamRecords(dreamRecordPage + 1);
}

// 页面加载
document.addEventListener("DOMContentLoaded", () => {
  // SSE 连接：实时接收死亡事件（仅对存活角色启用）
  let deathEventSource = null;
  let sseReconnectTimer = null;
  function connectDeathEvents() {
    deathEventSource = new EventSource("/api/v1/events");
    deathEventSource.addEventListener("connected", () => {
      // SSE 连接成功
    });
    deathEventSource.addEventListener("agent_died", (e) => {
      // 死亡后立即关闭 SSE，停止重连
      deathEventSource.close();
      deathEventSource = null;
      if (sseReconnectTimer) { clearTimeout(sseReconnectTimer); sseReconnectTimer = null; }
      // 停止周期性刷新
      stopRefreshTimer();

      try {
        const data = JSON.parse(e.data);
        showError("角色已死亡：" + (data.description || "你已经死亡"));
        showDeathModal(data);
      } catch (err) {
        showError("角色已死亡");
        showDeathModal(null);
      }
      // 同步更新 death-notice 气泡（与 Modal 同时显示，避免视觉跳跃）
      const deathNotice = document.getElementById("death-notice");
      if (deathNotice) deathNotice.classList.remove("hidden");
      // 同步更新世界树中当前角色的状态
      const currentChar = allCharacters.find((c) => c.is_current);
      if (currentChar) {
        currentChar.status = "dead";
        const card = document.querySelector(
          `.world-tree-card[data-agent-id="${currentChar.agent_id || ""}"]`,
        );
        if (card) {
          const statusEl = card.querySelector(".char-status");
          if (statusEl) {
            statusEl.className = "char-status dead";
            statusEl.textContent = "已故";
          }
        }
      }
    });
    deathEventSource.addEventListener("heartbeat", () => {
      // 连接存活，无需操作
    });
    deathEventSource.addEventListener("tick_update", () => {
      // 防抖：避免短时间内多次刷新
      if (window._tickRefreshTimer) clearTimeout(window._tickRefreshTimer);
      window._tickRefreshTimer = setTimeout(async () => {
        // 确保 attributeMeta 已加载且包含有效数据（防止 SSE reconnect 期间竞态）
        if (!attributeMeta || !Object.keys(attributeMeta.display_names || {}).length) {
          await loadAttributeMeta();
        }
        await loadCharacter();
        await loadRelationships();
      }, 1000);
    });
    deathEventSource.onerror = () => {
      console.warn("SSE connection lost, reconnecting...");
      deathEventSource.close();
      if (sseReconnectTimer) clearTimeout(sseReconnectTimer);
      sseReconnectTimer = setTimeout(connectDeathEvents, 5000);
    };
  }

  // 死亡通知弹窗
  function showDeathModal(data) {
    const modal =
      document.getElementById("death-notification-modal") || createDeathModal();
    document.getElementById("death-cause").textContent = data
      ? data.description || "你已经死亡"
      : "你已经死亡";
    modal.style.display = "flex";
  }
  function createDeathModal() {
    const div = document.createElement("div");
    div.id = "death-notification-modal";
    div.className = "dialog-overlay";
    div.innerHTML = `
            <div class="dialog">
                <h3>角色死亡</h3>
                <p id="death-cause">你已经死亡</p>
                <div class="dialog-actions">
                    <button id="death-goto-rebirth" class="btn-primary">前往转生</button>
                    <button id="death-close" class="cancel-btn">关闭</button>
                </div>
            </div>
        `;
    document.body.appendChild(div);
    div.querySelector("#death-goto-rebirth").addEventListener("click", () => {
      // 死亡后直接跳转创建页，无需"归隐"确认
      window.location.href = "create.html";
    });
    div.querySelector("#death-close").addEventListener("click", () => {
      div.style.display = "none";
    });
    div.addEventListener("click", (e) => {
      if (e.target === div) div.style.display = "none";
    });
    return div;
  }

  loadAttributeMeta().then(async () => {
    await loadActionTypeMap();
    await loadCharacterList();
    const currentChar = allCharacters.find((c) => c.is_current);

    // 当前角色非存活时，切换到世界树分页（不建立 SSE 连接）
    if (!currentChar || currentChar.status !== "alive") {
      hide("#loading");
      // 切换到世界树 tab
      document
        .querySelectorAll(".page-tab")
        .forEach((t) => t.classList.remove("active"));
      document.querySelector('[data-tab="worldtree"]').classList.add("active");
      document
        .querySelectorAll(".tab-content")
        .forEach((c) => c.classList.remove("active"));
      document.getElementById("tab-worldtree").classList.add("active");
      return;
    }

    // 仅对存活角色建立 SSE 连接
    connectDeathEvents();

    // 角色数据通过 HTTP API 获取，需等待 attributeMeta 就绪后再渲染属性
    await loadCharacter();
    await loadRelationships();
    await loadMemories();
    await loadDreamStatus();
    await loadDreamRecords();
  });

  document
    .getElementById("load-more-experiences-btn")
    .addEventListener("click", loadMoreExperiences);
  document
    .getElementById("load-more-memories-btn")
    .addEventListener("click", loadMoreMemories);
  document
    .getElementById("load-more-dream-records-btn")
    .addEventListener("click", loadMoreDreamRecords);
  document
    .getElementById("character-select")
    .addEventListener("change", switchCharacter);

  // 关系 Modal 关闭事件
  document
    .getElementById("modal-rel-close")
    .addEventListener("click", closeRelationshipModal);
  document
    .getElementById("relationship-modal-overlay")
    .addEventListener("click", (e) => {
      if (e.target === e.currentTarget) closeRelationshipModal();
    });

  // 角色抽屉关闭事件
  document
    .getElementById("char-drawer-close")
    .addEventListener("click", closeCharacterDrawer);
  document
    .getElementById("character-drawer-overlay")
    .addEventListener("click", closeCharacterDrawer);

  // 页面卸载时清理资源
  window.addEventListener("beforeunload", () => {
    stopRefreshTimer();
    if (deathEventSource) {
      deathEventSource.close();
      deathEventSource = null;
    }
    if (sseReconnectTimer) {
      clearTimeout(sseReconnectTimer);
      sseReconnectTimer = null;
    }
  });

  // ESC 关闭所有抽屉/Modal
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeRelationshipModal();
      closeCharacterDrawer();
    }
  });

  // 横向标签页切换
  document.querySelectorAll(".page-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      const targetTab = tab.dataset.tab;
      document
        .querySelectorAll(".page-tab")
        .forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      document
        .querySelectorAll(".tab-content")
        .forEach((c) => c.classList.remove("active"));
      document.getElementById("tab-" + targetTab).classList.add("active");
    });
  });

  // 加载托梦状态
  async function loadDreamStatus() {
    try {
      const data = await apiGet("/api/v1/character/dream");
      const statusEl = document.getElementById("dream-status");
      if (data.thought && data.remaining_ticks > 0) {
        document.getElementById("current-dream").textContent = data.thought;
        document.getElementById("remaining-ticks").textContent =
          data.remaining_ticks;
        show(statusEl);
      } else {
        hide(statusEl);
      }
    } catch (err) {
      console.error("加载托梦状态失败:", err);
    }
  }

  // 加载托梦记录
  async function loadDreamRecords(page = 1) {
    const recordsEl = document.getElementById("dream-records");
    const loadMoreEl = document.getElementById("load-more-dream-records");

    if (page === 1) {
      recordsEl.innerHTML = '<p class="loading-text">加载中...</p>';
    }

    try {
      const data = await apiGet(
        `/api/v1/character/dream/records?page=${page}&limit=${PAGE_LIMIT}`,
      );
      hasMoreDreamRecords = data.has_more;
      dreamRecordPage = page;

      if (page === 1) recordsEl.innerHTML = "";

      if (data.records && data.records.length > 0) {
        data.records.forEach((record) => {
          const div = document.createElement("div");
          div.className = "exp-item";

          let html = `
                        <div class="exp-header">
                            <span class="exp-tick">${formatDateTime(record.injected_at)}</span>
                        </div>
                        <div class="exp-content">${escapeHtml(record.thought)}</div>
                        <div style="margin-top: 8px; font-size: 12px; color: var(--text-muted);">
                            持续: ${record.duration} 回合
                        </div>
                    `;
          div.innerHTML = html;
          recordsEl.appendChild(div);
        });
      } else if (page === 1) {
        recordsEl.innerHTML = '<p class="no-data">暂无托梦记录</p>';
      }

      setVisible(loadMoreEl, hasMoreDreamRecords);
    } catch (err) {
      recordsEl.innerHTML = `<p class="error-text">加载失败: ${err.message}</p>`;
    }
  }

  // 垂直标签页切换
  document.querySelectorAll(".vertical-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      const targetTab = tab.dataset.verticalTab;
      document
        .querySelectorAll(".vertical-tab")
        .forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      document
        .querySelectorAll(".vertical-tab-content")
        .forEach((c) => c.classList.remove("active"));
      document
        .getElementById("vertical-tab-" + targetTab)
        .classList.add("active");

      // 切换到近期记忆 tab 时刷新记忆数据
      if (targetTab === "recent-memories") {
        loadMemories();
      }
      // 切换到已知关系 tab 时刷新关系数据
      if (targetTab === "relationships") {
        loadRelationships();
      }
    });
  });

  document.getElementById('load-more-summary-btn').addEventListener('click', loadMoreDailySummaries);

  // 自动重生 toggle
  (async () => {
    const cb = document.getElementById('auto-rebirth-toggle');
    const desc = document.getElementById('auto-rebirth-desc');
    if (!cb) return;
    try {
      const data = await apiGet('/api/v1/config/auto-rebirth');
      cb.checked = !!data.auto_rebirth;
      if (desc) desc.textContent = cb.checked ? '角色死亡后将自动转世重生' : '自动重生已关闭，角色死亡后需手动操作';
    } catch (_) { /* keep default */ }
    cb.addEventListener('change', async () => {
      try {
        await apiPost('/api/v1/config/auto-rebirth', { auto_rebirth: cb.checked });
        if (desc) desc.textContent = cb.checked ? '角色死亡后将自动转世重生' : '自动重生已关闭，角色死亡后需手动操作';
      } catch (_) { cb.checked = !cb.checked; }
    });
  })();

  // 转生按钮
  const rebirthBtn = document.getElementById("rebirth-btn");
  if (rebirthBtn) {
    rebirthBtn.addEventListener("click", async () => {
      if (!confirm("确定要让当前角色转生吗？此操作不可撤销。")) return;
      rebirthBtn.disabled = true;
      rebirthBtn.textContent = "转生中...";
      try {
        const data = await apiPost("/api/v1/character/rebirth", {
          confirm: true,
        });
        if (data.success) {
          // 隐藏死亡 modal（如果存在）
          const deathModal = document.getElementById(
            "death-notification-modal",
          );
          if (deathModal) deathModal.style.display = "none";
          document.getElementById("rebirth-message").textContent = data.message;
          show(document.getElementById("rebirth-result"));
          rebirthBtn.textContent = "已转生";
          // 直接跳转到创建页面
          setTimeout(() => {
            window.location.href = "create.html";
          }, 1000);
        } else {
          document.getElementById("rebirth-error-msg").textContent =
            data.message || "服务器错误";
          show(document.getElementById("rebirth-error"));
          rebirthBtn.disabled = false;
          rebirthBtn.textContent = "确认转生";
        }
      } catch (err) {
        document.getElementById("rebirth-error-msg").textContent =
          "网络错误: " + err.message;
        show(document.getElementById("rebirth-error"));
        rebirthBtn.disabled = false;
        rebirthBtn.textContent = "确认转生";
      }
    });
  }

  // 托梦表单
  const dreamForm = document.getElementById("dream-form");
  if (dreamForm) {
    dreamForm.addEventListener("submit", async (e) => {
      e.preventDefault();
      const btn = document.getElementById("dream-btn");
      const resultEl = document.getElementById("dream-result");
      const errorEl = document.getElementById("dream-error");

      hide(resultEl);
      hide(errorEl);
      btn.disabled = true;
      btn.textContent = "注入中...";

      const thought = document.getElementById("dream-thought").value.trim();
      const duration =
        parseInt(document.getElementById("dream-duration").value) || 5;

      try {
        const data = await apiPost("/api/v1/character/dream", {
          thought,
          duration,
        });
        showSuccess(data.message);
        show(resultEl);
        document.getElementById("dream-thought").value = "";
        loadDreamStatus();
        loadDreamRecords();
      } catch (err) {
        showError(err.message);
        show(errorEl);
      } finally {
        btn.disabled = false;
        btn.textContent = "注入托梦";
      }
    });
  }

  // 自动刷新：每秒 1 次（增量刷新，避免闪烁）
  let lastRefreshTickId = null;
  let lastRefreshLocation = null;
  let lastRefreshWorldTime = null;
  let lastRefreshAttrsHash = null;
  let lastRefreshDerivedHash = null;
  let lastRefreshInvHash = null;
  let refreshTimer = null;
  let refreshInitialized = false;
  function startRefreshTimer() {
    if (refreshTimer) clearInterval(refreshTimer);
    refreshTimer = setInterval(async () => {
      const currentChar = allCharacters.find((c) => c.is_current);
      if (currentChar && currentChar.status === "alive") {
        await incrementalRefresh();
      } else {
        // 角色已死亡或非存活，停止刷新
        stopRefreshTimer();
      }
    }, 1000);
  }
  function stopRefreshTimer() {
    if (refreshTimer) {
      clearInterval(refreshTimer);
      refreshTimer = null;
    }
  }
  startRefreshTimer();

  // 增量刷新：只更新变化的字段
  async function incrementalRefresh() {
    try {
      // 确保 attributeMeta 有效后再渲染属性
      if (!attributeMeta || !Object.keys(attributeMeta.display_names || {}).length) {
        await loadAttributeMeta();
        if (!attributeMeta) return; // 仍未就绪，跳过本轮
      }

      const data = await apiGet("/api/v1/character");

      // 记录当前数据用于下次比较
      const newAttrsHash = JSON.stringify(data.attributes || {});
      const newDerivedHash = JSON.stringify(data.derived_attributes || {});
      const newInvHash = JSON.stringify(data.inventory || []);

      // 数据没变化，跳过（首次刷新必须执行以初始化 last* 变量）
      if (refreshInitialized &&
          data.tick_id === lastRefreshTickId &&
          data.location === lastRefreshLocation &&
          data.world_time === lastRefreshWorldTime &&
          newAttrsHash === lastRefreshAttrsHash &&
          newDerivedHash === lastRefreshDerivedHash &&
          newInvHash === lastRefreshInvHash) return;
      refreshInitialized = true;
      lastRefreshTickId = data.tick_id;
      lastRefreshLocation = data.location;
      lastRefreshWorldTime = data.world_time;
      lastRefreshAttrsHash = newAttrsHash;
      lastRefreshDerivedHash = newDerivedHash;
      lastRefreshInvHash = newInvHash;

      // 更新高频变化字段（tick、位置、时间）
      updateField("tick-id", data.tick_id);
      updateField("location", data.location);
      updateField(
        "world-time",
        data.world_time ? formatWorldTime(data.world_time) : "-",
      );

      // 更新状态
      if (data.status) {
        const text = statusOf(data.status).label;
        const statusEl = document.getElementById("status");
        const onlineTag =
          data.status === "alive"
            ? data.is_stale
              ? '<span class="online-tag offline">离线</span>'
              : '<span class="online-tag online">在线</span>'
            : "";
        statusEl.innerHTML =
          '<span class="status-badge ' +
          data.status +
          '"><span class="status-dot"></span>' +
          text +
          "</span>" +
          onlineTag;

        // 同步世界树中当前角色的状态（局部更新，避免全量重渲染）
        const worldTreeChar = allCharacters.find((c) => c.is_current);
        if (worldTreeChar && worldTreeChar.status !== data.status) {
          worldTreeChar.status = data.status;
          const card = document.querySelector(
            `.world-tree-card[data-agent-id="${worldTreeChar.agent_id || ""}"]`,
          );
          if (card) {
            const statusEl = card.querySelector(".char-status");
            if (statusEl) {
              const info = statusOf(data.status);
              statusEl.className = "char-status " + data.status;
              statusEl.textContent = info.treeLabel;
            }
          }
        }
      }

      // 更新属性（全量重渲染，包含 derived_attributes）
      renderAttributes(data.attributes, data.derived_attributes);

      // 更新物品
      updateInventoryIncremental(data.inventory);

      // 更新关系（增量刷新）
      await refreshRelationshipsIncremental();
    } catch (err) {
      // 忽略刷新错误，静默失败
    }
  }

  // 增量刷新关系（每 5 秒检查一次，避免频繁请求）
  let lastRelationshipCheck = 0;
  let cachedRelationshipCount = 0;
  let cachedRelationshipData = null;

  async function refreshRelationshipsIncremental() {
    const now = Date.now();
    if (now - lastRelationshipCheck < 5000) return; // 最多 5 秒一次
    lastRelationshipCheck = now;

    try {
      const data = await apiGet("/api/v1/relationship/list");
      const rels = data.relationships || [];
      const newCount = rels.length;

      // 关系数量没变化，跳过
      if (newCount === cachedRelationshipCount && cachedRelationshipData) {
        // 但检查 favorability 是否有变化
        let hasChange = false;
        rels.forEach((rel, idx) => {
          const cached = cachedRelationshipData[idx];
          if (cached && cached.favorability !== rel.favorability) {
            hasChange = true;
          }
        });
        if (!hasChange) return;
      }

      cachedRelationshipCount = newCount;
      cachedRelationshipData = rels;

      const relEl = document.getElementById("relationships");
      if (rels.length === 0) {
        relEl.innerHTML = '<p class="no-data">暂无关系记录</p>';
        return;
      }

      relEl._relationships = rels;
      renderRelationshipCards(relEl, rels);
    } catch (err) {
      // 忽略错误
    }
  }

  // 更新字段（带变化检测和视觉反馈）
  function updateField(id, newValue) {
    const el = document.getElementById(id);
    if (!el) return;
    const oldValue = el.textContent;
    if (oldValue !== String(newValue)) {
      el.textContent = newValue;
      // 短暂高亮提示变化
      el.classList.add("value-changed");
      setTimeout(() => el.classList.remove("value-changed"), 300);
    }
  }

  // 增量更新物品（只更新变化的）
  function updateInventoryIncremental(inventory) {
    if (!inventory && inventory !== 0) return;
    const invEl = document.getElementById("inventory");

    if (!inventory || inventory.length === 0) {
      if (!invEl.querySelector(".no-data")) {
        invEl.innerHTML = '<p class="no-data">暂无物品</p>';
      }
      return;
    }

    const currentItems = Array.from(invEl.querySelectorAll(".inv-item")).map(
      (item) => ({
        name: item.querySelector(".inv-name")?.textContent,
        quantity: item.querySelector(".inv-qty")?.textContent,
      }),
    );

    const newItems = inventory.map((item) => ({
      name: item.name,
      quantity: String(item.quantity || 1),
    }));

    // 比较是否相同
    const isSame =
      currentItems.length === newItems.length &&
      currentItems.every(
        (curr, i) =>
          curr.name === newItems[i].name &&
          curr.quantity === newItems[i].quantity,
      );

    if (!isSame) {
      invEl.innerHTML = "";
      inventory.forEach((item) => {
        const div = document.createElement("div");
        div.className = "inv-item";
        const nameSpan = document.createElement("span");
        nameSpan.className = "inv-name";
        nameSpan.textContent = item.name || item.item_id || "未知物品";
        const qtySpan = document.createElement("span");
        qtySpan.className = "inv-qty";
        qtySpan.textContent = `x${item.quantity || 1}`;
        div.appendChild(nameSpan);
        div.appendChild(qtySpan);
        invEl.appendChild(div);
      });
    }
  }
});
