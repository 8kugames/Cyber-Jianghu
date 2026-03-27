// 角色创建页逻辑

// 检查角色状态，决定显示创建表单还是阻止信息
async function checkCharacterAndBlock() {
    try {
        const data = await apiGet('/api/v1/character');
        // status 是 'active', 'retired', 'dead'，不是中文
        if (data.agent_id && data.status === 'active') {
            document.getElementById('character-form').classList.add('hidden');
            document.getElementById('character-blocked').classList.remove('hidden');
            return;
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

// 选中标签（根据值数组）
function selectTagsByValues(containerId, hiddenInputId, values) {
    const container = document.getElementById(containerId);
    const hiddenInput = document.getElementById(hiddenInputId);
    if (!container || !hiddenInput) return;

    // 先清除所有选中状态
    container.querySelectorAll('.tag.selected').forEach(t => t.classList.remove('selected'));

    // 按值选中
    const tags = container.querySelectorAll('.tag');
    tags.forEach(tag => {
        if (values.includes(tag.dataset.value)) {
            tag.classList.add('selected');
        }
    });

    updateHiddenInput(container, hiddenInput);
}

// 一键生成角色
async function generateCharacter() {
    const btn = document.getElementById('generate-btn');
    const hint = document.getElementById('generate-hint');

    btn.disabled = true;
    btn.textContent = '生成中...';
    hint.textContent = '';
    hint.className = 'hint';

    try {
        const data = await apiPost('/api/v1/character/generate', {}, { timeout: 120000 });

        // 填充文本字段
        document.getElementById('name').value = data.name || '';
        document.getElementById('age').value = data.age || 25;
        document.getElementById('gender').value = data.gender || '男';
        document.getElementById('appearance').value = data.appearance || '';
        document.getElementById('identity').value = data.identity || '';
        document.getElementById('short_term_goal').value = data.goals?.short_term || '';
        document.getElementById('long_term_goal').value = data.goals?.long_term || '';

        // 设置语调下拉框
        const toneSelect = document.getElementById('tone');
        toneSelect.value = data.language_style?.tone || '';

        // 选中标签
        selectTagsByValues('personality-tags', 'personality', data.personality || []);
        selectTagsByValues('values-tags', 'values', data.values || []);
        selectTagsByValues('speech-patterns-tags', 'speech_patterns', data.language_style?.speech_patterns || []);

        hint.textContent = '角色已生成，可修改后提交';
        hint.className = 'hint';
    } catch (err) {
        hint.textContent = err.message;
        hint.className = 'hint error';
    } finally {
        btn.disabled = false;
        btn.textContent = '一键生成';
    }
}

document.addEventListener('DOMContentLoaded', () => {
    checkCharacterAndBlock();
    setupTagSelection('personality-tags', 'personality');
    setupTagSelection('values-tags', 'values');
    setupTagSelection('speech-patterns-tags', 'speech_patterns');

    document.getElementById('generate-btn').addEventListener('click', generateCharacter);

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

            // 1.5 秒后自动跳转到角色详情页
            setTimeout(() => {
                window.location.href = 'character.html';
            }, 1500);
        } catch (err) {
            showError(err.message);
            show(errorDiv);
        } finally {
            btn.disabled = false;
            btn.textContent = '创建角色';
        }
    });
});
