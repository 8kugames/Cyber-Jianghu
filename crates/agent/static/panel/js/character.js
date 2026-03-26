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

// 加载属性元数据（分类信息，从 narrative_config 解析）
async function loadAttributeMeta() {
    try {
        attributeMeta = await apiGet('/api/v1/attribute-meta');
    } catch (err) {
        console.error('加载属性元数据失败:', err);
    }
}

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
        } else {
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
        }

        renderWorldTree();
    } catch (err) {
        console.error('加载角色列表失败:', err);
    }
}

// 渲染世界树
function renderWorldTree() {
    const listEl = document.getElementById('world-tree-list');
    if (!allCharacters || allCharacters.length === 0) {
        listEl.innerHTML = '<p class="no-data">暂无角色记录</p>';
        return;
    }

    listEl.innerHTML = allCharacters.map(c => {
        const statusClass = c.status === 'alive' ? 'alive' : c.status === 'dead' ? 'dead' : 'retired';
        const statusText = c.status === 'alive' ? '存活' : c.status === 'dead' ? '已故' : '归隐';
        const currentLabel = c.is_current ? '<span class="current-label">当前</span>' : '';
        const registeredAt = c.registered_at ? new Date(c.registered_at).toLocaleDateString('zh-CN') : '';
        return `
            <div class="world-tree-card ${c.is_current ? 'current' : ''}" data-agent-id="${c.agent_id || ''}">
                <div class="char-name">
                    ${escapeHtml(c.name || '未知')}
                    ${currentLabel}
                </div>
                <div class="char-status ${statusClass}">${statusText}</div>
                <div class="char-meta">${registeredAt}</div>
            </div>
        `;
    }).join('');

    listEl.querySelectorAll('.world-tree-card').forEach(card => {
        card.addEventListener('click', () => {
            const agentId = card.dataset.agentId;
            const char = allCharacters.find(c => c.agent_id === agentId);
            if (!char) return;
            if (char.is_current) {
                document.querySelectorAll('.page-tab').forEach(t => t.classList.remove('active'));
                document.querySelector('[data-tab="current"]').classList.add('active');
                document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
                document.getElementById('tab-current').classList.add('active');
            } else {
                showCharacterDetail(char);
            }
        });
    });
}

// 显示角色详情对话框
function showCharacterDetail(char) {
    const dialog = document.getElementById('character-detail-dialog');
    const content = document.getElementById('character-detail-content');
    const statusClass = char.status === 'alive' ? 'alive' : char.status === 'dead' ? 'dead' : 'retired';
    const statusText = char.status === 'alive' ? '存活' : char.status === 'dead' ? '已故' : '归隐';
    const registeredAt = char.registered_at ? new Date(char.registered_at).toLocaleString('zh-CN') : '未知';

    content.innerHTML = `
        <div class="info-grid">
            <div class="info-item">
                <span class="label">姓名</span>
                <span class="value">${escapeHtml(char.name || '未知')}</span>
            </div>
            <div class="info-item">
                <span class="label">状态</span>
                <span class="value status-${statusClass}">${statusText}</span>
            </div>
            <div class="info-item">
                <span class="label">年龄</span>
                <span class="value">${char.age || '-'}</span>
            </div>
            <div class="info-item">
                <span class="label">性别</span>
                <span class="value">${escapeHtml(char.gender || '-')}</span>
            </div>
            <div class="info-item">
                <span class="label">身份</span>
                <span class="value">${escapeHtml(char.identity || '-')}</span>
            </div>
            <div class="info-item">
                <span class="label">注册时间</span>
                <span class="value">${registeredAt}</span>
            </div>
        </div>
    `;
    dialog.style.display = 'flex';
    document.getElementById('character-detail-close').onclick = () => {
        dialog.style.display = 'none';
    };
    dialog.onclick = (e) => {
        if (e.target === dialog) dialog.style.display = 'none';
    };
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
            loadRelationships();
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

        if (data.status) {
            const statusEl = document.getElementById('status');
            const text = data.status === 'alive' ? '存活' : data.status === 'dead' ? '死亡' : data.status;
            statusEl.innerHTML = '<span class="status-badge ' + data.status + '"><span class="status-dot"></span>' + text + '</span>';
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
        // 角色未注册（转生后或首次访问），显示提示并切到世界树
        if (err.message.includes('角色尚未注册') || err.message.includes('412')) {
            document.getElementById('character-info').innerHTML = `
                <div class="form-section">
                    <h2>当前无活跃角色</h2>
                    <p class="section-desc">角色已归隐或尚未创建。</p>
                    <div class="form-actions">
                        <a href="create.html" class="nav-link">创建新角色</a>
                    </div>
                </div>
            `;
            show('#character-info');
            // 切到世界树 tab
            document.querySelectorAll('.page-tab').forEach(t => t.classList.remove('active'));
            document.querySelector('[data-tab="worldtree"]').classList.add('active');
            document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
            document.getElementById('tab-worldtree').classList.add('active');
        } else {
            document.getElementById('error-message').textContent = err.message;
            show('.error');
        }
    }
}

// 渲染属性
function renderAttributes(attributes) {
    const attrsEl = document.getElementById('attributes');
    if (!attributes) {
        attrsEl.innerHTML = '<p class="no-data">暂无属性数据</p>';
        return;
    }

    const categories = attributeMeta ? attributeMeta.categories : null;
    const statusKeys = new Set(categories?.status || []);
    const primaryKeys = new Set(categories?.primary || []);
    const derivedKeys = new Set(categories?.derived || []);
    const knownKeys = new Set([...statusKeys, ...primaryKeys, ...derivedKeys]);

    const isRedundantMax = (key) => {
        if (typeof key !== 'string' || !key.endsWith('_max')) return false;
        const base = key.slice(0, -4);
        return knownKeys.has(base);
    };

    let html = '';

    // 先天属性
    const primaryList = categories?.primary || [];
    if (primaryList.length > 0) {
        html += '<div class="attr-section"><h4>先天属性</h4><div class="attr-group">';
        primaryList.forEach(key => {
            const attr = attributes[key];
            if (attr && typeof attr === 'object' && attr.current !== undefined) {
                html += `<div class="attr-item" title="${escapeHtml(attr.description || '')}">
                    <span class="attr-name">${escapeHtml(attr.name || key)}</span>
                    <span class="attr-value">${attr.current}</span>
                </div>`;
            }
        });
        html += '</div></div>';
    }

    // 状态属性
    const statusList = categories?.status || [];
    if (statusList.length > 0) {
        html += '<div class="attr-section"><h4>状态属性</h4><div class="attr-group">';
        statusList.forEach(key => {
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

    // 派生属性（从 attributes 中过滤掉已知 key 和 _max 冗余项）
    const derived = Object.keys(attributes)
        .filter(k => !knownKeys.has(k))
        .filter(k => !isRedundantMax(k))
        .filter(k => attributes[k] && typeof attributes[k] === 'object');

    if (derived.length > 0) {
        html += '<div class="attr-section"><h4>派生属性</h4><div class="attr-group">';
        derived.forEach(key => {
            const attr = attributes[key];
            if (attr && typeof attr === 'object' && attr.current !== undefined) {
                html += `<div class="attr-item" title="${escapeHtml(attr.description || '')}">
                    <span class="attr-name">${escapeHtml(attr.name || key)}</span>
                    <span class="attr-value">${attr.current}</span>
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
            relEl.innerHTML = data.relationships.map((rel, idx) => {
                const fav = rel.favorability ?? 0;
                const level = rel.relationship_level || 'neutral';
                const label = rel.relationship_label || '陌生人';
                const pct = Math.max(0, Math.min(100, Math.round(((fav + 100) / 200) * 100)));
                return `
                <div class="rel-item" data-rel-id="${rel.target_agent_id || idx}">
                    <div class="rel-item-left">
                        <span class="rel-name">${escapeHtml(rel.target_name || rel.target_agent_id || '未知')}</span>
                        <div class="rel-meta">
                            <span class="rel-label ${level}">${escapeHtml(label)}</span>
                        </div>
                    </div>
                    <div class="rel-right">
                        <div class="rel-favor-bar">
                            <div class="rel-favor-fill ${level}" style="width:${pct}%"></div>
                        </div>
                        <span class="rel-favor-value">${fav}</span>
                    </div>
                </div>`;
            }).join('');

            // 缓存关系数据供抽屉使用
            relEl._relationships = data.relationships;

            // 绑定点击事件
            relEl.querySelectorAll('.rel-item').forEach(item => {
                item.addEventListener('click', () => {
                    const id = item.dataset.relId;
                    const rel = relEl._relationships.find(r => (r.target_agent_id || '') === id);
                    if (rel) openRelationshipDrawer(rel);
                });
            });
        } else {
            relEl.innerHTML = '<p class="no-data">暂无关系记录</p>';
        }
    } catch (err) {
        relEl.innerHTML = '<p class="error-text">加载关系失败</p>';
    }
}

// 打开关系详情抽屉
function openRelationshipDrawer(rel) {
    if (!rel) return;

    const fav = rel.favorability ?? 0;
    const level = rel.relationship_level || 'neutral';
    const label = rel.relationship_label || '陌生人';
    const pct = Math.max(0, Math.min(100, Math.round(((fav + 100) / 200) * 100)));

    document.getElementById('drawer-name').textContent = rel.target_name || rel.target_agent_id || '未知';

    const labelEl = document.getElementById('drawer-label');
    labelEl.textContent = label;
    labelEl.className = 'drawer-label ' + level;

    const fillEl = document.getElementById('drawer-favorability-fill');
    fillEl.style.width = pct + '%';
    fillEl.className = 'favorability-fill ' + level;
    document.getElementById('drawer-favorability-value').textContent = fav;

    document.getElementById('drawer-description').textContent = rel.self_description || '暂无描述';

    // 渲染关键事件
    const eventsEl = document.getElementById('drawer-events');
    const events = rel.key_events || [];
    if (events.length > 0) {
        // 按时间倒序
        const sorted = [...events].sort((a, b) => (b.tick_id || 0) - (a.tick_id || 0));
        eventsEl.innerHTML = sorted.map(evt => {
            const delta = evt.favorability_delta || 0;
            const deltaCls = delta > 0 ? 'positive' : delta < 0 ? 'negative' : 'neutral';
            const deltaSign = delta > 0 ? '+' : '';
            return `
            <div class="drawer-event">
                <div class="drawer-event-header">
                    <span class="drawer-event-type">${escapeHtml(evt.event_type || '事件')}</span>
                    <span class="drawer-event-delta ${deltaCls}">${deltaSign}${delta}</span>
                </div>
                <div class="drawer-event-desc">${escapeHtml(evt.description || '')}</div>
                <div class="drawer-event-tick">Tick ${evt.tick_id || '-'}</div>
            </div>`;
        }).join('');
    } else {
        eventsEl.innerHTML = '<p class="no-data">暂无关键事件</p>';
    }

    // 打开抽屉
    const drawer = document.getElementById('relationship-drawer');
    const overlay = document.getElementById('relationship-drawer-overlay');
    drawer.classList.add('open');
    overlay.classList.add('open');
}

function closeRelationshipDrawer() {
    const drawer = document.getElementById('relationship-drawer');
    const overlay = document.getElementById('relationship-drawer-overlay');
    drawer.classList.remove('open');
    overlay.classList.remove('open');
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
    // SSE 连接：实时接收死亡事件
    let deathEventSource = null;
    function connectDeathEvents() {
        deathEventSource = new EventSource('/api/v1/events');
        deathEventSource.addEventListener('connected', () => {
            console.log('SSE connected');
        });
        deathEventSource.addEventListener('agent_died', (e) => {
            try {
                const data = JSON.parse(e.data);
                showError('角色已死亡：' + (data.description || '你已经死亡'));
                showDeathModal(data);
            } catch (err) {
                showError('角色已死亡');
                showDeathModal(null);
            }
        });
        deathEventSource.addEventListener('heartbeat', () => {
            // 连接存活，无需操作
        });
        deathEventSource.onerror = () => {
            console.warn('SSE connection lost, reconnecting...');
            deathEventSource.close();
            setTimeout(connectDeathEvents, 5000);
        };
    }
    connectDeathEvents();

    // 死亡通知弹窗
    function showDeathModal(data) {
        const modal = document.getElementById('death-notification-modal') || createDeathModal();
        document.getElementById('death-cause').textContent = data ? (data.description || '你已经死亡') : '你已经死亡';
        modal.style.display = 'flex';
    }
    function createDeathModal() {
        const div = document.createElement('div');
        div.id = 'death-notification-modal';
        div.className = 'dialog-overlay';
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
        div.querySelector('#death-goto-rebirth').addEventListener('click', () => {
            div.style.display = 'none';
            document.querySelectorAll('.page-tab').forEach(t => t.classList.remove('active'));
            document.querySelector('[data-tab="current"]').classList.add('active');
            document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
            document.getElementById('tab-current').classList.add('active');
            document.querySelectorAll('.vertical-tab').forEach(t => t.classList.remove('active'));
            document.querySelector('[data-vertical-tab="rebirth"]').classList.add('active');
            document.querySelectorAll('.vertical-tab-content').forEach(c => c.classList.remove('active'));
            document.getElementById('vertical-tab-rebirth').classList.add('active');
        });
        div.querySelector('#death-close').addEventListener('click', () => {
            div.style.display = 'none';
        });
        div.addEventListener('click', (e) => {
            if (e.target === div) div.style.display = 'none';
        });
        return div;
    }

    loadAttributeMeta().then(async () => {
        await loadCharacterList();
        // 当前角色非存活时，在当前角色 tab 显示创建入口
        const currentChar = allCharacters.find(c => c.is_current);
        if (!currentChar || currentChar.status !== 'alive') {
            hide('#loading');
            const infoEl = document.getElementById('character-info');
            infoEl.innerHTML = `
                <div class="form-section">
                    <h2>当前无活跃角色</h2>
                    <p class="section-desc">角色已归隐或尚未创建。</p>
                    <div class="form-actions">
                        <a href="create.html" class="nav-link">创建新角色</a>
                    </div>
                </div>
            `;
            show('#character-info');
            return;
        }
        loadCharacter();
    });

    document.getElementById('load-more-experiences-btn').addEventListener('click', loadMoreExperiences);
    document.getElementById('load-more-memories-btn').addEventListener('click', loadMoreMemories);
    document.getElementById('load-more-dream-records-btn').addEventListener('click', loadMoreDreamRecords);
    document.getElementById('character-select').addEventListener('change', switchCharacter);

    loadRelationships();
    loadMemories();
    loadDreamStatus();

    // 关系抽屉关闭事件
    document.getElementById('drawer-close').addEventListener('click', closeRelationshipDrawer);
    document.getElementById('relationship-drawer-overlay').addEventListener('click', closeRelationshipDrawer);
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') closeRelationshipDrawer();
    });

    // 横向标签页切换
    document.querySelectorAll('.page-tab').forEach(tab => {
        tab.addEventListener('click', () => {
            const targetTab = tab.dataset.tab;
            document.querySelectorAll('.page-tab').forEach(t => t.classList.remove('active'));
            tab.classList.add('active');
            document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
            document.getElementById('tab-' + targetTab).classList.add('active');
        });
    });

    // 加载托梦状态
    async function loadDreamStatus() {
        try {
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
        }
    }

    // 加载托梦记录
    async function loadDreamRecords(page = 1) {
        const recordsEl = document.getElementById('dream-records');
        const loadMoreEl = document.getElementById('load-more-dream-records');

        if (page === 1) {
            recordsEl.innerHTML = '<p class="loading-text">加载中...</p>';
        }

        try {
            const data = await apiGet(`/api/v1/character/dream/records?page=${page}&limit=${PAGE_LIMIT}`);
            hasMoreDreamRecords = data.has_more;
            dreamRecordPage = page;

            if (page === 1) recordsEl.innerHTML = '';

            if (data.records && data.records.length > 0) {
                data.records.forEach(record => {
                    const div = document.createElement('div');
                    div.className = 'exp-item';

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

    function loadMoreDreamRecords() {
        loadDreamRecords(dreamRecordPage + 1);
    }

    // 垂直标签页切换
    document.querySelectorAll('.vertical-tab').forEach(tab => {
        tab.addEventListener('click', () => {
            const targetTab = tab.dataset.verticalTab;
            document.querySelectorAll('.vertical-tab').forEach(t => t.classList.remove('active'));
            tab.classList.add('active');
            document.querySelectorAll('.vertical-tab-content').forEach(c => c.classList.remove('active'));
            document.getElementById('vertical-tab-' + targetTab).classList.add('active');
        });
    });

    // 转生按钮
    const rebirthBtn = document.getElementById('rebirth-btn');
    if (rebirthBtn) {
        rebirthBtn.addEventListener('click', async () => {
            if (!confirm('确定要让当前角色转生吗？此操作不可撤销。')) return;
            rebirthBtn.disabled = true;
            rebirthBtn.textContent = '转生中...';
            try {
                const data = await apiPost('/api/v1/character/rebirth', { confirm: true });
                if (data.success) {
                    document.getElementById('rebirth-message').textContent = data.message;
                    show(document.getElementById('rebirth-result'));
                    rebirthBtn.textContent = '已转生';
                    // 直接跳转到创建页面
                    setTimeout(() => { window.location.href = 'create.html'; }, 1000);
                } else {
                    document.getElementById('rebirth-error-msg').textContent = data.message || '服务器错误';
                    show(document.getElementById('rebirth-error'));
                    rebirthBtn.disabled = false;
                    rebirthBtn.textContent = '确认转生';
                }
            } catch (err) {
                document.getElementById('rebirth-error-msg').textContent = '网络错误: ' + err.message;
                show(document.getElementById('rebirth-error'));
                rebirthBtn.disabled = false;
                rebirthBtn.textContent = '确认转生';
            }
        });
    }

    // 托梦表单
    const dreamForm = document.getElementById('dream-form');
    if (dreamForm) {
        dreamForm.addEventListener('submit', async (e) => {
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
                loadDreamRecords();
            } catch (err) {
                showError(err.message);
                show(errorEl);
            } finally {
                btn.disabled = false;
                btn.textContent = '注入托梦';
            }
        });
    }
});
