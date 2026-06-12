// Action Evolution Tab
function loadActionEvolution() {
    fetch('/api/dashboard/action-evolution/stats', {
        headers: getAuthHeaders()
    })
    .then(r => r.json())
    .then(data => {
        const container = document.getElementById('evolution-content');
        if (!container) return;
        
        let html = '<div class="stats-grid">';
        html += '<div class="stat-card"><div class="stat-value">' + (data.total_proposals || 0) + '</div><div class="stat-label">总提案数</div></div>';
        html += '</div>';
        
        if (data.by_soul && data.by_soul.length > 0) {
            html += '<h3>按 Soul 分组</h3>';
            html += '<table class="data-table"><thead><tr><th>Soul</th><th>数量</th><th>状态</th></tr></thead><tbody>';
            data.by_soul.forEach(row => {
                html += '<tr><td>' + (row.soul || 'unassigned') + '</td><td>' + row.count + '</td><td>' + row.status + '</td></tr>';
            });
            html += '</tbody></table>';
        }
        
        container.innerHTML = html;
    })
    .catch(err => console.error('Failed to load action evolution stats:', err));
}