// 角色创建页逻辑

// 智能路由：检查服务器连接和角色状态
async function checkAndRedirect() {
    try {
        const data = await apiGet('/api/v1/character');
        if (data.agent_id && data.status === 'alive') {
            window.location.href = 'character.html';
        }
    } catch (err) {
        console.log('服务器连接检查失败，保持在创建页:', err.message);
    }
}

// 标签选择处理
function setupTagSelection(containerId, hiddenInputId) {
    const container = document.getElementById(containerId);
    const hiddenInput = document.getElementById(hiddenInputId);
    if (!container || !hiddenInput) return;

    const tags = container.querySelectorAll('.tag');
    tags.forEach(tag => {
        tag.addEventListener('click', () => {
            tag.classList.toggle('selected');
            updateHiddenInput(container, hiddenInput);
        });
    });
}

function updateHiddenInput(container, hiddenInput) {
    const selected = container.querySelectorAll('.tag.selected');
    const values = Array.from(selected).map(t => t.dataset.value);
    hiddenInput.value = JSON.stringify(values);
}

document.addEventListener('DOMContentLoaded', () => {
    checkAndRedirect();
    setupTagSelection('personality-tags', 'personality');
    setupTagSelection('values-tags', 'values');
    setupTagSelection('speech-patterns-tags', 'speech_patterns');

    document.getElementById('character-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const btn = document.getElementById('submit-btn');
        const resultDiv = document.getElementById('result');
        const errorDiv = document.getElementById('error');

        hide(resultDiv);
        hide(errorDiv);
        btn.disabled = true;
        btn.textContent = '创建中...';

        const formData = {
            name: document.getElementById('name').value.trim(),
            age: parseInt(document.getElementById('age').value) || 25,
            gender: document.getElementById('gender').value,
            appearance: document.getElementById('appearance').value.trim() || null,
            identity: document.getElementById('identity').value.trim() || null,
            personality: JSON.parse(document.getElementById('personality').value || '[]'),
            values: JSON.parse(document.getElementById('values').value || '[]'),
            language_style: {
                tone: document.getElementById('tone').value || null,
                speech_patterns: JSON.parse(document.getElementById('speech_patterns').value || '[]')
            },
            goals: {
                short_term: document.getElementById('short_term_goal').value.trim() || null,
                long_term: document.getElementById('long_term_goal').value.trim() || null
            }
        };

        try {
            const data = await apiPost('/api/v1/character/register', formData);
            document.getElementById('agent-id').textContent = data.agent_id;
            document.getElementById('message').textContent = data.message;
            show(resultDiv);
            document.getElementById('character-form').reset();
            document.querySelectorAll('.tag.selected').forEach(t => t.classList.remove('selected'));
            document.getElementById('personality').value = '';
            document.getElementById('values').value = '';
            document.getElementById('speech_patterns').value = '';
        } catch (err) {
            showError(err.message);
            show(errorDiv);
        } finally {
            btn.disabled = false;
            btn.textContent = '创建角色';
        }
    });
});
