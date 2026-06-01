// Character page: sidebar tabs + panel content

import { API, get } from './api.js';
import { escapeHtml, showLoading } from './ui.js';

export const characterPage = {
    mount(container) {
        showLoading(container);
        render(container);
    },
    unmount() {},
};

async function render(container) {
    container.innerHTML = `
        <div class="character-page">
            <div class="character-header" id="char-header">
                <p class="text-muted">加载角色信息...</p>
            </div>
            <div class="character-body">
                <div class="character-sidebar" id="char-sidebar"></div>
                <div class="character-content" id="char-content">
                    <p class="text-muted">选择左侧标签</p>
                </div>
            </div>
        </div>
    `;
}
