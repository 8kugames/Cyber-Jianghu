// Panel renderers: attribute/relationship/biography/experience/memory/dream/skill

import { API, get, post } from './api.js';
import { escapeHtml, showSuccess, showError, showLoading, formatWorldTime, showModal } from './ui.js';

// Panel registry: each panel exports { label, mount(container, data) }
const panels = {
    attributes: {
        label: '属性',
        mount: mountAttributes,
    },
    relationships: {
        label: '关系',
        mount: mountRelationships,
    },
    biography: {
        label: '传记',
        mount: mountBiography,
    },
    experiences: {
        label: '经历',
        mount: mountExperiences,
    },
    memories: {
        label: '记忆',
        mount: mountMemories,
    },
    dream: {
        label: '梦境',
        mount: mountDream,
    },
    skills: {
        label: '技能',
        mount: mountSkills,
    },
};

export function getPanelDefinitions() {
    return Object.entries(panels).map(([id, p]) => ({ id, label: p.label }));
}

export async function mountPanel(panelId, container, context) {
    const panel = panels[panelId];
    if (panel) {
        container.classList.remove('panel-enter');
        void container.offsetWidth; // force reflow
        container.classList.add('panel-enter');
        await panel.mount(container, context);
    } else {
        container.innerHTML = '<p class="text-muted">未知面板</p>';
    }
}

// ============================================================================
// Attributes
// ============================================================================

async function mountAttributes(container, ctx) {
    showLoading(container);
    try {
        const [attrs, meta] = await Promise.all([
            get(API.ATTRIBUTES),
            get(API.ATTRIBUTE_META),
        ]);

        const categories = meta.categories || {};
        const displayNames = meta.display_names || {};
        const allAttrs = attrs.attributes || [];

        const CAT_NAMES = { primary: '基础属性', derived: '衍生属性', status: '状态' };

        let html = '';
        for (const [catName, attrNames] of Object.entries(categories)) {
            const catLabel = CAT_NAMES[catName] || displayNames[catName] || catName;
            html += `<h3 style="font-size:14px;font-weight:600;margin:${html ? '16px' : '0'} 0 8px;color:var(--text-secondary)">${escapeHtml(catLabel)}</h3>`;
            html += '<div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:8px">';
            for (const attrName of attrNames) {
                const attr = allAttrs.find(a => a.name === attrName);
                if (!attr) continue;
                const label = displayNames[attrName] || attrName;
                html += `<div class="card" style="padding:10px"><div style="font-size:11px;color:var(--text-muted)">${escapeHtml(label)}</div><div style="font-size:16px;font-weight:600">${escapeHtml(attr.value_str)}</div></div>`;
            }
            html += '</div>';
        }

        container.innerHTML = html || '<p class="text-muted">无属性数据</p>';
    } catch (e) {
        container.innerHTML = `<p class="text-muted">属性加载失败: ${escapeHtml(e.message)}</p>`;
    }
}

// ============================================================================
// Relationships
// ============================================================================

async function mountRelationships(container, ctx) {
    showLoading(container);
    try {
        const data = await get(API.RELATIONSHIP_LIST);
        const relationships = data.relationships || data || [];

        if (relationships.length === 0) {
            container.innerHTML = '<p class="text-muted">无关系数据</p>';
            return;
        }

        let html = '';
        for (const rel of relationships) {
            const name = rel.target_name || rel.name || '-';
            const label = rel.relationship_label || '-';
            const favor = rel.favorability ?? rel.level ?? '-';
            const color = typeof favor === 'number' ? (favor >= 70 ? 'var(--success)' : favor >= 30 ? 'var(--warning)' : 'var(--danger)') : 'var(--text-muted)';
            const initial = name.charAt(0);
            html += `
            <div class="card rel-card" style="padding:12px;margin-bottom:8px;cursor:pointer" data-rel-id="${escapeHtml(rel.target_id || '')}">
                <div style="display:flex;align-items:center;gap:12px">
                    <div class="rel-avatar">${escapeHtml(initial)}</div>
                    <div style="flex:1;min-width:0">
                        <div style="font-weight:600">${escapeHtml(name)}</div>
                        <div style="font-size:12px;color:var(--text-muted)">${escapeHtml(label)}</div>
                    </div>
                    <div style="font-size:14px;font-weight:600;color:${color}">${favor}</div>
                </div>
            </div>`;
        }
        container.innerHTML = html;

        container.querySelectorAll('[data-rel-id]').forEach(card => {
            card.addEventListener('click', () => {
                const id = card.dataset.relId;
                const rel = relationships.find(r => r.target_id === id);
                if (rel) showRelationshipDetail(rel);
            });
        });
    } catch (e) {
        container.innerHTML = `<p class="text-muted">关系加载失败</p>`;
    }
}

function showRelationshipDetail(rel) {
    const html = `
        <h3 style="margin-bottom:12px">${escapeHtml(rel.target_name || '-')}</h3>
        <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px">
            <div><span style="color:var(--text-muted);font-size:12px">关系</span><br>${escapeHtml(rel.relationship_label || '-')}</div>
            <div><span style="color:var(--text-muted);font-size:12px">好感度</span><br>${rel.favorability ?? rel.level ?? '-'}</div>
            <div><span style="color:var(--text-muted);font-size:12px">信任</span><br>${rel.trust ?? '-'}</div>
            <div><span style="color:var(--text-muted);font-size:12px">互动</span><br>${rel.interaction_count ?? '-'}</div>
        </div>
        ${rel.key_events ? `<div style="margin-top:12px"><div style="color:var(--text-muted);font-size:12px;margin-bottom:4px">关键事件</div>${escapeHtml(rel.key_events)}</div>` : ''}
        <div style="margin-top:16px;text-align:right"><button class="btn" onclick="document.getElementById('modal-overlay').classList.add('hidden')">关闭</button></div>
    `;
    showModal(html);
}

// ============================================================================
// Biography
// ============================================================================

async function mountBiography(container, ctx) {
    showLoading(container);
    try {
        const agentId = ctx.agentId || '';
        const param = agentId ? `?agent_id=${agentId}` : '';
        const data = await get(`${API.BIOGRAPHY}${param}`);
        const bio = data.biography;

        if (!bio) {
            container.innerHTML = `
                <div class="empty-state">
                    <p class="text-muted">暂无传记</p>
                    <button class="btn btn-primary" id="gen-bio-btn" style="margin-top:12px">生成传记</button>
                </div>`;
            document.getElementById('gen-bio-btn')?.addEventListener('click', async () => {
                const btn = document.getElementById('gen-bio-btn');
                btn.disabled = true;
                btn.textContent = '生成中...';
                try {
                    const result = await post(`${API.BIOGRAPHY}${param}`);
                    if (result.biography) {
                        mountBiography(container, ctx);
                        showSuccess('传记生成成功');
                    }
                } catch (e) {
                    showError('生成失败: ' + e.message);
                    btn.disabled = false;
                    btn.textContent = '生成传记';
                }
            });
            return;
        }

        container.innerHTML = `
            <div style="padding:8px">
                <div style="font-size:14px;line-height:1.8;white-space:pre-wrap">${escapeHtml(bio)}</div>
            </div>`;
    } catch (e) {
        container.innerHTML = `<p class="text-muted">传记加载失败</p>`;
    }
}

// ============================================================================
// Experiences (Soul Cycles)
// ============================================================================

let expPage = 1;

const LAYER_NAMES = { layer1: '动作审查', layer2: '规则校验', layer3: '意图审查' };
const SPEAK_TYPES = { speak: true, talk: true, say: true, chat: true };
const WHISPER_TYPES = { whisper: true, murmur: true };
const SHOUT_TYPES = { shout: true, yell: true };

async function mountExperiences(container, ctx) {
    expPage = 1;
    showLoading(container);
    await loadExpPage(container, ctx);
}

async function loadExpPage(container, ctx) {
    try {
        const agentId = ctx.agentId || '';
        const param = agentId ? `&agent_id=${agentId}` : '';
        const data = await get(`${API.SOUL_CYCLES}?page=${expPage}&limit=10${param}`);

        // records: { tick_id: [SoulCycleAttemptEntry] }, immediate_intents: { tick_id: [...] }
        let recordMap = data.records || {};
        let immMap = data.immediate_intents || {};
        if (Array.isArray(recordMap)) { recordMap = groupByTick(recordMap); }

        const tickIds = Object.keys(recordMap).sort((a, b) => Number(b) - Number(a));

        if (tickIds.length === 0 && expPage === 1) {
            container.innerHTML = '<p class="text-muted">暂无经历记录</p>';
            return;
        }

        let html = '';
        for (const tickId of tickIds) {
            const attempts = recordMap[tickId] || [];
            const immediate = immMap[tickId] || [];
            html += renderTickCard(tickId, attempts, immediate);
        }

        if (data.has_more || tickIds.length >= 10) {
            html += `<div style="text-align:center;margin-top:12px"><button class="btn btn-sm" id="exp-load-more">加载更多</button></div>`;
        }

        if (expPage === 1) {
            container.innerHTML = `<div class="exp-list">${html}</div>`;
        } else {
            const list = container.querySelector('.exp-list');
            if (list) list.insertAdjacentHTML('beforeend', html);
        }

        document.getElementById('exp-load-more')?.addEventListener('click', () => {
            document.getElementById('exp-load-more')?.remove();
            expPage++;
            loadExpPage(container, ctx);
        });
    } catch (e) {
        if (expPage === 1) container.innerHTML = '<p class="text-muted">经历加载失败</p>';
    }
}

function groupByTick(arr) {
    const m = {};
    for (const r of arr) {
        const k = String(r.tick_id || 0);
        (m[k] ||= []).push(r);
    }
    return m;
}

function renderTickCard(tickId, attempts, immediate) {
    const first = attempts[0];
    const wt = first?.world_time ? formatWorldTime(first.world_time) : '-';
    const ts = first?.created_at ? new Date(first.created_at).toLocaleString('zh-CN') : '';

    let html = `<div class="tl-item"><div class="tl-dot"></div><div class="tl-content">`;
    html += `<div class="tick-card-header">`;
    html += `<span class="tick-badge">T${escapeHtml(tickId)}</span>`;
    html += `<span class="tick-world-time">${escapeHtml(wt)}</span>`;
    html += `<span class="tick-real-time">${escapeHtml(ts)}</span>`;
    html += `</div>`;

    // 行动区
    html += `<div class="tick-section"><div class="tick-section-title">行动</div>`;
    html += `<div class="tick-attempts-container">`;
    attempts.forEach((att, idx) => {
        html += `<div class="tick-attempt-box">`;
        if (attempts.length > 1) html += `<div class="tick-attempt-label">行动 ${idx + 1}</div>`;
        html += renderRenhun(att.renhun);
        html += renderTianhun(att.tianhun);
        if (att.final_intent) html += renderDihun(att.final_intent);
        html += `</div>`;
    });
    html += `</div></div>`;

    // 即时区
    if (immediate.length > 0) {
        html += `<div class="tick-section tick-section-immediate"><div class="tick-section-title">即时</div>`;
        for (const imm of immediate) {
            const statusCls = imm.send_status === 'sent' ? 'sent' : 'failed';
            const statusText = imm.send_status === 'sent' ? '已发送' : '失败';
            html += `<div class="imm-item">`;
            html += `<div class="exp-immediate"><span class="exp-soul-label">即时</span>`;
            html += `<div class="exp-soul-content">`;
            html += renderActionText(imm.action_type, { content: imm.speech_content, target_agent_id: imm.target_agent_id });
            html += `</div></div>`;
            html += `<span class="imm-status ${statusCls}">${escapeHtml(statusText)}</span>`;
            if (imm.send_error) html += `<span class="imm-error">${escapeHtml(imm.send_error)}</span>`;
            html += `</div>`;
        }
        html += `</div>`;
    }

    html += `</div></div></div>`;
    return html;
}

function renderRenhun(data) {
    if (!data) return '';
    const hasNarrative = data.narrative;
    const hasThought = data.thought_log;
    if (!hasNarrative && !hasThought) return '';

    let html = `<div class="exp-renhun"><span class="exp-soul-label">人魂</span><div class="exp-soul-content">`;
    if (hasNarrative) html += `<div class="soul-text">${escapeHtml(data.narrative)}</div>`;
    if (hasThought) html += `<div class="soul-thought">${escapeHtml(data.thought_log)}</div>`;
    html += `</div></div>`;
    return html;
}

function renderTianhun(data) {
    if (!data) return '';
    let html = `<div class="exp-tianhun"><span class="exp-soul-label">天魂</span><div class="exp-soul-content">`;

    if (data.result) {
        const isApproved = data.result === 'approved';
        html += `<div class="soul-result ${isApproved ? 'approved' : 'rejected'}">${isApproved ? '通过' : '驳回'}</div>`;
    }
    if (data.layers && data.layers.length > 0) {
        html += `<div class="soul-layers">`;
        for (const l of data.layers) {
            const name = LAYER_NAMES[l.layer] || l.layer;
            html += `<span class="soul-layer-tag ${l.passed ? 'passed' : 'failed'}">${escapeHtml(name)}`;
            if (!l.passed && l.detail) html += `: ${escapeHtml(l.detail)}`;
            html += `</span>`;
        }
        html += `</div>`;
    }
    if (data.reason) html += `<div class="soul-reason">${escapeHtml(data.reason)}</div>`;
    if (data.narrative) html += `<div class="soul-narrative">${escapeHtml(data.narrative)}</div>`;

    html += `</div></div>`;
    return html;
}

function renderDihun(data) {
    if (!data) return '';
    let html = `<div class="exp-dihun"><span class="exp-soul-label">地魂</span><div class="exp-soul-content">`;

    const pipeline = data.pipeline_actions;
    if (pipeline && pipeline.length >= 1) {
        const multi = pipeline.length > 1;
        pipeline.forEach((pa, idx) => {
            if (multi) html += `<div style="font-size:10px;color:var(--text-muted);margin-top:${idx > 0 ? '4px' : '0'}">意图 ${idx + 1}</div>`;
            html += renderActionText(pa.action_type, pa.action_data);
        });
    } else if (data.action_type) {
        html += renderActionText(data.action_type, data.action_data);
    }

    html += `</div></div>`;
    return html;
}

function renderActionText(actionType, actionData) {
    const at = actionType || '';
    let ad = actionData;
    if (typeof ad === 'string') { try { ad = JSON.parse(ad); } catch { ad = {}; } }
    if (!ad || typeof ad !== 'object') ad = {};
    const content = ad.content || '';
    const targetId = ad.target_agent_id;

    let html = '<div class="soul-text">';
    if (SPEAK_TYPES[at] && content) {
        const label = targetId ? `对${targetId.substring(0, 8)}...说话` : '向在场众人说话';
        html += `${escapeHtml(label)}："${escapeHtml(content)}"`;
    } else if (WHISPER_TYPES[at] && content) {
        html += `密语："${escapeHtml(content)}"`;
    } else if (SHOUT_TYPES[at] && content) {
        html += `大喊："${escapeHtml(content)}"`;
    } else {
        html += escapeHtml(at);
        const keys = Object.keys(ad).filter(k => ad[k] != null && ad[k] !== '');
        if (keys.length > 0) html += ` <span class="soul-params">${escapeHtml(JSON.stringify(ad, null, 0))}</span>`;
    }
    html += '</div>';
    return html;
}

// ============================================================================
// Memory
// ============================================================================

let memPage = 1;

async function mountMemories(container, ctx) {
    memPage = 1;
    showLoading(container);

    container.innerHTML = `
        <div style="margin-bottom:12px">
            <div style="display:flex;gap:8px">
                <input class="form-input" type="text" id="mem-search-input" placeholder="搜索记忆...">
                <button class="btn" id="mem-search-btn">搜索</button>
            </div>
        </div>
        <div id="mem-list"></div>
    `;

    await loadMemPage(container, ctx);

    document.getElementById('mem-search-btn')?.addEventListener('click', async () => {
        const query = document.getElementById('mem-search-input')?.value?.trim();
        if (!query) return;
        const list = document.getElementById('mem-list');
        if (!list) return;
        showLoading(list);
        try {
            const results = await post(API.MEMORY_SEARCH, { query });
            const memories = results.memories || results || [];
            if (memories.length === 0) {
                list.innerHTML = '<p class="text-muted">无匹配结果</p>';
                return;
            }
            let html = '';
            for (const m of memories) {
                html += `<div class="card" style="padding:10px;margin-bottom:6px"><div style="font-size:12px;color:var(--text-muted)">Tick ${m.tick_id ?? '-'} · ${m.importance ?? ''}</div><div style="font-size:13px;margin-top:2px">${escapeHtml(m.content || '-')}</div></div>`;
            }
            list.innerHTML = html;
        } catch (e) {
            list.innerHTML = '<p class="text-muted">搜索失败</p>';
        }
    });
}

async function loadMemPage(container, ctx) {
    const list = document.getElementById('mem-list');
    if (!list) return;

    try {
        const data = await get(`${API.MEMORY_RECENT}?page=${memPage}&limit=20`);
        const memories = data.memories || data || [];

        if (memories.length === 0 && memPage === 1) {
            list.innerHTML = '<p class="text-muted">暂无记忆</p>';
            return;
        }

        let html = '';
        for (const m of memories) {
            html += `<div class="card" style="padding:10px;margin-bottom:6px"><div style="font-size:12px;color:var(--text-muted)">Tick ${m.tick_id ?? '-'}</div><div style="font-size:13px;margin-top:2px">${escapeHtml(m.content || '-')}</div></div>`;
        }

        if (memories.length >= 20) {
            html += `<div style="text-align:center;margin-top:8px"><button class="btn btn-sm" id="mem-load-more">加载更多</button></div>`;
        }

        if (memPage === 1) {
            list.innerHTML = html;
        } else {
            list.insertAdjacentHTML('beforeend', html);
        }

        document.getElementById('mem-load-more')?.addEventListener('click', () => {
            document.getElementById('mem-load-more')?.remove();
            memPage++;
            loadMemPage(container, ctx);
        });
    } catch (e) {
        if (memPage === 1) list.innerHTML = '<p class="text-muted">记忆加载失败</p>';
    }
}

// ============================================================================
// Dream
// ============================================================================

async function mountDream(container, ctx) {
    showLoading(container);
    try {
        const [status, records] = await Promise.allSettled([
            get(API.DREAM),
            get(`${API.DREAM_RECORDS}?page=1&limit=20`),
        ]);

        let html = '<h3 style="font-size:14px;font-weight:600;margin-bottom:8px">注入梦境</h3>';
        html += `
        <form id="dream-form" style="margin-bottom:16px">
            <div class="form-group">
                <label class="form-label">梦境内容</label>
                <textarea class="form-input" id="dream-thought" rows="3" placeholder="输入要注入的思考内容" required></textarea>
            </div>
            <div style="display:flex;gap:8px;align-items:end">
                <div class="form-group" style="flex:0 0 120px;margin-bottom:0">
                    <label class="form-label">持续轮数</label>
                    <input class="form-input" type="number" id="dream-duration" min="1" max="100" value="5">
                </div>
                <button type="submit" class="btn btn-primary">注入</button>
            </div>
        </form>`;

        // Current dream status
        if (status.status === 'fulfilled' && status.value?.active) {
            html += `<div class="card" style="padding:10px;margin-bottom:12px;border-left:3px solid var(--accent)"><div style="font-size:12px;color:var(--text-muted)">当前梦境 (剩余 ${status.value.remaining ?? '?'} 轮)</div><div style="font-size:13px;margin-top:2px">${escapeHtml(status.value.thought || '')}</div></div>`;
        }

        // Records
        html += '<h3 style="font-size:14px;font-weight:600;margin:12px 0 8px">梦境记录</h3>';
        const recs = records.status === 'fulfilled' ? (records.value.records || records.value || []) : [];
        if (recs.length === 0) {
            html += '<p class="text-muted">暂无梦境记录</p>';
        } else {
            for (const r of recs) {
                html += `<div class="card" style="padding:10px;margin-bottom:6px"><div style="font-size:12px;color:var(--text-muted)">${escapeHtml(r.injected_at || '-')} · ${r.duration ?? '?'}轮</div><div style="font-size:13px;margin-top:2px">${escapeHtml(r.thought || '-')}</div></div>`;
            }
        }

        container.innerHTML = html;

        document.getElementById('dream-form')?.addEventListener('submit', async (e) => {
            e.preventDefault();
            const thought = document.getElementById('dream-thought')?.value?.trim();
            const duration = parseInt(document.getElementById('dream-duration')?.value) || 5;
            if (!thought) return;
            try {
                await post(API.DREAM, { thought, duration });
                showSuccess('梦境已注入');
                mountDream(container, ctx);
            } catch (e) {
                showError('注入失败: ' + e.message);
            }
        });
    } catch (e) {
        container.innerHTML = '<p class="text-muted">梦境数据加载失败</p>';
    }
}

// ============================================================================
// Skills
// ============================================================================

async function mountSkills(container, ctx) {
    showLoading(container);
    try {
        const charData = ctx.character || {};
        const skills = charData.skills || [];

        if (skills.length === 0) {
            container.innerHTML = '<p class="text-muted">暂无已学技能</p>';
            return;
        }

        let html = '<div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr));gap:10px">';
        for (const skill of skills) {
            const name = skill.name || skill.skill_id || '-';
            const desc = skill.description || '';
            html += `
            <div class="card" style="padding:12px">
                <div style="font-weight:600;font-size:14px">${escapeHtml(name)}</div>
                ${desc ? `<div style="font-size:12px;color:var(--text-muted);margin-top:4px">${escapeHtml(desc)}</div>` : ''}
            </div>`;
        }
        html += '</div>';
        container.innerHTML = html;
    } catch (e) {
        container.innerHTML = '<p class="text-muted">技能数据加载失败</p>';
    }
}

