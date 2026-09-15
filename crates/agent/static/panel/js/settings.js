// Settings page: server + LLM config, setup wizard mode

import { API, get, post } from './api.js';
import { escapeHtml, showSuccess, showError, showWarning, showModal, hideModal, fmtNum } from './ui.js';
import { appState } from './app.js';

export const settingsPage = {
    mount(container) {
        render(container);
        loadData();
    },
    unmount() {},
};

// 最近一次 update/status 快照（renderUpdateCard 与按钮 handler 共用）
let lastUpdateStatus = null;

async function loadData() {
    const isWizard = !appState.setupStatus?.server_configured || !appState.setupStatus?.llm_configured;
    const [llmConfig, providers, usage, llmDisabled, updateStatus] = await Promise.allSettled([
        get(API.CONFIG_LLM),
        get(API.CONFIG_LLM_PROVIDERS),
        get(API.CONFIG_LLM_USAGE),
        get(API.CONFIG_LLM_DISABLED),
        get(API.UPDATE_STATUS),
    ]);

    // 版本与更新卡片
    if (updateStatus.status === 'fulfilled') {
        lastUpdateStatus = updateStatus.value;
        renderUpdateCard();
    } else {
        setText('s-update-body', '更新状态不可用');
    }

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

        // LLM disabled toggle 从独立的 llmDisabled promise 读取
        // （/config/llm 响应不含 llm_disabled 字段，必须用 /config/llm-disabled 的结果）
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
        }
    }

    // Token stats
    if (usage.status === 'fulfilled') {
        const data = usage.value;
        let totalInput = 0, totalOutput = 0, totalCalls = 0, totalFailures = 0;
        if (Array.isArray(data)) {
            data.forEach(item => {
                totalInput += item.prompt_tokens || 0;
                totalOutput += item.completion_tokens || 0;
                totalCalls += item.calls || 0;
                totalFailures += item.failures || 0;
            });
        }
        setText('s-stat-input', fmtNum(totalInput));
        setText('s-stat-output', fmtNum(totalOutput));
        setText('s-stat-calls', fmtNum(totalCalls));
        setText('s-stat-errors', fmtNum(totalFailures));
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
                        LLM 配置
                        <span class="mode-badge cognitive" id="s-mode-badge">Cognitive</span>
                    </span>
                    <span style="display:flex;align-items:center;gap:12px;">
                        <label style="display:flex;align-items:center;gap:6px;font-size:12px;color:var(--text-muted)">
                            <input type="checkbox" id="s-llm-disabled"> 停止 LLM
                        </label>
                        <span style="display:flex;align-items:center;gap:6px;font-size:13px;color:var(--text-secondary)">
                            <span class="connection-dot" id="s-conn-dot"></span>
                            <span id="s-conn-text">未连接</span>
                        </span>
                    </span>
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
                                <div class="form-group">
                                    <label class="form-label">备用模型列表</label>
                                    <textarea class="form-input" id="s-fallback-models" rows="3" placeholder="每行一个模型名称"></textarea>
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

        const thinkingVal = document.getElementById('s-thinking')?.value;
        const fallbackText = document.getElementById('s-fallback-models')?.value?.trim();

        const config = {
            actor: {
                provider: document.getElementById('s-provider')?.value,
                model: document.getElementById('s-model')?.value?.trim(),
                base_url: document.getElementById('s-base-url')?.value?.trim(),
                api_key: document.getElementById('s-api-key')?.value?.trim(),
                temperature: parseFloat(document.getElementById('s-temperature')?.value) || 0.7,
                max_tokens: parseInt(document.getElementById('s-max-tokens')?.value) || 8192,
                context_window_tokens: parseInt(document.getElementById('s-context-window')?.value) || 32768,
                enable_streaming: document.getElementById('s-streaming')?.checked ?? false,
                summary_trigger_ratio: parseFloat(document.getElementById('s-summary-trigger')?.value) || 0.75,
                keep_recent_turns: parseInt(document.getElementById('s-keep-turns')?.value) || 4,
                idle_rotate_threshold: parseInt(document.getElementById('s-idle-rotate')?.value) || 24,
                enable_thinking: thinkingVal ? thinkingVal === 'true' : null,
                fallback_models: fallbackText ? fallbackText.split('\n').map(s => s.trim()).filter(Boolean) : [],
            },
            reflector: null,
            reflector_inherits_actor: true,
        };

        try {
            await post(API.CONFIG_LLM, config);
            showSuccess('LLM 配置已保存');
        } catch (err) {
            showError('保存失败: ' + err.message);
        } finally {
            btn.disabled = false;
            btn.textContent = '保存配置';
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
