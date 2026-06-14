// Action Evolution Tab
let currentGroupFilter = 'all';

function loadActionEvolution() {
    const container = document.getElementById('evolution-content');
    if (!container) return;

    container.innerHTML = `
        <div class="stats-grid" id="ae-stats"></div>
        <div class="ae-groups-section">
            <div class="ae-groups-header">
                <h3>提案组</h3>
                <div class="ae-filter-bar">
                    <button class="ae-filter-btn active" data-status="all" onclick="filterGroups('all')">全部</button>
                    <button class="ae-filter-btn" data-status="pending_review" onclick="filterGroups('pending_review')">待审核</button>
                    <button class="ae-filter-btn" data-status="approved" onclick="filterGroups('approved')">已通过</button>
                    <button class="ae-filter-btn" data-status="rejected" onclick="filterGroups('rejected')">已拒绝</button>
                    <button class="ae-filter-btn" data-status="escalated" onclick="filterGroups('escalated')">已上报</button>
                </div>
            </div>
            <div id="ae-groups-list"><p class="loading">加载中...</p></div>
        </div>
        <div id="ae-group-detail" style="display:none"></div>
    `;

    loadAEStats();
    loadGroups('all');
}

function loadAEStats() {
    const el = document.getElementById('ae-stats');
    fetch('/api/dashboard/action-evolution/stats', { headers: getAuthHeaders() })
        .then(r => r.json())
        .then(data => {
            let html = '';
            html += '<div class="stat-card"><div class="stat-value">' + (data.total_proposals || 0) + '</div><div class="stat-label">总提案数</div></div>';
            html += '<div class="stat-card"><div class="stat-value">' + (data.total_groups || 0) + '</div><div class="stat-label">总提案组</div></div>';
            if (data.by_status) {
                data.by_status.forEach(s => {
                    html += '<div class="stat-card"><div class="stat-value">' + s.count + '</div><div class="stat-label">' + s.status + '</div></div>';
                });
            }
            el.innerHTML = html;
        });
}

function filterGroups(status) {
    currentGroupFilter = status;
    document.querySelectorAll('.ae-filter-btn').forEach(btn => {
        btn.classList.toggle('active', btn.dataset.status === status);
    });
    loadGroups(status);
}

function loadGroups(status) {
    const el = document.getElementById('ae-groups-list');
    el.innerHTML = '<p class="loading">加载中...</p>';
    const url = status === 'all'
        ? '/api/dashboard/action-evolution/groups'
        : '/api/dashboard/action-evolution/groups?status=' + encodeURIComponent(status);

    fetch(url, { headers: getAuthHeaders() })
        .then(r => {
            if (!r.ok) throw new Error('HTTP ' + r.status);
            return r.json();
        })
        .then(data => {
            const groups = data.groups || [];
            if (groups.length === 0) {
                el.innerHTML = '<p class="empty">暂无提案组</p>';
                return;
            }
            let html = '<table class="data-table"><thead><tr>';
            html += '<th>ID</th><th>Soul</th><th>状态</th><th>创建时间</th><th>更新时间</th><th>操作</th>';
            html += '</tr></thead><tbody>';
            groups.forEach(g => {
                const idShort = g.id ? g.id.substring(0, 8) : '-';
                const created = g.created_at ? new Date(g.created_at).toLocaleString('zh-CN') : '-';
                const updated = g.updated_at ? new Date(g.updated_at).toLocaleString('zh-CN') : '-';
                html += '<tr>';
                html += '<td title="' + (g.id || '') + '">' + idShort + '...</td>';
                html += '<td>' + (g.primary_soul || '-') + '</td>';
                html += '<td><span class="status-badge status-' + (g.status || 'unknown') + '">' + (g.status || '-') + '</span></td>';
                html += '<td>' + created + '</td>';
                html += '<td>' + updated + '</td>';
                html += '<td><button class="btn btn-sm" onclick="showGroupDetail(\'' + g.id + '\')">详情</button></td>';
                html += '</tr>';
            });
            html += '</tbody></table>';
            el.innerHTML = html;
        })
        .catch(err => {
            el.innerHTML = '<p class="error">加载失败: ' + err.message + '</p>';
        });
}

function showGroupDetail(groupId) {
    const el = document.getElementById('ae-group-detail');
    const listEl = document.querySelector('.ae-groups-section');
    listEl.style.display = 'none';
    el.style.display = 'block';
    el.innerHTML = '<p class="loading">加载中...</p><button class="btn" onclick="hideGroupDetail()">返回列表</button>';

    fetch('/api/dashboard/action-evolution/groups/' + groupId, { headers: getAuthHeaders() })
        .then(r => {
            if (!r.ok) throw new Error('HTTP ' + r.status);
            return r.json();
        })
        .then(data => {
            let html = '<button class="btn" onclick="hideGroupDetail()">返回列表</button>';
            html += '<h3>提案组详情</h3>';
            html += '<table class="data-table detail-table">';
            html += '<tr><th>ID</th><td>' + (data.id || '-') + '</td></tr>';
            html += '<tr><th>相似度键</th><td>' + (data.similarity_key || '-') + '</td></tr>';
            html += '<tr><th>Primary Soul</th><td>' + (data.primary_soul || '-') + '</td></tr>';
            html += '<tr><th>状态</th><td><span class="status-badge status-' + (data.status || 'unknown') + '">' + (data.status || '-') + '</span></td></tr>';
            html += '<tr><th>Co-Reviewers</th><td>' + formatJsonField(data.co_reviewers) + '</td></tr>';
            html += '<tr><th>治理主题</th><td>' + formatJsonField(data.governance_topics) + '</td></tr>';
            html += '<tr><th>投票</th><td>' + formatJsonField(data.votes) + '</td></tr>';
            html += '<tr><th>最终决策</th><td>' + (data.final_decision || '-') + '</td></tr>';
            html += '<tr><th>异议记录</th><td>' + formatJsonField(data.dissent_log) + '</td></tr>';
            html += '<tr><th>提案 ID</th><td>' + formatJsonField(data.proposal_ids) + '</td></tr>';
            html += '<tr><th>创建时间</th><td>' + (data.created_at ? new Date(data.created_at).toLocaleString('zh-CN') : '-') + '</td></tr>';
            html += '<tr><th>更新时间</th><td>' + (data.updated_at ? new Date(data.updated_at).toLocaleString('zh-CN') : '-') + '</td></tr>';
            html += '</table>';

            // 审批操作按钮（仅 pending_review / under_review 状态可操作）
            if (data.status === 'pending_review' || data.status === 'under_review') {
                html += '<div class="ae-action-bar" style="margin-top:16px;display:flex;gap:8px">';
                html += '<button class="btn btn-success" onclick="adminGroupAction(\'' + groupId + '\', \'approve\')">通过</button>';
                html += '<button class="btn btn-danger" onclick="adminGroupAction(\'' + groupId + '\', \'reject\')">驳回</button>';
                html += '</div>';
            }

            el.innerHTML = html;
        })
        .catch(err => {
            el.innerHTML = '<button class="btn" onclick="hideGroupDetail()">返回列表</button><p class="error">加载失败: ' + err.message + '</p>';
        });
}

function hideGroupDetail() {
    document.getElementById('ae-group-detail').style.display = 'none';
    document.querySelector('.ae-groups-section').style.display = '';
}

function formatJsonField(val) {
    if (val === null || val === undefined) return '-';
    if (typeof val === 'string') return val;
    return '<pre class="json-pre">' + JSON.stringify(val, null, 2) + '</pre>';
}

function adminGroupAction(groupId, action) {
    const reason = action === 'approve'
        ? prompt('通过理由（可选）：', '管理员手动通过')
        : prompt('驳回理由（必填）：', '');
    if (reason === null) return;
    if (action === 'reject' && !reason.trim()) {
        alert('驳回理由不能为空');
        return;
    }

    apiFetch('/api/dashboard/action-evolution/groups/' + groupId + '/action', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: action, reason: reason })
    })
    .then(r => r.json())
    .then(data => {
        alert('操作成功: ' + (data.new_status || action));
        hideGroupDetail();
        loadGroups(currentGroupFilter);
        loadAEStats();
    })
    .catch(err => {
        if (err.status === 401) return; // apiFetch 已处理
        alert('操作失败: ' + err.message);
    });
}
