// 管理页逻辑

let dialog, confirmBtn, cancelBtn, rebirthBtn;

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
    const res = await fetch('/api/v1/config/llm/providers');
    const data = await res.json();

    const actorSelect = document.getElementById('actor-provider');
    const reflectorSelect = document.getElementById('reflector-provider');
    const baseUrlGroup = document.getElementById('actor-base-url-group');
    const apiKeyGroup = document.getElementById('actor-api-key-group');

    actorSelect.innerHTML = '';
    reflectorSelect.innerHTML = '';

    data.providers.forEach(provider => {
        actorSelect.add(new Option(provider.label, provider.value));
        reflectorSelect.add(new Option(provider.label, provider.value));
    });

    actorSelect.addEventListener('change', (e) => {
        const selected = data.providers.find(p => p.value === e.target.value);
        baseUrlGroup.style.display = selected.requires_base_url ? 'block' : 'none';
        apiKeyGroup.style.display = selected.requires_base_url ? 'block' : 'none';
    });
}

async function loadLlmConfig() {
    const res = await fetch('/api/v1/config/llm');
    const data = await res.json();

    const section = document.getElementById('llm-config-section');

    if (data.runtime_mode === 'claw') {
        section.classList.add('claw-mode-disabled');
        section.querySelector('.section-desc').textContent =
            'LLM 配置仅在 Cognitive 模式下生效。当前模式：Claw';
        disableAllInputs(section);
        return;
    }

    setFormValue('actor-provider', data.actor.provider);
    setFormValue('actor-model', data.actor.model);
    setFormValue('actor-base-url', data.actor.base_url || '');

    const apiKeyInput = document.getElementById('actor-api-key');
    apiKeyInput.placeholder = data.actor.has_api_key ? '已配置（留空不修改）' : '未配置';

    document.getElementById('reflector-inherit').checked = data.reflector_inherits_actor;
    document.getElementById('reflector-inherit').dispatchEvent(new Event('change'));

    document.getElementById('actor-provider').dispatchEvent(new Event('change'));
}

document.addEventListener('DOMContentLoaded', () => {
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
        showSavingState();

        try {
            const payload = {
                actor: getActorConfig(),
                reflector: document.getElementById('reflector-inherit').checked ? null : getReflectorConfig(),
                reflector_inherits_actor: document.getElementById('reflector-inherit').checked
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
    document.getElementById('reflector-inherit').addEventListener('change', (e) => {
        const form = document.getElementById('reflector-llm-form');
        if (e.target.checked) {
            form.classList.add('disabled-form');
            form.querySelectorAll('input, select').forEach(el => el.disabled = true);
        } else {
            form.classList.remove('disabled-form');
            form.querySelectorAll('input, select').forEach(el => el.disabled = false);
        }
    });

    // 加载 LLM 配置
    loadProviders();
    loadLlmConfig();
});
