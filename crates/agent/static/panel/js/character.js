// Character page: sidebar tabs, panel switching, character dropdown, creation, death/rebirth

import { API, get, post } from './api.js';
import { escapeHtml, showLoading, showSuccess, showError, showModal, hideModal } from './ui.js';
import { onEvent } from './app.js';
import { getPanelDefinitions, mountPanel } from './panels.js';

let activePanel = 'attributes';
let characterData = null;
let unsubscribe = null;

export const characterPage = {
    mount(container) {
        showLoading(container);
        render(container);
        loadCharacterList();
        loadCharacterData();
        unsubscribe = onEvent((event) => {
            if (event.type === 'death' || event.type === 'agent_died') handleDeath(event);
        });
    },
    unmount() {
        if (unsubscribe) { unsubscribe(); unsubscribe = null; }
    },
};

function render(container) {
    container.innerHTML = `
    <div class="character-page">
        <div class="character-header" id="char-header">
            <div style="display:flex;align-items:center;gap:10px" id="char-info">
                <p class="text-muted">加载中...</p>
            </div>
            <div style="display:flex;align-items:center;gap:8px">
                <select class="form-select" id="char-select" style="width:180px"></select>
                <button class="btn btn-sm" id="char-create-btn">+ 新角色</button>
            </div>
        </div>
        <div class="character-body">
            <div class="character-sidebar" id="char-sidebar"></div>
            <div class="character-content" id="char-content">
                <div class="loading"><div class="spinner"></div><p>加载面板...</p></div>
            </div>
        </div>
    </div>`;

    // Build sidebar tabs
    const sidebar = document.getElementById('char-sidebar');
    if (sidebar) {
        const panels = getPanelDefinitions();
        sidebar.innerHTML = panels.map(p =>
            `<div class="sidebar-tab${p.id === activePanel ? ' active' : ''}" data-panel="${p.id}">${escapeHtml(p.label)}</div>`
        ).join('');

        sidebar.addEventListener('click', (e) => {
            const tab = e.target.closest('.sidebar-tab');
            if (!tab) return;
            const panelId = tab.dataset.panel;
            if (panelId && panelId !== activePanel) {
                activePanel = panelId;
                sidebar.querySelectorAll('.sidebar-tab').forEach(t => t.classList.toggle('active', t.dataset.panel === panelId));
                loadPanel();
            }
        });
    }

    // Character select
    document.getElementById('char-select')?.addEventListener('change', async (e) => {
        const agentId = e.target.value;
        if (agentId === '__create__') {
            showCreationModal();
            e.target.value = characterData?.agent_id || '';
            return;
        }
        if (agentId) {
            try {
                await post(API.CHARACTERS_SWITCH, { agent_id: agentId });
                loadCharacterData();
            } catch (err) {
                showError('切换失败: ' + err.message);
            }
        }
    });

    // Create button
    document.getElementById('char-create-btn')?.addEventListener('click', showCreationModal);
}

async function loadCharacterList() {
    try {
        const data = await get(API.CHARACTERS);
        const characters = data.characters || data || [];
        const select = document.getElementById('char-select');
        if (!select) return;

        let html = '';
        for (const ch of characters) {
            const id = ch.agent_id || ch.id;
            const name = ch.name || id;
            const status = ch.status || 'unknown';
            const disabled = status === 'dead' || status === 'retired';
            html += `<option value="${escapeHtml(id)}" ${disabled ? 'disabled' : ''}>${escapeHtml(name)} (${status})</option>`;
        }
        html += '<option value="__create__">+ 新角色</option>';
        select.innerHTML = html;

        if (characterData?.agent_id) {
            select.value = characterData.agent_id;
        }
    } catch (_) {}
}

async function loadCharacterData() {
    const infoEl = document.getElementById('char-info');
    try {
        const data = await get(API.CHARACTER);
        characterData = data;

        const name = data.name || data.agent_name || '-';
        const age = data.age ?? '-';
        const gender = data.gender || '-';
        const location = data.location_name || data.location_id || '-';
        const status = data.status || 'unknown';
        const statusMap = { alive: '活跃', dead: '死亡', retired: '退休' };

        if (infoEl) {
            infoEl.innerHTML = `
                <div style="width:36px;height:36px;background:var(--accent-light);border-radius:50%;display:flex;align-items:center;justify-content:center;font-size:16px;font-weight:600">${escapeHtml(name.charAt(0))}</div>
                <div>
                    <div style="font-size:16px;font-weight:600">${escapeHtml(name)}</div>
                    <div style="font-size:12px;color:var(--text-muted)">${escapeHtml(gender)} · ${age}岁 · ${escapeHtml(location)} · <span style="color:${status === 'alive' ? 'var(--success)' : 'var(--danger)'}">${statusMap[status] || status}</span></div>
                </div>
            `;
        }

        // Update select
        const select = document.getElementById('char-select');
        if (select && data.agent_id) select.value = data.agent_id;

        loadPanel();
    } catch (e) {
        if (infoEl) infoEl.innerHTML = '<p class="text-muted">无角色数据</p>';
        if (e.message.includes('412') || e.message.includes('没有')) {
            if (infoEl) infoEl.innerHTML = '<p class="text-muted">当前无活跃角色，请创建新角色</p>';
        }
    }
}

async function loadPanel() {
    const container = document.getElementById('char-content');
    if (!container) return;
    showLoading(container);

    const ctx = {
        agentId: characterData?.agent_id,
        character: characterData,
    };

    await mountPanel(activePanel, container, ctx);
}

// ============================================================================
// Character Creation Modal
// ============================================================================

function showCreationModal() {
    showModal(`
        <h3 style="margin-bottom:12px">创建新角色</h3>
        <div style="display:flex;gap:8px;margin-bottom:16px">
            <button class="btn btn-primary" id="create-generate-btn">一键生成 (LLM)</button>
            <button class="btn" id="create-manual-btn">手动创建</button>
        </div>
        <div id="create-content"></div>
    `);

    document.getElementById('create-generate-btn')?.addEventListener('click', startGeneration);
    document.getElementById('create-manual-btn')?.addEventListener('click', showManualForm);
}

async function startGeneration() {
    const content = document.getElementById('create-content');
    if (!content) return;
    content.innerHTML = '<div class="loading"><div class="spinner"></div><p>正在生成角色...</p></div>';

    try {
        const data = await post(API.CHARACTER_GENERATE, {});
        content.innerHTML = `
            <div class="card" style="padding:16px">
                <div style="font-size:16px;font-weight:600;margin-bottom:8px">${escapeHtml(data.name || '-')}</div>
                <div style="font-size:13px;color:var(--text-muted);margin-bottom:4px">${escapeHtml(data.gender || '-')} · ${data.age ?? '-'}岁</div>
                <div style="font-size:13px;margin-top:8px">${escapeHtml(data.background || data.personality || '')}</div>
            </div>
            <div style="margin-top:12px;display:flex;gap:8px">
                <button class="btn btn-primary" id="create-confirm-btn">确认注册</button>
                <button class="btn" id="create-regen-btn">重新生成</button>
            </div>
        `;

        const charData = data;
        document.getElementById('create-confirm-btn')?.addEventListener('click', async () => {
            try {
                await post(API.CHARACTER_REGISTER, charData);
                showSuccess('角色创建成功');
                hideModal();
                loadCharacterData();
                loadCharacterList();
            } catch (e) {
                showError('注册失败: ' + e.message);
            }
        });
        document.getElementById('create-regen-btn')?.addEventListener('click', startGeneration);
    } catch (e) {
        content.innerHTML = `<p style="color:var(--danger)">生成失败: ${escapeHtml(e.message)}</p><button class="btn" id="create-retry-btn" style="margin-top:8px">重试</button>`;
        document.getElementById('create-retry-btn')?.addEventListener('click', startGeneration);
    }
}

function showManualForm() {
    const content = document.getElementById('create-content');
    if (!content) return;
    content.innerHTML = `
        <form id="manual-create-form">
            <div class="form-group"><label class="form-label">姓名</label><input class="form-input" id="mc-name" required></div>
            <div class="form-group"><label class="form-label">性别</label><select class="form-select" id="mc-gender"><option value="男">男</option><option value="女">女</option></select></div>
            <div class="form-group"><label class="form-label">年龄</label><input class="form-input" type="number" id="mc-age" value="20" min="10" max="80"></div>
            <div class="form-group"><label class="form-label">背景</label><textarea class="form-input" id="mc-background" rows="3"></textarea></div>
            <button type="submit" class="btn btn-primary">创建</button>
        </form>
    `;

    document.getElementById('manual-create-form')?.addEventListener('submit', async (e) => {
        e.preventDefault();
        const charData = {
            name: document.getElementById('mc-name')?.value?.trim(),
            gender: document.getElementById('mc-gender')?.value,
            age: parseInt(document.getElementById('mc-age')?.value) || 20,
            background: document.getElementById('mc-background')?.value?.trim(),
        };
        try {
            await post(API.CHARACTER_REGISTER, charData);
            showSuccess('角色创建成功');
            hideModal();
            loadCharacterData();
            loadCharacterList();
        } catch (e) {
            showError('创建失败: ' + e.message);
        }
    });
}

// ============================================================================
// Death & Rebirth
// ============================================================================

function handleDeath(event) {
    showModal(`
        <div style="text-align:center;padding:24px">
            <h2 style="color:var(--danger);margin-bottom:12px">角色已死亡</h2>
            <p style="color:var(--text-secondary);margin-bottom:20px">${escapeHtml(event.name || event.agent_id || '')} 已离开这个世界</p>
            <button class="btn btn-primary" id="rebirth-btn">转世重生</button>
            <button class="btn" id="close-death-btn" style="margin-left:8px">关闭</button>
        </div>
    `);

    document.getElementById('rebirth-btn')?.addEventListener('click', async () => {
        const btn = document.getElementById('rebirth-btn');
        btn.disabled = true;
        btn.textContent = '重生中...';
        try {
            await post(API.CHARACTER_REBIRTH, {});
            showSuccess('角色已重生');
            hideModal();
            loadCharacterData();
            loadCharacterList();
        } catch (e) {
            showError('重生失败: ' + e.message);
            btn.disabled = false;
            btn.textContent = '转世重生';
        }
    });

    document.getElementById('close-death-btn')?.addEventListener('click', hideModal);
}
