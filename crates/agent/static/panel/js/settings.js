// Settings page: server + LLM config, setup wizard mode

import { API, get, post } from './api.js';
import { showSuccess, showError, fmtNum } from './ui.js';
import { appState } from './app.js';

export const settingsPage = {
    mount(container) {
        render(container);
        loadData();
    },
    unmount() {},
};

async function loadData() {
    const isWizard = !appState.setupStatus?.server_configured || !appState.setupStatus?.llm_configured;
    const [llmConfig, providers, usage, llmDisabled] = await Promise.allSettled([
        get(API.CONFIG_LLM),
        get(API.CONFIG_LLM_PROVIDERS),
        get(API.CONFIG_LLM_USAGE),
        get(API.CONFIG_LLM_DISABLED),
    ]);

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
        const fields = { 's-provider': c.provider, 's-model': c.model, 's-base-url': c.base_url, 's-api-key': c.api_key, 's-temperature': c.temperature, 's-max-tokens': c.max_tokens, 's-context-window': c.context_window };
        for (const [id, val] of Object.entries(fields)) {
            const el = document.getElementById(id);
            if (el && val != null) el.value = val;
        }
        const streamEl = document.getElementById('s-streaming');
        if (streamEl) streamEl.checked = c.streaming !== false;

        // Advanced
        const advFields = { 's-summary-trigger': c.summary_trigger_ratio, 's-keep-turns': c.summary_keep_turns, 's-idle-rotate': c.idle_rotate_threshold };
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

        // LLM disabled toggle
        const toggle = document.getElementById('s-llm-disabled');
        if (toggle && resp.llm_disabled) toggle.checked = true;
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
        if (dot) dot.className = `connection-dot ${appState.setupStatus.server_connected ? 'connected' : 'disconnected'}`;
        if (text) text.textContent = appState.setupStatus.server_connected ? '已连接' : '未连接';
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
                            <label class="form-label">API Key</label>
                            <input class="form-input" type="password" id="s-api-key" placeholder="输入 API Key">
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
                                <input class="form-input" type="number" id="s-context-window" min="4096" max="1048576" step="1024" value="32000">
                            </div>
                        </div>
                        <div class="form-group">
                            <label style="display:flex;align-items:center;gap:8px;cursor:pointer">
                                <input type="checkbox" id="s-streaming" checked> 启用流式输出
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
            await post(API.CONFIG_LLM_DISABLED, { disabled: this.checked });
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
            provider: document.getElementById('s-provider')?.value,
            model: document.getElementById('s-model')?.value?.trim(),
            base_url: document.getElementById('s-base-url')?.value?.trim(),
            api_key: document.getElementById('s-api-key')?.value?.trim(),
            temperature: parseFloat(document.getElementById('s-temperature')?.value) || 0.7,
            max_tokens: parseInt(document.getElementById('s-max-tokens')?.value) || 8192,
            context_window: parseInt(document.getElementById('s-context-window')?.value) || 32000,
            streaming: document.getElementById('s-streaming')?.checked ?? true,
            summary_trigger_ratio: parseFloat(document.getElementById('s-summary-trigger')?.value) || 0.75,
            summary_keep_turns: parseInt(document.getElementById('s-keep-turns')?.value) || 4,
            idle_rotate_threshold: parseInt(document.getElementById('s-idle-rotate')?.value) || 24,
            enable_thinking: thinkingVal ? thinkingVal === 'true' : null,
            fallback_models: fallbackText ? fallbackText.split('\n').map(s => s.trim()).filter(Boolean) : [],
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
}

function setText(id, text) {
    const el = document.getElementById(id);
    if (el) el.textContent = text;
}
