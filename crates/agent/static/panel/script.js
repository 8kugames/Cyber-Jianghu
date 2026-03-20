// 赛博江湖 - 角色创建面板脚本

// 获取当前主机和端口
const API_BASE = `${window.location.protocol}//${window.location.host}`;

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
    const selectedTags = container.querySelectorAll('.tag.selected');
    const values = Array.from(selectedTags).map(tag => tag.dataset.value);
    hiddenInput.value = JSON.stringify(values);
}

// 初始化标签选择
document.addEventListener('DOMContentLoaded', () => {
    setupTagSelection('personality-tags', 'personality');
    setupTagSelection('values-tags', 'values');
    setupTagSelection('speech-patterns-tags', 'speech_patterns');
});

// 表单提交处理
document.getElementById('character-form').addEventListener('submit', async (e) => {
    e.preventDefault();

    const submitBtn = document.getElementById('submit-btn');
    const resultDiv = document.getElementById('result');
    const errorDiv = document.getElementById('error');

    // 隐藏之前的结果
    resultDiv.classList.add('hidden');
    errorDiv.classList.add('hidden');

    // 禁用提交按钮
    submitBtn.disabled = true;
    submitBtn.textContent = '创建中...';

    // 收集表单数据
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
        const response = await fetch(`${API_BASE}/api/v1/character/register`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify(formData)
        });

        const data = await response.json();

        if (response.ok) {
            // 显示成功结果
            document.getElementById('agent-id').textContent = data.agent_id;
            document.getElementById('message').textContent = data.message;
            resultDiv.classList.remove('hidden');

            // 清空表单
            document.getElementById('character-form').reset();
            document.querySelectorAll('.tag.selected').forEach(tag => {
                tag.classList.remove('selected');
            });
            document.getElementById('personality').value = '';
            document.getElementById('values').value = '';
            document.getElementById('speech_patterns').value = '';
        } else {
            // 显示错误
            document.getElementById('error-message').textContent =
                data.message || `服务器错误: ${response.status}`;
            errorDiv.classList.remove('hidden');
        }
    } catch (err) {
        // 显示网络错误
        document.getElementById('error-message').textContent =
            `网络错误: ${err.message}。请确保 Agent 正在运行。`;
        errorDiv.classList.remove('hidden');
    } finally {
        // 恢复提交按钮
        submitBtn.disabled = false;
        submitBtn.textContent = '创建角色';
    }
});
