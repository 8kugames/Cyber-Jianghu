// 管理页逻辑

let dialog, confirmBtn, cancelBtn, rebirthBtn;
let providersData = null;

function showDialog() {
    if (dialog) dialog.style.display = 'flex';
}

function hideDialog() {
    if (dialog) dialog.style.display = 'none';
}

async function loadServerConfig() {
    try {
        const data = await apiGet('/api/v1/config');
        if (data) {
            document.getElementById('server-ws-url').value = data.server_ws_url || '';
            document.getElementById('server-http-url').value = data.server_http_url || '';
        }
    } catch (err) {
        console.error('加载服务器配置失败:', err);
    }
}

async function loadDreamStatus() {
    try {
        const charResp = await apiGet('/api/v1/character');
        const targetEl = document.getElementById('dream-target-character');
        if (charResp.agent_id && charResp.status === 'alive') {
            targetEl.textContent = charResp.name || '当前角色';
        } else {
            targetEl.textContent = '无活跃角色';
        }

        const data = await apiGet('/api/v1/character/dream');
        const statusEl = document.getElementById('dream-status');
        if (data.thought && data.remaining_ticks > 0) {
            document.getElementById('current-dream').textContent = data.thought;
            document.getElementById('remaining-ticks').textContent = data.remaining_ticks;
            show(statusEl);
        } else {
            hide(statusEl);
        }
    } catch (err) {
        console.error('加载托梦状态失败:', err);
        document.getElementById('dream-target-character').textContent = '加载失败';
    }
}

async function executeRebirth() {
    const resultEl = document.getElementById('rebirth-result');
    const errorEl = document.getElementById('rebirth-error');

    hideDialog();
    hide(resultEl);
    hide(errorEl);

    rebirthBtn.disabled = true;
    rebirthBtn.textContent = '转生中...';

    try {
        const data = await apiPost('/api/v1/character/rebirth', { confirm: true });
        if (data.success) {
            document.getElementById('rebirth-message').textContent = data.message;
            show(resultEl);
            rebirthBtn.textContent = '已转生';
        } else {
            document.getElementById('rebirth-error-msg').textContent = data.message || '服务器错误';
            show(errorEl);
            rebirthBtn.disabled = false;
            rebirthBtn.textContent = '确认转生';
        }
    } catch (err) {
        document.getElementById('rebirth-error-msg').textContent = `网络错误: ${err.message}`;
        show(errorEl);
        rebirthBtn.disabled = false;
        rebirthBtn.textContent = '确认转生';
    }
}

// LLM 配置辅助函数
function setFormValue(id, value) {
    const el = document.getElementById(id);
    if (el) el.value = value || '';
}

function getActorConfig() {
    return {
        provider: document.getElementById('actor-provider').value,
        model: document.getElementById('actor-model').value,
        base_url: document.getElementById('actor-base-url').value || null,
        api_key: document.getElementById('actor-api-key').value || null
    };
}

function getReflectorConfig() {
    return {
        provider: document.getElementById('reflector-provider').value,
        model: document.getElementById('reflector-model').value,
        base_url: document.getElementById('reflector-base-url').value || null,
        api_key: document.getElementById('reflector-api-key').value || null
    };
}

function disableAllInputs(section) {
    section.querySelectorAll('input, select, button').forEach(el => el.disabled = true);
}

function showSavingState() {
    const btn = document.getElementById('save-llm-btn');
    btn.disabled = true;
    btn.textContent = '保存中...';
}

function resetSaveButton() {
    const btn = document.getElementById('save-llm-btn');
    btn.disabled = false;
    btn.textContent = '保存配置';
}

async function loadProviders() {
    try {
        const res = await fetch('/api/v1/config/llm/providers');
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();

        const actorSelect = document.getElementById('actor-provider');
        const reflectorSelect = document.getElementById('reflector-provider');
        const baseUrlGroup = document.getElementById('actor-base-url-group');
        const apiKeyGroup = document.getElementById('actor-api-key-group');

        if (!actorSelect || !reflectorSelect || !baseUrlGroup || !apiKeyGroup) {
            console.error('LLM 配置 DOM 元素未找到');
            return;
        }

        actorSelect.innerHTML = '';
        reflectorSelect.innerHTML = '';

        data.providers.forEach(provider => {
            actorSelect.add(new Option(provider.label, provider.value));
            reflectorSelect.add(new Option(provider.label, provider.value));
        });

        // Store provider data globally for event handlers
        providersData = data;
    } catch (err) {
        console.error('加载 Provider 列表失败:', err);
        showError('加载 Provider 列表失败: ' + err.message);
    }
}

function handleProviderChange(targetSelect, baseUrlGroup, apiKeyGroup) {
    if (!providersData) return;

    const selected = providersData.providers.find(p => p.value === targetSelect.value);
    if (selected) {
        baseUrlGroup.style.display = selected.requires_base_url ? 'block' : 'none';
        apiKeyGroup.style.display = selected.requires_base_url ? 'block' : 'none';
    }
}

async function loadLlmConfig() {
    try {
        const res = await fetch('/api/v1/config/llm');
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();

        const section = document.getElementById('llm-config-section');
        if (!section) {
            console.error('LLM 配置区域未找到');
            return;
        }

        if (data.runtime_mode === 'claw') {
            section.classList.add('claw-mode-disabled');
            const sectionDesc = section.querySelector('.section-desc');
            if (sectionDesc) {
                sectionDesc.textContent =
                    'LLM 配置仅在 Cognitive 模式下生效。当前模式：Claw';
            }
            disableAllInputs(section);
            return;
        }

        setFormValue('actor-provider', data.actor.provider);
        setFormValue('actor-model', data.actor.model);
        setFormValue('actor-base-url', data.actor.base_url || '');
        setFormValue('reflector-provider', data.reflector?.provider || '');
        setFormValue('reflector-model', data.reflector?.model || '');
        setFormValue('reflector-base-url', data.reflector?.base_url || '');

        const apiKeyInput = document.getElementById('actor-api-key');
        if (apiKeyInput) {
            apiKeyInput.placeholder = data.actor.has_api_key ? '已配置（留空不修改）' : '未配置';
        }

        const reflectorInherit = document.getElementById('reflector-inherit');
        if (reflectorInherit) {
            reflectorInherit.checked = data.reflector_inherits_actor;
            reflectorInherit.dispatchEvent(new Event('change'));
        }

        // Trigger actor provider change to show/hide fields
        const actorProvider = document.getElementById('actor-provider');
        if (actorProvider) {
            actorProvider.dispatchEvent(new Event('change'));
        }
    } catch (err) {
        console.error('加载 LLM 配置失败:', err);
        showError('加载 LLM 配置失败: ' + err.message);
    }
}

document.addEventListener('DOMContentLoaded', async () => {
    dialog = document.getElementById('confirm-dialog');
    confirmBtn = document.getElementById('confirm-ok');
    cancelBtn = document.getElementById('confirm-cancel');
    rebirthBtn = document.getElementById('rebirth-btn');

    if (dialog) dialog.style.display = 'none';

    if (rebirthBtn) rebirthBtn.addEventListener('click', showDialog);
    if (cancelBtn) cancelBtn.addEventListener('click', hideDialog);
    if (confirmBtn) confirmBtn.addEventListener('click', executeRebirth);
    if (dialog) dialog.addEventListener('click', (e) => { if (e.target === dialog) hideDialog(); });

    loadDreamStatus();
    loadServerConfig();

    // 服务器配置表单
    document.getElementById('server-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const btn = document.getElementById('save-server-btn');
        const resultEl = document.getElementById('server-result');
        const errorEl = document.getElementById('server-error');

        hide(resultEl);
        hide(errorEl);
        btn.disabled = true;
        btn.textContent = '保存中...';

        const ws_url = document.getElementById('server-ws-url').value.trim();
        const http_url = document.getElementById('server-http-url').value.trim();
        const body = { ws_url };
        if (http_url) body.http_url = http_url;

        try {
            const data = await apiPost('/api/v1/config/server', body);
            if (data.success) {
                showSuccess(data.message);
                show(resultEl);
            } else {
                showError(data.message || '服务器错误');
                show(errorEl);
            }
        } catch (err) {
            showError(`网络错误: ${err.message}`);
            show(errorEl);
        } finally {
            btn.disabled = false;
            btn.textContent = '保存并重连';
        }
    });

    // 托梦表单
    document.getElementById('dream-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const btn = document.getElementById('dream-btn');
        const resultEl = document.getElementById('dream-result');
        const errorEl = document.getElementById('dream-error');

        hide(resultEl);
        hide(errorEl);
        btn.disabled = true;
        btn.textContent = '注入中...';

        const thought = document.getElementById('dream-thought').value.trim();
        const duration = parseInt(document.getElementById('dream-duration').value) || 5;

        try {
            const data = await apiPost('/api/v1/character/dream', { thought, duration });
            showSuccess(data.message);
            show(resultEl);
            document.getElementById('dream-thought').value = '';
            loadDreamStatus();
        } catch (err) {
            showError(err.message);
            show(errorEl);
        } finally {
            btn.disabled = false;
            btn.textContent = '注入托梦';
        }
    });

    // LLM 配置保存按钮
    document.getElementById('save-llm-btn').addEventListener('click', async () => {
        // Client-side validation
        const actorConfig = getActorConfig();
        if (!actorConfig.provider || !actorConfig.model) {
            showError('请填写 ActorSoul 的 Provider 和 Model');
            return;
        }

        const reflectorInherit = document.getElementById('reflector-inherit');
        if (!reflectorInherit || !reflectorInherit.checked) {
            const reflectorConfig = getReflectorConfig();
            if (!reflectorConfig.provider || !reflectorConfig.model) {
                showError('请填写 ReflectorSoul 的 Provider 和 Model');
                return;
            }
        }

        showSavingState();

        try {
            const payload = {
                actor: actorConfig,
                reflector: reflectorInherit.checked ? null : getReflectorConfig(),
                reflector_inherits_actor: reflectorInherit.checked
            };

            const res = await fetch('/api/v1/config/llm', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(payload)
            });

            if (res.ok) {
                showSuccess('配置已保存，将在检测到文件变更时生效');
                await loadLlmConfig();
            } else {
                const err = await res.json();
                showError(err.message || '保存失败');
            }
        } catch (e) {
            showError('网络错误: ' + e.message);
        } finally {
            resetSaveButton();
        }
    });

    // ReflectorSoul 继承 checkbox
    const reflectorInherit = document.getElementById('reflector-inherit');
    if (reflectorInherit) {
        reflectorInherit.addEventListener('change', (e) => {
            const form = document.getElementById('reflector-llm-form');
            if (!form) return;

            if (e.target.checked) {
                form.classList.add('disabled-form');
                form.querySelectorAll('input, select').forEach(el => el.disabled = true);
            } else {
                form.classList.remove('disabled-form');
                form.querySelectorAll('input, select').forEach(el => el.disabled = false);
            }
        });
    }

    // ActorSoul provider change listener
    const actorProvider = document.getElementById('actor-provider');
    const actorBaseUrlGroup = document.getElementById('actor-base-url-group');
    const actorApiKeyGroup = document.getElementById('actor-api-key-group');

    if (actorProvider && actorBaseUrlGroup && actorApiKeyGroup) {
        actorProvider.addEventListener('change', (e) => {
            handleProviderChange(e.target, actorBaseUrlGroup, actorApiKeyGroup);
        });
    }

    // ReflectorSoul provider change listener
    const reflectorProvider = document.getElementById('reflector-provider');
    const reflectorBaseUrlGroup = document.getElementById('reflector-base-url-group');
    const reflectorApiKeyGroup = document.getElementById('reflector-api-key-group');

    if (reflectorProvider && reflectorBaseUrlGroup && reflectorApiKeyGroup) {
        reflectorProvider.addEventListener('change', (e) => {
            handleProviderChange(e.target, reflectorBaseUrlGroup, reflectorApiKeyGroup);
        });
    }

    // 加载 LLM 配置（串行执行避免竞态条件）
    await loadProviders();
    await loadLlmConfig();
});
