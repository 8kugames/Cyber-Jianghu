// 管理页逻辑

let providersData = null;
let tokenUsageTimer = null;

function formatNumber(n) {
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
    if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
    return n.toString();
}

async function loadTokenUsage() {
    try {
        const res = await fetch('/api/v1/config/llm/usage');
        if (!res.ok) return;
        const data = await res.json();

        const setText = (id, val) => {
            const el = document.getElementById(id);
            if (el) el.textContent = val;
        };

        setText('stat-total-calls', formatNumber(data.total_calls));
        setText('stat-prompt-tokens', formatNumber(data.total_prompt_tokens));
        setText('stat-completion-tokens', formatNumber(data.total_completion_tokens));
        setText('stat-total-tokens', formatNumber(data.total_tokens));
    } catch (err) {
        console.error('加载 Token 使用统计失败:', err);
    }
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

    // Token 使用统计：立即加载 + 每分钟刷新
    await loadTokenUsage();
    tokenUsageTimer = setInterval(loadTokenUsage, 60_000);
});
