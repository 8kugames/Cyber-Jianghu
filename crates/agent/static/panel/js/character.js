// 角色信息页逻辑

let currentPage = 1;
let memoryPage = 1;
const PAGE_LIMIT = 20;
let hasMore = false;
let hasMoreMemories = false;
let allCharacters = [];

// 加载所有角色列表
async function loadCharacterList() {
    try {
        const data = await apiGet('/api/v1/characters');
        allCharacters = data.characters || [];
        const selectEl = document.getElementById('character-select');
        const serverEl = document.getElementById('current-server');
        const selectorSection = document.getElementById('character-selector-section');

        serverEl.textContent = data.current_server_url || '-';

        const aliveCharacters = allCharacters.filter(c => c.status === 'alive');
        if (aliveCharacters.length <= 1) {
            selectorSection.classList.add('hidden');
            return;
        }

        selectEl.innerHTML = allCharacters.map(c => {
            const statusText = c.status === 'alive' ? '' :
                              c.status === 'dead' ? ' [已故]' : ' [归隐]';
            const serverInfo = c.server_url
                ? ` (${c.server_url.replace(/^https?:\/\//, '').split('/')[0]})`
                : '';
            const selected = c.is_current ? 'selected' : '';
            const disabled = c.status !== 'alive' ? 'disabled' : '';
            return `<option value="${c.agent_id || ''}" ${selected} ${disabled}>${escapeHtml(c.name)}${statusText}${serverInfo}</option>`;
        }).join('');

        selectorSection.classList.remove('hidden');
    } catch (err) {
        console.error('加载角色列表失败:', err);
    }
}

// 切换角色
async function switchCharacter() {
    const selectEl = document.getElementById('character-select');
    const agentId = selectEl.value;
    if (!agentId) return;

    const currentChar = allCharacters.find(c => c.agent_id === agentId);
    if (currentChar && currentChar.is_current) return;

    try {
        const data = await apiPost('/api/v1/characters/switch', { agent_id: agentId });
        if (data.success) {
            loadCharacter();
        } else {
            showError(data.message || '切换角色失败');
            const currentChar = allCharacters.find(c => c.is_current);
            if (currentChar) selectEl.value = currentChar.agent_id;
        }
    } catch (err) {
        showError('切换角色失败: ' + err.message);
    }
}

// 加载角色信息
async function loadCharacter() {
    hide('.error');
    hide('#character-info');
    hide('#experiences-section');
    show('#loading');

    try {
        const data = await apiGet('/api/v1/character');

        // 基本信息
        document.getElementById('name').textContent = data.name || '-';
        document.getElementById('age').textContent = data.age || '-';
        document.getElementById('gender').textContent = data.gender || '-';
        document.getElementById('identity').textContent = data.identity || '-';
        document.getElementById('appearance').textContent = data.appearance || '-';
        document.getElementById('location').textContent = data.location || '-';
        document.getElementById('tick-id').textContent = data.tick_id || '-';

        // 状态
        if (data.status) {
            const statusEl = document.getElementById('status');
            statusEl.textContent = data.status === 'alive' ? '存活' :
                                   data.status === 'dead' ? '死亡' : data.status;
            statusEl.className = `value status-${data.status}`;
        }

        // 注册时间
        if (data.registered_at) {
            document.getElementById('registered-at').textContent = formatDateTime(data.registered_at);
        }

        // 游戏时间
        if (data.world_time) {
            document.getElementById('world-time').textContent = formatWorldTime(data.world_time);
        }

        // 性格标签
        const personalityEl = document.getElementById('personality');
        personalityEl.innerHTML = data.personality && data.personality.length > 0
            ? data.personality.map(p => `<span class="info-tag">${escapeHtml(p)}</span>`).join('')
            : '-';

        // 价值观标签
        const valuesEl = document.getElementById('values');
        valuesEl.innerHTML = data.values && data.values.length > 0
            ? data.values.map(v => `<span class="info-tag">${escapeHtml(v)}</span>`).join('')
            : '-';

        // 属性
        renderAttributes(data.attributes);

        // 物品（修复 XSS）
        renderInventory(data.inventory);

        hide('#loading');
        show('#character-info');
        show('#experiences-section');
        loadExperiences();

    } catch (err) {
        hide('#loading');
        document.getElementById('error-message').textContent = err.message;
        show('.error');
    }
}

// 渲染属性
function renderAttributes(attributes) {
    const attrsEl = document.getElementById('attributes');
    if (!attributes) {
        attrsEl.innerHTML = '<p class="no-data">暂无属性数据</p>';
        return;
    }

    const statusAttrs = ['hp', 'stamina', 'hunger', 'thirst'];
    const innateAttrs = ['strength', 'agility', 'constitution', 'intelligence', 'charisma', 'luck'];

    const isRedundantMax = (key) => {
        if (typeof key !== 'string' || !key.endsWith('_max')) return false;
        const base = key.slice(0, -4);
        if (statusAttrs.includes(base) || innateAttrs.includes(base)) return true;
        return false;
    };

    let html = '';

    // 先天属性
    html += '<div class="attr-section"><h4>先天属性</h4><div class="attr-group">';
    innateAttrs.forEach(key => {
        const attr = attributes[key];
        if (attr && typeof attr === 'object' && attr.current !== undefined) {
            html += `<div class="attr-item" title="${escapeHtml(attr.description || '')}">
                <span class="attr-name">${escapeHtml(attr.name || key)}</span>
                <span class="attr-value">${attr.current}/${attr.max}</span>
            </div>`;
        }
    });
    html += '</div></div>';

    // 状态属性
    html += '<div class="attr-section"><h4>状态属性</h4><div class="attr-group">';
    statusAttrs.forEach(key => {
        const attr = attributes[key];
        if (attr && typeof attr === 'object' && attr.current !== undefined) {
            const pct = attr.max > 0 ? Math.round((attr.current / attr.max) * 100) : 0;
            const cls = pct > 70 ? 'attr-high' : pct > 30 ? 'attr-medium' : 'attr-low';
            html += `<div class="attr-item ${cls}" title="${escapeHtml(attr.description || '')}">
                <span class="attr-name">${escapeHtml(attr.name || key)}</span>
                <span class="attr-value">${attr.current}/${attr.max}</span>
            </div>`;
        }
    });
    html += '</div></div>';

    // 派生属性
    const known = [...statusAttrs, ...innateAttrs];
    const derived = Object.keys(attributes)
        .filter(k => !known.includes(k))
        .filter(k => !isRedundantMax(k))
        .filter(k => attributes[k] && typeof attributes[k] === 'object');

    if (derived.length > 0) {
        html += '<div class="attr-section"><h4>派生属性</h4><div class="attr-group">';
        derived.forEach(key => {
            const attr = attributes[key];
            if (attr && typeof attr === 'object' && attr.current !== undefined) {
                const pct = attr.max > 0 ? Math.round((attr.current / attr.max) * 100) : 0;
                const cls = pct > 70 ? 'attr-high' : pct > 30 ? 'attr-medium' : 'attr-low';
                html += `<div class="attr-item ${cls}" title="${escapeHtml(attr.description || '')}">
                    <span class="attr-name">${escapeHtml(attr.name || key)}</span>
                    <span class="attr-value">${attr.current}/${attr.max}</span>
                </div>`;
            }
        });
        html += '</div></div>';
    }

    attrsEl.innerHTML = html;
}

// 渲染物品（XSS 修复）
function renderInventory(inventory) {
    const invEl = document.getElementById('inventory');
    if (!inventory || inventory.length === 0) {
        invEl.innerHTML = '<p class="no-data">暂无物品</p>';
        return;
    }

    // 使用 textContent 避免 XSS
    invEl.innerHTML = '';
    inventory.forEach(item => {
        const div = document.createElement('div');
        div.className = 'inv-item';
        const nameSpan = document.createElement('span');
        nameSpan.className = 'inv-name';
        nameSpan.textContent = item.name || item.item_id;
        const qtySpan = document.createElement('span');
        qtySpan.className = 'inv-qty';
        qtySpan.textContent = `x${item.quantity || 1}`;
        div.appendChild(nameSpan);
        div.appendChild(qtySpan);
        invEl.appendChild(div);
    });
}

// 加载经历日志
async function loadExperiences(page = 1) {
    const expEl = document.getElementById('experiences');
    const loadMoreEl = document.getElementById('load-more');

    if (page === 1) {
        expEl.innerHTML = '<p class="loading-text">加载中...</p>';
    }

    try {
        const data = await apiGet(`/api/v1/character/experiences?page=${page}&limit=${PAGE_LIMIT}`);
        hasMore = data.has_more;
        currentPage = page;

        if (page === 1) expEl.innerHTML = '';

        if (data.experiences && data.experiences.length > 0) {
            data.experiences.forEach(exp => {
                const div = document.createElement('div');
                div.className = 'exp-item';

                let html = `
                    <div class="exp-header">
                        <span class="exp-tick">Tick ${exp.tick_id}</span>
                    </div>
                    <div class="exp-content">${escapeHtml(exp.event)}</div>
                `;
                if (exp.intent_summary) {
                    html += `<div class="exp-thought">
                        <span class="thought-label">意图:</span>
                        <span class="thought-content">${escapeHtml(exp.intent_summary)}</span>
                    </div>`;
                }
                if (exp.observer_thought) {
                    html += `<div class="exp-observer">
                        <span class="observer-label">审查:</span>
                        <span class="observer-content">${escapeHtml(exp.observer_thought)}</span>
                    </div>`;
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

function loadMoreExperiences() {
    loadExperiences(currentPage + 1);
}

// 加载关系列表
async function loadRelationships() {
    const relEl = document.getElementById('relationships');
    try {
        const data = await apiGet('/api/v1/relationship/list');
        if (data.relationships && data.relationships.length > 0) {
            relEl.innerHTML = data.relationships.map(rel => `
                <div class="rel-item">
                    <span class="rel-name">${escapeHtml(rel.target_name || rel.target_agent_id || '未知')}</span>
                    <span class="rel-favorability">好感度: ${rel.favorability ?? 0}</span>
                </div>
            `).join('');
        } else {
            relEl.innerHTML = '<p class="no-data">暂无关系记录</p>';
        }
    } catch (err) {
        relEl.innerHTML = '<p class="error-text">加载关系失败</p>';
    }
}

// 加载近期记忆
async function loadMemories(page = 1) {
    const memEl = document.getElementById('memories');
    const loadMoreEl = document.getElementById('load-more-memories');

    if (page === 1) {
        memEl.innerHTML = '<p class="loading-text">加载中...</p>';
    }

    try {
        const data = await apiGet(`/api/v1/memory/recent?page=${page}&limit=${PAGE_LIMIT}`);
        hasMoreMemories = data.has_more;
        memoryPage = page;

        if (page === 1) memEl.innerHTML = '';

        if (data.memories && data.memories.length > 0) {
            data.memories.forEach(mem => {
                const div = document.createElement('div');
                div.className = 'mem-item';
                const tickSpan = document.createElement('span');
                tickSpan.className = 'mem-tick';
                tickSpan.textContent = `Tick ${mem.tick_id || '-'}`;
                const contentDiv = document.createElement('div');
                contentDiv.className = 'mem-content';
                contentDiv.textContent = mem.content || '';
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
    loadMemories(memoryPage + 1);
}

// 页面加载
document.addEventListener('DOMContentLoaded', () => {
    loadCharacterList();
    loadCharacter();

    document.getElementById('load-more-experiences-btn').addEventListener('click', loadMoreExperiences);
    document.getElementById('load-more-memories-btn').addEventListener('click', loadMoreMemories);
    document.getElementById('character-select').addEventListener('change', switchCharacter);

    loadRelationships();
    loadMemories();
});
