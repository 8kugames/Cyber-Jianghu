// Hash router: manages #/dashboard, #/characters, #/settings

import { escapeHtml } from './ui.js';

const routes = {};
let currentRoute = null;
let currentModule = null;

export function register(name, mod) {
    routes[name] = mod;
}

export function navigate(hash) {
    window.location.hash = hash;
}

export function init() {
    window.addEventListener('hashchange', onHashChange);
    onHashChange();
}

function onHashChange() {
    const hash = window.location.hash || '#/dashboard';
    const name = hash.replace('#/', '').split('/')[0] || 'dashboard';

    if (name === currentRoute) return;

    if (currentModule && currentModule.unmount) {
        currentModule.unmount();
    }

    const mod = routes[name];
    const container = document.getElementById('app');
    container.innerHTML = '';

    // Update nav active state
    document.querySelectorAll('.nav-tab').forEach(tab => {
        tab.classList.toggle('active', tab.dataset.route === name);
    });

    currentRoute = name;
    currentModule = mod || null;

    if (mod && mod.mount) {
        mod.mount(container);
    } else {
        container.innerHTML = `<div class="empty-state"><p>页面 "${escapeHtml(name)}" 不存在</p></div>`;
    }
}

export function getCurrentRoute() {
    return currentRoute;
}
