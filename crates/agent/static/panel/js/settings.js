// Settings page: server + LLM config, setup wizard mode

import { API, get, post, getStoredAuthToken } from './api.js';
import { escapeHtml, showSuccess, showError, showWarning, showModal, hideModal, fmtNum } from './ui.js';
import { appState } from './app.js';

export const settingsPage = {
    mount(container) {
        render(container);
        loadData();
        subscribeDecisionEvents();
    },
    unmount() {
        unsubscribeDecisionEvents();
    },
};

// 最近一次 update/status 快照（renderUpdateCard 与按钮 handler 共用）
let lastUpdateStatus = null;
// 最近一次 decision-model/status 快照（renderDecisionCard 与 SSE 进度共用）
let lastDmStatus = null;
// 决策模型下载进度 SSE（EventSource 不支持 header，走 ?token= 通道）
let dmEventSource = null;

async function loadData() {
    const isWizard = !appState.setupStatus?.server_configured || !appState.setupStatus?.llm_configured;
    const [llmConfig, providers, usage, llmDisabled, updateStatus, dmStatus] = await Promise.allSettled([
        get(API.CONFIG_LLM),
        get(API.CONFIG_LLM_PROVIDERS),
        get(API.CONFIG_LLM_USAGE),
        get(API.CONFIG_LLM_DISABLED),
        get(API.UPDATE_STATUS),
        get(API.DECISION_MODEL_STATUS, { retries: 0 }),
    ]);

    // 全局开关状态先行（renderSecondaryCard 的锁定镜像依赖，必须先于卡片渲染）
    window.__llmDisabled = llmDisabled.status === 'fulfilled'
        ? llmDisabled.value?.llm_disabled === true : false;
    window.__secondaryDisabled = llmDisabled.status === 'fulfilled'
        ? llmDisabled.value?.secondary_disabled === true : false;

    // 版本与更新卡片
    if (updateStatus.status === 'fulfilled') {
        lastUpdateStatus = updateStatus.value;
        renderUpdateCard();
    } else {
        setText('s-update-body', '更新状态不可用');
    }

    // 决策模型卡片
    if (dmStatus.status === 'fulfilled') {
        lastDmStatus = dmStatus.value;
    } else {
        lastDmStatus = { enabled: false, status: { state: 'unknown' } };
    }
    renderDecisionCard();

    // Populate server form
    if (appState.setupStatus) {
        const wsInput = document.getElementById('s-ws-url');
        const httpInput = document.getElementById('s-http-url');
        if (wsInput && appState.setupStatus.ws_url) wsInput.value = appState.setupStatus.ws_url;
        if (httpInput && appState.setupStatus.http_url) httpInput.value = appState.setupStatus.http_url;
    }

    // Populate LLM form
    if (llmConfig.status === 'fulfilled') {
        const resp = llmConfig.value;
        const c = resp.actor || resp; // API 返回 {actor: {...}, reflector, ...}，兼容旧格式
        const fields = { 's-provider': c.provider, 's-model': c.model, 's-base-url': c.base_url, 's-api-key': c.api_key, 's-temperature': c.temperature, 's-max-tokens': c.max_tokens, 's-context-window': c.context_window_tokens ?? c.context_window };
        for (const [id, val] of Object.entries(fields)) {
            const el = document.getElementById(id);
            if (el && val != null) el.value = val;
        }
        const streamEl = document.getElementById('s-streaming');
        if (streamEl) streamEl.checked = (c.enable_streaming ?? c.streaming) === true;

        // API Key 状态徽标：后端不回显密钥，has_api_key=true 表示已配置，提示用户留空即可保持原值
        const apiKeyBadge = document.getElementById('s-api-key-badge');
        if (apiKeyBadge) apiKeyBadge.style.display = c.has_api_key === true ? '' : 'none';

        // Advanced
        const advFields = { 's-summary-trigger': c.summary_trigger_ratio, 's-keep-turns': c.keep_recent_turns ?? c.summary_keep_turns, 's-idle-rotate': c.idle_rotate_threshold };
        for (const [id, val] of Object.entries(advFields)) {
            const el = document.getElementById(id);
            if (el && val != null) el.value = val;
        }
        const thinkEl = document.getElementById('s-thinking');
        if (thinkEl && c.enable_thinking != null) thinkEl.value = String(c.enable_thinking);
        const fallbackEl = document.getElementById('s-fallback-models');
        if (fallbackEl && c.fallback_models) fallbackEl.value = c.fallback_models.join('\n');



        // Mode badge
        const badge = document.getElementById('s-mode-badge');
        const mode = resp.runtime_mode || resp.mode || '';
        if (badge && mode) {
            badge.textContent = mode;
            badge.className = `mode-badge ${mode.toLowerCase()}`;
        }

        // Claw mode notice
        const clawNotice = document.getElementById('s-claw-notice');
        if (clawNotice) clawNotice.classList.toggle('visible', mode === 'Claw');

        // LLM 开关组从独立的 llmDisabled promise 读取
        // （/config/llm 响应不含开关字段，必须用 /config/llm-disabled 的结果）
        const toggle = document.getElementById('s-llm-disabled');
        if (toggle && llmDisabled.status === 'fulfilled' && llmDisabled.value?.llm_disabled) {
            toggle.checked = true;
        }

    }

    // Providers dropdown
    if (providers.status === 'fulfilled') {
        const select = document.getElementById('s-provider');
        if (select) {
            select.innerHTML = '';
            (providers.value.providers || []).forEach(p => {
                const opt = document.createElement('option');
                opt.value = p.value;
                opt.textContent = p.label;
                opt.disabled = p.disabled || false;
                if (p.disabled_reason) opt.title = p.disabled_reason;
                select.appendChild(opt);
            });
            // Restore selected value after populating
            if (llmConfig.status === 'fulfilled') {
                const actor = llmConfig.value.actor || llmConfig.value;
                if (actor.provider) select.value = actor.provider;
            }
            select.dispatchEvent(new Event('change'));
            window.__providerOptions = select.innerHTML;
        }
    }

    // 场景分流 + 从模型卡片（在 providers 与全局开关状态就绪后渲染）
    if (llmConfig.status === 'fulfilled') {
        window.__lastSecondaryCfg = llmConfig.value.llm_secondary || null;
        renderScenarioRoutes(llmConfig.value);
        renderSecondaryCard(llmConfig.value);
    }

    // Token stats：按模型维度拆分到主/从两块（usage 按 provider/model 分组返回）
    if (usage.status === 'fulfilled') {
        const data = usage.value;
        const items = Array.isArray(data) ? data : [];
        const cfg = llmConfig.status === 'fulfilled' ? llmConfig.value : null;
        const primaryModel = cfg?.actor?.model;
        const secondaryModel = cfg?.llm_secondary?.model;
        const splitStats = (model) => {
            const agg = { input: 0, output: 0, calls: 0, failures: 0 };
            items.forEach(it => {
                if (model && it.model === model) {
                    agg.input += it.prompt_tokens || 0;
                    agg.output += it.completion_tokens || 0;
                    agg.calls += it.calls || 0;
                    agg.failures += it.failures || 0;
                }
            });
            return agg;
        };
        const p = splitStats(primaryModel);
        setText('s-stat-input', fmtNum(p.input));
        setText('s-stat-output', fmtNum(p.output));
        setText('s-stat-calls', fmtNum(p.calls));
        setText('s-stat-errors', fmtNum(p.failures));
        const sc = splitStats(secondaryModel);
        setText('s2-stat-input', secondaryModel ? fmtNum(sc.input) : '-');
        setText('s2-stat-output', secondaryModel ? fmtNum(sc.output) : '-');
        setText('s2-stat-calls', secondaryModel ? fmtNum(sc.calls) : '-');
        setText('s2-stat-errors', secondaryModel ? fmtNum(sc.failures) : '-');
    }

    // Connection status
    if (appState.setupStatus) {
        const dot = document.getElementById('s-conn-dot');
        const text = document.getElementById('s-conn-text');
        const connected = appState.setupStatus.has_server;
        if (dot) dot.className = `connection-dot ${connected ? 'connected' : 'disconnected'}`;
        if (text) text.textContent = connected ? '已连接' : '未连接';
    }
}

function render(container) {
    const isWizard = !appState.setupStatus?.server_configured || !appState.setupStatus?.llm_configured;

    container.innerHTML = `
    <div class="settings-page">
        <h2>${isWizard ? '初始配置' : '系统设置'}</h2>
        ${isWizard ? '<p class="text-muted" style="margin-bottom:16px">首次使用，请完成以下配置后开始</p>' : ''}

        <section class="settings-section">
            <div class="card">
                <div class="card-header">Server 配置</div>
                <div class="card-body">
                    <form id="server-form">
                        <div class="form-group">
                            <label class="form-label">WebSocket 地址</label>
                            <input class="form-input" type="text" id="s-ws-url" value="ws://localhost:23333/ws" required>
                        </div>
                        <div class="form-group">
                            <label class="form-label">HTTP 地址</label>
                            <input class="form-input" type="text" id="s-http-url" placeholder="http://localhost:23333">
                        </div>
                        <button type="submit" class="btn btn-primary">保存并重连</button>
                    </form>
                </div>
            </div>
        </section>

        <section class="settings-section">
            <div class="card">
                <div class="card-header" style="display:flex;align-items:center;justify-content:space-between;">
                    <span style="display:flex;align-items:center;gap:10px;">
                        主模型
                        <span class="mode-badge cognitive" id="s-mode-badge">Cognitive</span>
                    </span>
                    <label style="display:flex;align-items:center;gap:12px;font-size:12px;color:var(--text-muted)" title="同时停止主模型与从模型的全部 LLM 调用（含重试/降级链）；决策模型（本地 2B）不受影响">
                        <span style="display:flex;align-items:center;gap:6px;cursor:pointer">
                            <input type="checkbox" id="s-llm-disabled"> 停止全部 LLM（主/从）
                        </span>
                        <span style="display:flex;align-items:center;gap:6px;font-size:13px;color:var(--text-secondary)">
                            <span class="connection-dot" id="s-conn-dot"></span>
                            <span id="s-conn-text">未连接</span>
                        </span>
                    </label>
                </div>
                <div class="card-body">
                    <div class="llm-claw-notice" id="s-claw-notice">当前运行在 Claw 模式，无需 LLM 配置（由外部调度器控制）</div>

                    <div class="stats-grid">
                        <div class="stat-card"><div class="stat-value" id="s-stat-input">-</div><div class="stat-label">输入 Token</div></div>
                        <div class="stat-card"><div class="stat-value" id="s-stat-output">-</div><div class="stat-label">输出 Token</div></div>
                        <div class="stat-card"><div class="stat-value" id="s-stat-calls">-</div><div class="stat-label">累计请求</div></div>
                        <div class="stat-card"><div class="stat-value" id="s-stat-errors">-</div><div class="stat-label">错误次数</div></div>
                    </div>

                    <form id="llm-form">
                        <div class="form-group">
                            <label class="form-label" style="font-size:13px;font-weight:600">主模型</label>
                            <div class="text-muted" style="font-size:12px">认知与决策使用的 LLM（Provider + 模型 + 密钥）</div>
                        </div>
                        <div class="form-group">
                            <label class="form-label">Provider</label>
                            <select class="form-select" id="s-provider"></select>
                        </div>
                        <div class="form-group">
                            <label class="form-label">模型</label>
                            <input class="form-input" type="text" id="s-model" placeholder="如: qwen2.5:14b" required>
                        </div>
                        <div class="form-group" id="s-base-url-group">
                            <label class="form-label">Base URL</label>
                            <input class="form-input" type="text" id="s-base-url" placeholder="如: http://localhost:11434">
                        </div>
                        <div class="form-group hidden" id="s-api-key-group">
                            <label class="form-label">API Key <span id="s-api-key-badge" class="badge badge-success" style="display:none;font-size:11px;font-weight:normal;margin-left:6px;">已配置</span></label>
                            <input class="form-input" type="password" id="s-api-key" placeholder="留空则保持原值不变">
                        </div>
                        <div style="display:flex;gap:12px;flex-wrap:wrap">
                            <div class="form-group" style="flex:1;min-width:120px">
                                <label class="form-label">Temperature</label>
                                <input class="form-input" type="number" id="s-temperature" min="0" max="2" step="0.1" value="0.7">
                            </div>
                            <div class="form-group" style="flex:1;min-width:120px">
                                <label class="form-label">最大 Token</label>
                                <input class="form-input" type="number" id="s-max-tokens" min="256" max="32768" step="256" value="8192">
                            </div>
                            <div class="form-group" style="flex:1;min-width:120px">
                                <label class="form-label">上下文窗口</label>
                                <input class="form-input" type="number" id="s-context-window" min="4096" max="1048576" step="1024" value="32768">
                            </div>
                        </div>
                        <div class="form-group">
                            <label style="display:flex;align-items:center;gap:8px;cursor:pointer">
                                <input type="checkbox" id="s-streaming"> 启用流式输出
                            </label>
                        </div>
                        <div class="form-group" style="margin-top:8px;padding:10px;border:1px dashed var(--border);border-radius:6px">
                            <label class="form-label" style="font-size:13px;font-weight:600">备用</label>
                            <textarea class="form-input" id="s-fallback-models" rows="2" placeholder="每行一个模型名称，留空表示无备用"></textarea>
                            <div class="text-muted" style="font-size:12px;margin-top:4px">故障兜底链：主模型 403/超时时自动降级（同 Provider/密钥），连续空闲时按顺序轮换；与下方「从模型」的省钱分流是两套机制</div>
                        </div>
                        
                        <details style="margin-top:12px;border:1px solid var(--border);border-radius:6px;padding:10px">
                            <summary style="cursor:pointer;font-weight:500">高级参数</summary>
                            <div style="margin-top:12px;display:flex;flex-direction:column;gap:12px">
                                <div class="form-group">
                                    <label class="form-label">摘要触发比例</label>
                                    <input class="form-input" type="number" id="s-summary-trigger" min="0.3" max="0.95" step="0.05" value="0.75">
                                </div>
                                <div class="form-group">
                                    <label class="form-label">保留最近轮次</label>
                                    <input class="form-input" type="number" id="s-keep-turns" min="1" max="20" step="1" value="4">
                                </div>
                                <div class="form-group">
                                    <label class="form-label">空闲轮换阈值</label>
                                    <input class="form-input" type="number" id="s-idle-rotate" min="0" max="100" step="1" value="24">
                                </div>
                                <div class="form-group">
                                    <label class="form-label">思考模式</label>
                                    <select class="form-select" id="s-thinking">
                                        <option value="">默认</option>
                                        <option value="true">开启</option>
                                        <option value="false">关闭</option>
                                    </select>
                                </div>
                            </div>
                        </details>
                        <div style="margin-top:16px">
                            <button type="submit" class="btn btn-primary">保存配置</button>
                        </div>
                    </form>
                </div>
            </div>
        </section>

        <section class="settings-section">
            <div class="card">
                <div class="card-header" style="display:flex;align-items:center;justify-content:space-between;">
                    <span style="display:flex;align-items:center;gap:10px;">
                        从模型
                        <span class="mode-badge claw" id="s2-badge" style="display:none;">-</span>
                    </span>
                    <span style="display:flex;align-items:center;gap:12px;">
                        <label style="display:flex;align-items:center;gap:6px;font-size:12px;color:var(--text-muted);cursor:pointer" title="停用=暂停场景分流（全部走主模型），配置保留可随时恢复；勾选后如未配置需先填写并保存">
                            <input type="checkbox" id="s2-enabled"> 启用
                        </label>
                        <button type="button" class="btn btn-sm" id="s2-mirror-btn" style="display:none">恢复跟随主模型</button>
                    </span>
                </div>
                <div class="card-body">
                    <div class="stats-grid" style="margin-bottom:12px">
                        <div class="stat-card"><div class="stat-value" id="s2-stat-input">-</div><div class="stat-label">输入 Token</div></div>
                        <div class="stat-card"><div class="stat-value" id="s2-stat-output">-</div><div class="stat-label">输出 Token</div></div>
                        <div class="stat-card"><div class="stat-value" id="s2-stat-calls">-</div><div class="stat-label">累计请求</div></div>
                        <div class="stat-card"><div class="stat-value" id="s2-stat-errors">-</div><div class="stat-label">错误次数</div></div>
                    </div>
                    <div id="s2-mirror-note" class="text-muted" style="font-size:13px;display:none">
                        当前跟随主模型（未定义独立从模型）。填写下方表单并保存即启用独立从模型。
                    </div>
                    <form id="s2-form">
                        <div style="display:flex;gap:12px;flex-wrap:wrap">
                            <div class="form-group" style="flex:1;min-width:150px">
                                <label class="form-label">Provider</label>
                                <select class="form-select" id="s2-provider"></select>
                            </div>
                            <div class="form-group" style="flex:1;min-width:150px">
                                <label class="form-label">模型</label>
                                <input class="form-input" type="text" id="s2-model" placeholder="如 MiniMax-M2.7">
                            </div>
                        </div>
                        <div class="form-group">
                            <label class="form-label">Base URL</label>
                            <input class="form-input" type="text" id="s2-base-url" placeholder="同 Provider 可留空">
                        </div>
                        <div class="form-group">
                            <label class="form-label">API Key <span id="s2-api-key-badge" class="badge badge-success" style="display:none;font-size:11px;font-weight:normal;margin-left:6px;">已配置</span></label>
                            <input class="form-input" type="password" id="s2-api-key" placeholder="留空则保持原值；同 Provider 可复用主模型密钥">
                        </div>
                        <div style="display:flex;gap:12px;flex-wrap:wrap">
                            <div class="form-group" style="flex:1;min-width:110px">
                                <label class="form-label">Temperature</label>
                                <input class="form-input" type="number" id="s2-temperature" min="0" max="2" step="0.1" value="0.7">
                            </div>
                            <div class="form-group" style="flex:1;min-width:110px">
                                <label class="form-label">最大 Token</label>
                                <input class="form-input" type="number" id="s2-max-tokens" min="64" max="65536" step="64" value="2048">
                            </div>
                            <div class="form-group" style="flex:1;min-width:110px">
                                <label class="form-label">上下文窗口</label>
                                <input class="form-input" type="number" id="s2-context-window" min="4096" max="1048576" step="1024" value="31744">
                            </div>
                        </div>
                        <div class="form-group">
                            <label style="display:flex;align-items:center;gap:8px;cursor:pointer">
                                <input type="checkbox" id="s2-streaming"> 启用流式输出
                            </label>
                        </div>
                        <div class="form-group" style="padding:10px;border:1px dashed var(--border);border-radius:6px">
                            <label class="form-label" style="font-size:13px;font-weight:600">备用</label>
                            <textarea class="form-input" id="s2-fallback-models" rows="2" placeholder="每行一个模型名称，留空表示无备用"></textarea>
                            <div class="text-muted" style="font-size:12px;margin-top:4px">从模型故障兜底链（同从 Provider/密钥）；主模型不受影响</div>
                        </div>
                        <details style="margin-top:12px;border:1px solid var(--border);border-radius:6px;padding:10px">
                            <summary style="cursor:pointer;font-weight:500">高级参数</summary>
                            <div style="margin-top:12px;display:flex;flex-direction:column;gap:12px">
                                <div class="form-group">
                                    <label class="form-label">摘要触发比例</label>
                                    <input class="form-input" type="number" id="s2-summary-trigger" min="0.3" max="0.95" step="0.05" value="0.75">
                                </div>
                                <div class="form-group">
                                    <label class="form-label">保留最近轮次</label>
                                    <input class="form-input" type="number" id="s2-keep-turns" min="1" max="20" step="1" value="4">
                                </div>
                                <div class="form-group">
                                    <label class="form-label">空闲轮换阈值</label>
                                    <input class="form-input" type="number" id="s2-idle-rotate" min="0" max="100" step="1" value="24">
                                </div>
                                <div class="form-group">
                                    <label class="form-label">思考模式</label>
                                    <select class="form-select" id="s2-thinking">
                                        <option value="">默认</option>
                                        <option value="true">开启</option>
                                        <option value="false">关闭</option>
                                    </select>
                                </div>
                            </div>
                        </details>
                        <div style="margin-top:14px;display:flex;gap:8px;align-items:center;flex-wrap:wrap">
                            <button type="submit" class="btn btn-primary">保存从模型</button>
                            <span class="text-muted" style="font-size:12px">保存即定义独立从模型（可跨 Provider）；「场景分流」中选「从」的场景走此模型，失败自动回退主模型</span>
                        </div>
                    </form>
                </div>
            </div>
        </section>

        <section class="settings-section">
            <div class="card">
                <div class="card-header">场景分流（主/从路由）</div>
                <div class="card-body">
                    <div class="text-muted" style="font-size:12px;margin-bottom:10px">
                        每个场景选择走主模型还是从模型（默认：轻量场景走从，主决策与质量敏感场景走主）。
                        「从」需要上方从模型卡片启用（未启用时从即主）。场景分流随任一保存自动热生效。
                    </div>
                    <div id="s-scenario-routes" style="display:flex;flex-direction:column;gap:6px"></div>
                    <div style="margin-top:12px">
                        <button type="button" class="btn btn-primary" id="sr-save-btn">保存场景分流</button>
                    </div>
                </div>
            </div>
        </section>

        <section class="settings-section">
            <div class="card">
                <div class="card-header" style="display:flex;align-items:center;justify-content:space-between;">
                    <span style="display:flex;align-items:center;gap:10px;">
                        决策模型
                        <span class="mode-badge claw" id="s-dm-badge" style="display:none;">-</span>
                    </span>
                    <label style="display:flex;align-items:center;gap:6px;font-size:12px;color:var(--text-muted)">
                        <input type="checkbox" id="s-dm-enabled"> 启用决策模型
                    </label>
                </div>
                <div class="card-body">
                    <div id="s-dm-status-line" class="text-muted" style="font-size:13px">加载中…</div>
                    <div id="s-dm-progress" style="display:none;margin-top:10px">
                        <div style="display:flex;justify-content:space-between;font-size:12px;color:var(--text-muted);margin-bottom:4px">
                            <span id="s-dm-progress-file">-</span>
                            <span id="s-dm-progress-pct">0%</span>
                        </div>
                        <div style="height:8px;border-radius:4px;background:var(--border);overflow:hidden">
                            <div id="s-dm-progress-fill" style="height:100%;width:0%;background:#60a5fa;transition:width .3s"></div>
                        </div>
                    </div>
                    <div id="s-dm-error" style="display:none;margin-top:10px;padding:8px 10px;border-radius:6px;background:rgba(239,68,68,0.1);color:#ef4444;font-size:12px;word-break:break-all"></div>
                    <form id="dm-form" style="margin-top:14px">
                        <div style="display:flex;gap:12px;flex-wrap:wrap">
                            <div class="form-group" style="flex:1;min-width:130px">
                                <label class="form-label">量化档位</label>
                                <select class="form-select" id="s-dm-quant">
                                    <option value="q5_k_m">q5_k_m（推荐）</option>
                                    <option value="q4_k_s">q4_k_s（低内存）</option>
                                </select>
                            </div>
                            <div class="form-group" style="flex:1;min-width:130px">
                                <label class="form-label">置信度门控阈值</label>
                                <input class="form-input" type="number" id="s-dm-threshold" min="0" max="1" step="0.05" value="0.7">
                            </div>
                            <div class="form-group" style="flex:1;min-width:130px">
                                <label class="form-label">单问超时（ms）</label>
                                <input class="form-input" type="number" id="s-dm-timeout" min="1000" max="120000" step="1000" value="30000">
                            </div>
                        </div>
                        <div style="margin-top:12px;display:flex;gap:8px;flex-wrap:wrap">
                            <button type="submit" class="btn btn-primary">保存决策模型配置</button>
                            <button type="button" class="btn" id="s-dm-install-btn">立即下载 / 修复</button>
                        </div>
                        <div class="text-muted" style="font-size:12px;margin-top:8px">
                            启用后自动下载并装配 2B 决策模型（两段式：主模型出认知，决策模型出意图）；未就绪或置信度门控不过时自动回退主模型路径。保存后立即生效，无需重启。
                        </div>
                    </form>
                </div>
            </div>
        </section>

        <section class="settings-section">
            <div class="card">
                <div class="card-header">版本与更新</div>
                <div class="card-body">
                    <div id="s-update-body" class="text-muted">加载中…</div>
                    <div style="margin-top:12px;display:flex;gap:8px">
                        <button type="button" class="btn btn-sm" id="s-update-check-btn">检查更新</button>
                        <button type="button" class="btn btn-sm btn-primary" id="s-update-apply-btn" style="display:none">安装并重启</button>
                    </div>
                </div>
            </div>
        </section>
    </div>
    `;

    bindEvents();
}

function bindEvents() {
    // Provider change → toggle api-key visibility
    document.getElementById('s-provider')?.addEventListener('change', function () {
        const isLocal = this.value === 'ollama';
        const keyGroup = document.getElementById('s-api-key-group');
        if (keyGroup) keyGroup.classList.toggle('hidden', isLocal);
        const baseUrlEl = document.getElementById('s-base-url');
        const modelEl = document.getElementById('s-model');
        if (isLocal) {
            if (baseUrlEl) baseUrlEl.placeholder = 'http://localhost:11434';
            if (modelEl) modelEl.placeholder = '如: qwen2.5:14b 或 llama3.2:3b';
        } else {
            if (baseUrlEl) baseUrlEl.placeholder = '如: https://api.openai.com/v1';
            if (modelEl) modelEl.placeholder = '如: gpt-4o';
        }
    });

    // LLM disabled toggle
    document.getElementById('s-llm-disabled')?.addEventListener('change', async function () {
        try {
            await post(API.CONFIG_LLM_DISABLED, { llm_disabled: this.checked });
            window.__llmDisabled = this.checked;
            // 跟随态：从模型开关镜像主模型状态（锁定不可点，仅同步显示）
            if (!window.__secondaryDefined) {
                const s2cb = document.getElementById('s2-enabled');
                if (s2cb) s2cb.checked = !this.checked;
            }
            showSuccess(this.checked ? 'LLM 已停止' : 'LLM 已恢复');
        } catch (e) {
            showError('操作失败: ' + e.message);
        }
    });

    // Server form submit
    document.getElementById('server-form')?.addEventListener('submit', async (e) => {
        e.preventDefault();
        const btn = e.target.querySelector('button[type="submit"]');
        btn.disabled = true;
        btn.textContent = '保存中...';
        try {
            await post(API.CONFIG_SERVER, {
                ws_url: document.getElementById('s-ws-url')?.value?.trim(),
                http_url: document.getElementById('s-http-url')?.value?.trim(),
            });
            showSuccess('Server 配置已保存');
        } catch (err) {
            showError('保存失败: ' + err.message);
        } finally {
            btn.disabled = false;
            btn.textContent = '保存并重连';
        }
    });

    // LLM form submit
    document.getElementById('llm-form')?.addEventListener('submit', async (e) => {
        e.preventDefault();
        const btn = e.target.querySelector('button[type="submit"]');
        btn.disabled = true;
        btn.textContent = '保存中...';

        const actor = actorFromForm();
        const thinkingVal = document.getElementById('s-thinking')?.value;
        const fallbackText = document.getElementById('s-fallback-models')?.value?.trim();
        actor.enable_thinking = thinkingVal ? thinkingVal === 'true' : null;
        actor.fallback_models = fallbackText ? fallbackText.split('\n').map(x => x.trim()).filter(Boolean) : [];

        try {
            await post(API.CONFIG_LLM, {
                actor,
                reflector: null,
                reflector_inherits_actor: true,
            });
            // 保存仅落盘：reload 重建 LLM 客户端后才真正生效（与其他保存入口一致）
            try {
                await saveAndReload();
                showSuccess('主模型配置已保存并生效');
            } catch (reloadErr) {
                showWarning(`已保存，但热重载失败（${reloadErr.message}），重启 agent 后生效`);
            }
        } catch (err) {
            showError('保存失败: ' + err.message);
        } finally {
            btn.disabled = false;
            btn.textContent = '保存配置';
        }
    });

    // === 从模型 ===
    document.getElementById('s2-form')?.addEventListener('submit', async (e) => {
        e.preventDefault();
        const btn = e.target.querySelector('button[type="submit"]');
        if (btn) { btn.disabled = true; btn.textContent = '保存中…'; }
        const payload = collectSecondary();
        if (!payload.config.model) {
            showError('从模型 model 不能为空');
            if (btn) { btn.disabled = false; btn.textContent = '保存从模型'; }
            return;
        }
        try {
            await post(API.CONFIG_LLM, {
                actor: actorFromForm(),
                reflector: null,
                reflector_inherits_actor: true,
                llm_secondary: payload,
            });
            try {
                await post(API.CONFIG_RELOAD, {}, { timeout: 30000, retries: 0 });
                await loadData();
                showSuccess('从模型已保存并启用（分流场景即刻走从）');
            } catch (reloadErr) {
                showWarning(`已保存，但热重载失败（${reloadErr.message}），重启 agent 后生效`);
            }
        } catch (err) {
            showError('保存从模型失败: ' + err.message);
        } finally {
            if (btn) { btn.disabled = false; btn.textContent = '保存从模型'; }
        }
    });

    document.getElementById('s2-enabled')?.addEventListener('change', async function () {
        // 停用=暂停分流（配置保留）；启用=恢复。未定义从模型时提示先保存。
        if (!window.__secondaryDefined) {
            // 跟随态开关已锁定（disabled），此处为防御兜底：回弹并提示
            this.checked = !window.__llmDisabled;
            showWarning('当前跟随主模型：请在主模型处启停 LLM，或先保存独立从模型');
            return;
        }
        try {
            await post(API.CONFIG_LLM_DISABLED, { secondary_disabled: !this.checked });
            window.__secondaryDisabled = !this.checked;
            showSuccess(this.checked ? '从模型已启用' : '从模型已停用（分流暂停，全部走主模型）');
            // 用最近一次真实配置重渲染（空对象会破坏 provider 下拉与密钥徽标）
            renderSecondaryCard({ llm_secondary: window.__lastSecondaryCfg });
        } catch (e) {
            showError('操作失败: ' + e.message);
        }
    });

    document.getElementById('s2-mirror-btn')?.addEventListener('click', async () => {
        try {
            await post(API.CONFIG_LLM, {
                actor: actorFromForm(),
                reflector: null,
                reflector_inherits_actor: true,
                llm_secondary: { mode: 'mirror' },
            });
            await post(API.CONFIG_RELOAD, {}, { timeout: 30000, retries: 0 });
            await loadData();
            showSuccess('从模型已恢复跟随主模型');
        } catch (e) {
            showError('操作失败: ' + e.message);
        }
    });

    // === 场景分流 ===
    document.getElementById('sr-save-btn')?.addEventListener('click', async () => {
        const btn = document.getElementById('sr-save-btn');
        if (btn) { btn.disabled = true; btn.textContent = '保存中…'; }
        const { routing, skipped } = collectScenarioRoutes();
        if (skipped > 0) showWarning(`${skipped} 条场景 max_tokens 非法（须为正整数），已忽略该字段`);
        if (!Object.keys(routing).length) {
            showError('场景分流表未就绪（配置加载失败），已取消提交以免清空路由');
            if (btn) { btn.disabled = false; btn.textContent = '保存场景分流'; }
            return;
        }
        try {
            await post(API.CONFIG_LLM, {
                actor: actorFromForm(),
                reflector: null,
                reflector_inherits_actor: true,
                scenario_routing: routing,
            });
            try {
                await post(API.CONFIG_RELOAD, {}, { timeout: 30000, retries: 0 });
                await loadData();
                showSuccess('场景分流已保存并生效');
            } catch (reloadErr) {
                showWarning(`已保存，但热重载失败（${reloadErr.message}），重启 agent 后生效`);
            }
        } catch (err) {
            showError('保存场景分流失败: ' + err.message);
        } finally {
            if (btn) { btn.disabled = false; btn.textContent = '保存场景分流'; }
        }
    });

    // === 版本与更新 ===
    const checkBtn = document.getElementById('s-update-check-btn');
    checkBtn?.addEventListener('click', async () => {
        checkBtn.disabled = true;
        const original = checkBtn.textContent;
        checkBtn.textContent = '检查中…';
        try {
            // 服务端同步请求 GitHub（含自身超时），放宽前端超时且不重试以免重复请求
            const r = await post(API.UPDATE_CHECK, {}, { timeout: 30000, retries: 0 });
            if (r.update_available) showSuccess(`发现新版本 ${r.release_tag}`);
            else showSuccess('已是最新');
        } catch (e) {
            showError(e.message);
        }
        await refreshUpdateCard();
        checkBtn.disabled = false;
        checkBtn.textContent = original;
    });

    const applyBtn = document.getElementById('s-update-apply-btn');
    applyBtn?.addEventListener('click', () => {
        const tag = lastUpdateStatus?.latest?.tag_name || '最新版本';
        showModal(`
            <h3 style="margin-bottom:12px">安装更新</h3>
            <p style="margin-bottom:16px;font-size:13px;color:var(--text-secondary)">
                将下载并安装 ${escapeHtml(tag)}，随后进程自动重启，面板会短暂断开。确认继续？
            </p>
            <div style="display:flex;gap:8px;justify-content:flex-end">
                <button class="btn" id="s-update-cancel-btn">取消</button>
                <button class="btn btn-primary" id="s-update-confirm-btn">确认安装</button>
            </div>`);
        document.getElementById('s-update-cancel-btn')?.addEventListener('click', hideModal);
        document.getElementById('s-update-confirm-btn')?.addEventListener('click', async () => {
            const confirmBtn = document.getElementById('s-update-confirm-btn');
            if (confirmBtn) { confirmBtn.disabled = true; confirmBtn.textContent = '下载安装中…'; }
            try {
                // apply 等待下载+校验+安装完成后才响应（20MB 级资产），放宽超时且不重试
                const r = await post(API.UPDATE_APPLY, {}, { timeout: 600000, retries: 0 });
                hideModal();
                if (r.applied) {
                    showWarning(`已安装 ${r.tag}，进程即将重启，面板将短暂断开…`);
                    if (checkBtn) checkBtn.disabled = true;
                    applyBtn.disabled = true;
                } else {
                    showSuccess('已是最新，无需更新');
                    await refreshUpdateCard();
                }
            } catch (e) {
                hideModal();
                showError(e.message);
            }
        });
    });

    // === 决策模型 ===
    document.getElementById('dm-form')?.addEventListener('submit', async (e) => {
        e.preventDefault();
        const btn = e.target.querySelector('button[type="submit"]');
        if (btn) { btn.disabled = true; btn.textContent = '保存中…'; }
        const payload = {
            enabled: document.getElementById('s-dm-enabled')?.checked ?? false,
            quant: document.getElementById('s-dm-quant')?.value || 'q5_k_m',
            // 不能用 || 兜底：threshold=0 是合法值（门控恒过），会被 || 静默改成 0.7
            threshold: Number.isFinite(parseFloat(document.getElementById('s-dm-threshold')?.value))
                ? parseFloat(document.getElementById('s-dm-threshold').value)
                : 0.7,
            timeout_ms: parseInt(document.getElementById('s-dm-timeout')?.value, 10) || 30000,
        };
        try {
            // 换装需等旧 manager 回收与新装配任务拉起，放宽超时且不重试以免重复提交
            const r = await post(API.DECISION_MODEL_CONFIG, payload, { timeout: 15000, retries: 0 });
            if (r.success) {
                showSuccess(r.message || '决策模型配置已保存');
                await refreshDecisionCard();
            } else {
                showError(r.message || '保存失败');
            }
        } catch (err) {
            showError('保存决策模型配置失败: ' + err.message);
        } finally {
            if (btn) { btn.disabled = false; btn.textContent = '保存决策模型配置'; }
        }
    });

    document.getElementById('s-dm-install-btn')?.addEventListener('click', () => {
        const quant = document.getElementById('s-dm-quant')?.value || 'q5_k_m';
        showModal(`
            <h3 style="margin-bottom:12px">下载决策模型</h3>
            <p style="margin-bottom:16px;font-size:13px;color:var(--text-secondary)">
                将按档位 ${escapeHtml(quant)} 从 ModelScope / GitHub 下载并逐文件校验 sha256；
                已完整的文件自动跳过。下载进度在下方实时展示。确认执行？
            </p>
            <div style="display:flex;gap:8px;justify-content:flex-end">
                <button class="btn" id="s-dm-install-cancel">取消</button>
                <button class="btn btn-primary" id="s-dm-install-confirm">确认下载</button>
            </div>`);
        document.getElementById('s-dm-install-cancel')?.addEventListener('click', hideModal);
        document.getElementById('s-dm-install-confirm')?.addEventListener('click', async () => {
            const confirmBtn = document.getElementById('s-dm-install-confirm');
            if (confirmBtn) { confirmBtn.disabled = true; confirmBtn.textContent = '触发中…'; }
            try {
                // 手动安装服务端 spawn 后立即返回，本身是快速操作；不重试以免重复触发下载
                const r = await post(API.DECISION_MODEL_INSTALL, {}, { timeout: 15000, retries: 0 });
                hideModal();
                if (r.success) showSuccess(r.message || '安装任务已触发');
                else showError(r.message || '触发失败');
            } catch (e) {
                hideModal();
                showError('触发安装失败: ' + e.message);
            }
        });
    });
}

function setText(id, text) {
    const el = document.getElementById(id);
    if (el) el.textContent = text;
}

// ============================================================================
// 版本与更新卡片
// ============================================================================

/// 根据 update/status 快照渲染卡片正文与安装按钮可见性
function renderUpdateCard() {
    const st = lastUpdateStatus;
    const body = document.getElementById('s-update-body');
    if (!body || !st) return;

    const badge = updateBadgeInfo(st);
    const shortDigest = st.current_digest
        ? st.current_digest.replace('sha256:', '').slice(0, 12) + '…'
        : '-';
    const lines = [];

    // 环境守卫说明（apply 不可用时的原因）
    if (st.dev_build) lines.push('cargo 本地构建产物，不参与自动更新');
    else if (st.in_container) lines.push('容器内运行，请通过更新镜像升级（build-agent-image.sh）');
    else if (st.hard_disabled) lines.push('自更新已被 CYBER_JIANGHU_SELF_UPDATE=0 禁用');

    if (st.latest) {
        const published = st.latest.published_at
            ? new Date(st.latest.published_at).toLocaleString('zh-CN')
            : '';
        lines.push(`最新 release：${st.latest.tag_name}（${st.latest.asset_name}）${published ? '，发布于 ' + published : ''}`);
    }
    if (st.last_check_unix) {
        lines.push(`上次检查：${new Date(st.last_check_unix * 1000).toLocaleString('zh-CN')}`);
    } else {
        lines.push('尚未检查');
    }
    if (st.last_error) lines.push(`上次检查失败：${st.last_error}`);
    if (st.installed_tag) {
        const at = st.installed_at_unix
            ? new Date(st.installed_at_unix * 1000).toLocaleString('zh-CN')
            : '';
        lines.push(`已安装 ${st.installed_tag}${at ? '（' + at + '）' : ''}，重启后生效`);
    }

    body.innerHTML = `
        <div style="display:flex;align-items:center;gap:10px;flex-wrap:wrap">
            <span style="font-size:13px;color:var(--text-secondary)">当前版本</span>
            <span style="font-weight:600">v${escapeHtml(st.current_version)}</span>
            <span class="mode-badge ${badge.cls}">${badge.text}</span>
        </div>
        <div style="margin-top:6px;font-size:12px;color:var(--text-muted);font-family:monospace">sha256: ${escapeHtml(shortDigest)}</div>
        <div style="margin-top:8px;font-size:13px;color:var(--text-secondary);display:flex;flex-direction:column;gap:4px">
            ${lines.map(l => `<div>${escapeHtml(l)}</div>`).join('')}
        </div>`;

    // 安装按钮：仅在具备自更新条件且确认有新版本时展示
    const applyBtn = document.getElementById('s-update-apply-btn');
    if (applyBtn) {
        const canApply = st.update_available === true && !st.dev_build && !st.in_container && !st.hard_disabled;
        applyBtn.style.display = canApply ? '' : 'none';
    }
}

/// 状态徽标：claw（琥珀）表异常/待处理，cognitive（蓝）表正常
function updateBadgeInfo(st) {
    if (st.dev_build) return { cls: 'claw', text: '本地构建' };
    if (st.in_container) return { cls: 'claw', text: '容器内' };
    if (st.hard_disabled) return { cls: 'claw', text: '已禁用' };
    if (st.update_available === true) return { cls: 'claw', text: '有新版本' };
    if (st.update_available === false) return { cls: 'cognitive', text: '已是最新' };
    return { cls: 'claw', text: '未检查' };
}

/// 重新拉取 update/status 并重渲染卡片（检查/安装动作后调用）
async function refreshUpdateCard() {
    try {
        lastUpdateStatus = await get(API.UPDATE_STATUS);
        renderUpdateCard();
    } catch (e) {
        showWarning('刷新更新状态失败: ' + e.message);
    }
}

// ============================================================================
// 场景分流编辑区（轻量模型路由：键白名单服务端下发，勾选+模型名+输出上限）
// ============================================================================

const SCENARIO_LABELS = {
    think: '主认知决策（不建议分流）',
    reflector_l3: '天魂 L3 审查（观察拒裁率）',
    session_triage: '事件分诊',
    daily_summary: '每日纪要',
    narrative: '记忆叙事合成',
    conversation_summary: '对话历史压缩',
    relationship_eval: '关系评估',
    relationship_narrative: '好感度描述',
    biography: '传记生成',
    character_generation: '角色生成',
};

function renderScenarioRoutes(cfg) {
    const wrap = document.getElementById('s-scenario-routes');
    if (!wrap) return;
    const keys = cfg.scenario_keys || [];
    window.__scenarioKeys = keys;
    const routing = cfg.scenario_routing || {};
    // escapeHtml 不转义引号，属性上下文补一道 &quot; 防逃逸
    const attrEscape = s => escapeHtml(String(s)).replace(/"/g, '&quot;');
    wrap.innerHTML = keys.map(k => {
        const r = routing[k] || {};
        const label = SCENARIO_LABELS[k] || k;
        const via = r.via === 'primary' ? 'primary' : 'secondary';
        return `<div style="display:flex;align-items:center;gap:10px;flex-wrap:wrap">
            <span style="min-width:190px;font-size:12px">${escapeHtml(label)}</span>
            <label style="display:flex;align-items:center;gap:4px;font-size:12px;cursor:pointer">
                <input type="radio" name="sr-${attrEscape(k)}" value="primary" ${via === 'primary' ? 'checked' : ''}> 主
            </label>
            <label style="display:flex;align-items:center;gap:4px;font-size:12px;cursor:pointer">
                <input type="radio" name="sr-${attrEscape(k)}" value="secondary" ${via === 'secondary' ? 'checked' : ''}> 从
            </label>
            <input class="form-input" type="number" placeholder="max_tokens" min="64" step="64" value="${r.max_tokens ?? ''}" data-scenario-tokens="${attrEscape(k)}" style="width:110px;font-size:12px" title="仅工具循环类路径生效；摘要类走从模型自身上限">
        </div>`;
    }).join('');
}

/// 从编辑区收集路由表；返回 { routing, skipped }（max_tokens 非法时忽略该字段）
function collectScenarioRoutes() {
    const routing = {};
    let skipped = 0;
    (window.__scenarioKeys || []).forEach(k => {
        const picked = document.querySelector(`input[name="sr-${k}"]:checked`);
        if (!picked) return;
        const tokensEl = document.querySelector(`[data-scenario-tokens="${k}"]`);
        const t = parseInt(tokensEl?.value, 10);
        const hasTokens = Number.isFinite(t) && t > 0;
        if (!hasTokens && (tokensEl?.value || '').trim() !== '') skipped++;
        routing[k] = hasTokens ? { via: picked.value, max_tokens: t } : { via: picked.value };
    });
    return { routing, skipped };
}

/// 从模型卡片渲染：跟随（未定义）/ 停用 / 启用 三态 + 表单回填
function renderSecondaryCard(cfg) {
    const form = document.getElementById('s2-form');
    const note = document.getElementById('s2-mirror-note');
    const badge = document.getElementById('s2-badge');
    const enableCb = document.getElementById('s2-enabled');
    const mirrorBtn = document.getElementById('s2-mirror-btn');
    if (!form || !note || !badge || !enableCb) return;
    const sec = cfg.llm_secondary || null;
    const disabled = window.__secondaryDisabled === true;
    window.__secondaryDefined = Boolean(sec);

    badge.style.display = '';
    if (!sec) {
        badge.className = 'mode-badge claw';
        badge.textContent = '跟随主模型';
    } else if (disabled) {
        badge.className = 'mode-badge claw';
        badge.textContent = '已停用';
    } else {
        badge.className = 'mode-badge cognitive';
        badge.textContent = '启用中';
    }
    if (sec) {
        // 独立从模型：开关可操作，控制从链暂停/恢复（与全局停止正交）
        enableCb.disabled = false;
        enableCb.title = '停用=暂停场景分流（全部走主模型），配置保留可随时恢复';
        enableCb.checked = !disabled;
    } else {
        // 跟随主模型：从即主，开关锁定并镜像主模型 LLM 状态（必须在主模型处切换）
        enableCb.disabled = true;
        enableCb.title = '跟随主模型：请在主模型处启停 LLM';
        enableCb.checked = !window.__llmDisabled;
    }
    if (mirrorBtn) mirrorBtn.style.display = sec ? '' : 'none';
    note.style.display = sec ? 'none' : '';

    if (sec) {
        const sel = document.getElementById('s2-provider');
        if (sel && window.__providerOptions) {
            sel.innerHTML = window.__providerOptions;
            if (sec.provider) sel.value = sec.provider;
        }
        const fields = {
            's2-model': sec.model, 's2-base-url': sec.base_url,
            's2-temperature': sec.temperature, 's2-max-tokens': sec.max_tokens,
            's2-context-window': sec.context_window_tokens,
            's2-summary-trigger': sec.summary_trigger_ratio, 's2-keep-turns': sec.keep_recent_turns,
            's2-idle-rotate': sec.idle_rotate_threshold,
        };
        for (const [id, val] of Object.entries(fields)) {
            const el = document.getElementById(id);
            if (el && val != null) el.value = val;
        }
        const streamEl = document.getElementById('s2-streaming');
        if (streamEl) streamEl.checked = sec.enable_streaming === true;
        const thinkEl = document.getElementById('s2-thinking');
        if (thinkEl && sec.enable_thinking != null) thinkEl.value = String(sec.enable_thinking);
        const fbEl = document.getElementById('s2-fallback-models');
        if (fbEl && sec.fallback_models) fbEl.value = sec.fallback_models.join('\n');
        const keyBadge = document.getElementById('s2-api-key-badge');
        if (keyBadge) keyBadge.style.display = sec.has_api_key === true ? '' : 'none';
    } else {
        // 未定义：用主模型当前值预填（「默认保持一致」的编辑起点）
        const prov = document.getElementById('s-provider');
        const sel = document.getElementById('s2-provider');
        if (sel && window.__providerOptions) sel.innerHTML = window.__providerOptions;
        if (sel && prov) sel.value = prov.value;
        ['model', 'base-url', 'temperature'].forEach(f => {
            const src = document.getElementById(`s-${f}`);
            const dst = document.getElementById(`s2-${f}`);
            if (src && dst && src.value) dst.value = src.value;
        });
    }
}

/// 从从模型表单收集完整自定义载荷（与主模型同构字段集）
function collectSecondary() {
    const thinkingVal = document.getElementById('s2-thinking')?.value;
    const fallbackText = document.getElementById('s2-fallback-models')?.value?.trim();
    return {
        mode: 'custom',
        config: {
            provider: document.getElementById('s2-provider')?.value || 'openai_compatible',
            model: (document.getElementById('s2-model')?.value || '').trim(),
            base_url: (document.getElementById('s2-base-url')?.value || '').trim(),
            api_key: (document.getElementById('s2-api-key')?.value || '').trim(),
            temperature: parseFloat(document.getElementById('s2-temperature')?.value) || 0.7,
            max_tokens: parseInt(document.getElementById('s2-max-tokens')?.value, 10) || 2048,
            context_window_tokens: parseInt(document.getElementById('s2-context-window')?.value, 10) || 32768,
            enable_streaming: document.getElementById('s2-streaming')?.checked ?? false,
            summary_trigger_ratio: parseFloat(document.getElementById('s2-summary-trigger')?.value) || 0.75,
            keep_recent_turns: parseInt(document.getElementById('s2-keep-turns')?.value, 10) || 4,
            idle_rotate_threshold: parseInt(document.getElementById('s2-idle-rotate')?.value, 10) || 24,
            enable_thinking: thinkingVal ? thinkingVal === 'true' : null,
            fallback_models: fallbackText ? fallbackText.split('\n').map(x => x.trim()).filter(Boolean) : [],
        },
    };
}

/// 主模型表单 → actor 载荷（主/从/场景分流三个保存入口共用；
/// 密钥留空由后端 resolve_api_key 回填已保存值）
function actorFromForm() {
    return {
        provider: document.getElementById('s-provider')?.value,
        model: (document.getElementById('s-model')?.value || '').trim(),
        base_url: (document.getElementById('s-base-url')?.value || '').trim(),
        api_key: (document.getElementById('s-api-key')?.value || '').trim(),
        temperature: parseFloat(document.getElementById('s-temperature')?.value) || 0.7,
        max_tokens: parseInt(document.getElementById('s-max-tokens')?.value, 10) || 8192,
        context_window_tokens: parseInt(document.getElementById('s-context-window')?.value, 10) || 32768,
        enable_streaming: document.getElementById('s-streaming')?.checked ?? false,
        summary_trigger_ratio: parseFloat(document.getElementById('s-summary-trigger')?.value) || 0.75,
        keep_recent_turns: parseInt(document.getElementById('s-keep-turns')?.value, 10) || 4,
        idle_rotate_threshold: parseInt(document.getElementById('s-idle-rotate')?.value, 10) || 24,
    };
}

/// 保存后热生效（reload 重建客户端）并刷新设置页数据
async function saveAndReload() {
    await post(API.CONFIG_RELOAD, {}, { timeout: 30000, retries: 0 });
    await loadData();
}

// ============================================================================
// 决策模型卡片（状态徽标 / 下载进度 SSE / 配置编辑 / 手动安装）
// ============================================================================

/// 状态文本字节数人性化
function fmtBytes(n) {
    if (!Number.isFinite(n) || n < 0) return '-';
    const units = ['B', 'KB', 'MB', 'GB'];
    let i = 0;
    while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
    return n.toFixed(i ? 1 : 0) + ' ' + units[i];
}

/// 生命周期状态 → 徽标（复用 mode-badge 两档语义：cognitive=正常，claw=需关注）
function dmBadgeClass(state) {
    return state === 'ready' || state === 'downloading' ? 'cognitive' : 'claw';
}

function dmBadgeText(state, st) {
    if (state === 'ready') return '已就绪';
    if (state === 'downloading') {
        const pct = st?.total_bytes ? Math.min(100, (st.downloaded_bytes / st.total_bytes) * 100).toFixed(0) : '0';
        return `下载中 ${pct}%`;
    }
    if (state === 'failed') return '安装失败';
    if (state === 'not_installed') return '未安装';
    if (state === 'disabled') return '未启用';
    return '未知';
}

/// 进度条展示（dl=null 隐藏）
function updateDmProgress(dl) {
    const wrap = document.getElementById('s-dm-progress');
    if (!wrap) return;
    if (!dl || !dl.total_bytes) { wrap.style.display = 'none'; return; }
    wrap.style.display = '';
    const pct = Math.min(100, (dl.downloaded_bytes / dl.total_bytes) * 100);
    setText('s-dm-progress-file', dl.file || '-');
    setText('s-dm-progress-pct', `${pct.toFixed(1)}%（${fmtBytes(dl.downloaded_bytes)} / ${fmtBytes(dl.total_bytes)}）`);
    const fill = document.getElementById('s-dm-progress-fill');
    if (fill) fill.style.width = `${pct}%`;
}

/// 根据快照渲染决策模型卡片（表单回填避开正在编辑的控件）
function renderDecisionCard() {
    const st = lastDmStatus || { enabled: false, status: { state: 'unknown' } };
    const inner = st.status || {};
    const state = inner.state || (st.enabled ? 'unknown' : 'disabled');

    const badge = document.getElementById('s-dm-badge');
    if (badge) {
        badge.style.display = '';
        badge.className = `mode-badge ${dmBadgeClass(state)}`;
        badge.textContent = dmBadgeText(state, inner);
    }

    const enableCb = document.getElementById('s-dm-enabled');
    // 回填避开正在编辑的控件（与下方三输入框同口径），防止未保存的拨动被静默回滚
    if (enableCb && document.activeElement !== enableCb) enableCb.checked = st.enabled === true;

    const line = document.getElementById('s-dm-status-line');
    if (line) {
        if (state === 'ready') {
            line.textContent = `版本 ${inner.version} · ${inner.quant} · 已就绪（点击徽标旁开关可停用；换档位后自动重新装配）`;
            line.title = inner.gguf || '';
        } else if (state === 'downloading') {
            line.textContent = `正在下载 ${inner.file || ''}`;
        } else if (state === 'failed') {
            line.textContent = '安装失败，可点击「立即下载 / 修复」重试';
        } else if (state === 'not_installed') {
            line.textContent = '尚未安装，保存启用后自动下载，或点击「立即下载 / 修复」';
        } else if (state === 'disabled') {
            line.textContent = '未启用：决策全部走主模型路径（勾选右上方开关并保存即启用）';
        } else {
            line.textContent = '状态未知（决策路径按主模型回退处理）';
        }
    }

    const errBox = document.getElementById('s-dm-error');
    if (errBox) {
        if (state === 'failed' && inner.error) { errBox.style.display = ''; errBox.textContent = inner.error; }
        else errBox.style.display = 'none';
    }

    const active = document.activeElement;
    const quantSel = document.getElementById('s-dm-quant');
    if (quantSel && st.quant_configured && active !== quantSel) quantSel.value = st.quant_configured;
    const thrInput = document.getElementById('s-dm-threshold');
    if (thrInput && st.threshold != null && active !== thrInput) {
        thrInput.value = Math.round(st.threshold * 100) / 100;
    }
    const toInput = document.getElementById('s-dm-timeout');
    if (toInput && st.timeout_ms != null && active !== toInput) toInput.value = st.timeout_ms;

    updateDmProgress(state === 'downloading' ? inner : null);
}

/// 重新拉取状态并重渲染卡片（保存/进度完成后调用）
async function refreshDecisionCard() {
    try {
        lastDmStatus = await get(API.DECISION_MODEL_STATUS, { retries: 0 });
    } catch (_) {
        // 保持旧快照（卡片上次渲染仍在）
    }
    renderDecisionCard();
}

/// 订阅下载进度 SSE（EventSource 不支持 header，走 ?token= 通道）
function subscribeDecisionEvents() {
    if (dmEventSource) return;
    const token = getStoredAuthToken();
    if (!token) return; // 未认证时不连；页面刷新后随 token 就绪重建
    const url = `${window.location.protocol}//${window.location.host}${API.DECISION_MODEL_EVENTS}?token=${encodeURIComponent(token)}`;
    dmEventSource = new EventSource(url);
    dmEventSource.addEventListener('open', () => { dmSseRetryCount = 0; });
    dmEventSource.addEventListener('download_progress', (e) => {
        try {
            const p = JSON.parse(e.data);
            if (p.done) { confirmDmTerminal(0); return; }
            const badge = document.getElementById('s-dm-badge');
            if (badge) {
                badge.style.display = '';
                badge.className = 'mode-badge cognitive';
                badge.textContent = dmBadgeText('downloading', p);
            }
            updateDmProgress(p);
        } catch (_) { /* 忽略坏帧 */ }
    });
    dmEventSource.addEventListener('disabled', () => {
        // 外部停用（另一标签页保存等）：整卡刷新而非只改徽标，避免状态行/进度条滞留
        refreshDecisionCard();
    });
    dmEventSource.addEventListener('error', () => {
        // EventSource 无法区分 401 与网络错误；超限关停防重连风暴；
        // 置空 dmEventSource 保留重建入口（切页重进 mount 会重新订阅）
        dmSseRetryCount++;
        if (dmSseRetryCount >= DM_SSE_MAX_RETRIES && dmEventSource) {
            dmEventSource.close();
            dmEventSource = null;
            showWarning('决策模型进度连接失败，已停止重试（切换页面重进可恢复）');
        }
    });
    // connected/heartbeat 事件无需处理；未超限的断线由浏览器 EventSource 自动重连
}

/// done 帧后的终态确认轮询：最后帧到达后后端还要解压归档 + 逐文件 sha256 终验
/// （GB 级需数秒），期间 status 仍报 Downloading 满字节视图，且解压/终验失败
/// 不发任何帧——轮询至 ready/failed 才算闭环（上限 10 次 × 2s，超限留旧态）。
function confirmDmTerminal(attempt) {
    refreshDecisionCard().then(() => {
        const state = lastDmStatus?.status?.state;
        if (state === 'downloading' && attempt < 10) {
            setTimeout(() => confirmDmTerminal(attempt + 1), 2000);
        }
    });
}

/// SSE 连续失败上限（对齐 app.js 的重试纪律）：超过后关闭连接停止重连风暴
const DM_SSE_MAX_RETRIES = 5;
let dmSseRetryCount = 0;

function unsubscribeDecisionEvents() {
    if (dmEventSource) {
        dmEventSource.close();
        dmEventSource = null;
    }
}
