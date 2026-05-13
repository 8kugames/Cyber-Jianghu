// Auth Functions
// Note: authToken is declared in utils.js (loaded first)

// Don't reset authVerified if already set by URL token flow (utils.js)
// auth.js loads after utils.js, so authVerified may already be true from URL token
var authVerified = typeof authVerified !== "undefined" ? authVerified : false;
// Note: authTokenType is declared in utils.js (loaded first)

class ApiError extends Error {
    constructor(status, message) {
        super(message);
        this.status = status;
        this.name = "ApiError";
    }
}

// 统一的 API 请求包装器，处理鉴权和全局错误
async function apiFetch(url, options = {}) {
    options.headers = options.headers || {};
    if (authToken) {
        options.headers["Authorization"] = "Bearer " + authToken;
    }
    
    try {
        const res = await fetch(url, options);
        
        // 统一处理 401 鉴权失败
        if (res.status === 401) {
            // 如果曾验证过但现在 401，说明 Token 过期，需要重置状态并唤起弹窗
            if (authVerified) {
                authVerified = false;
                authToken = null;
                localStorage.removeItem("admin_token");
                localStorage.removeItem("admin_token_type");
            }
            showAuthModal();
            throw new ApiError(401, "UNAUTHORIZED");
        }
        
        // 统一处理 5xx 网关/服务异常
        if (res.status >= 500) {
            console.error(`[API] Server Error ${res.status} on ${url}`);
            showToast("服务器内部错误，请稍后重试", "error");
            throw new ApiError(res.status, "SERVER_ERROR");
        }
        
        return res;
    } catch (e) {
        // 网络层异常 (如 net::ERR_CONNECTION_REFUSED)
        if (e.name === "TypeError" && e.message === "Failed to fetch") {
            console.error(`[API] Network Error on ${url}`, e);
            showToast("网络连接失败，请检查服务状态", "error");
            throw new ApiError(0, "NETWORK_ERROR");
        }
        throw e; // 继续向上抛出 ApiError 或其他 Error
    }
}

function showAuthModal() {
    var modal = document.getElementById("auth-modal");
    if (!modal) {
        // Zero Trust Injection: If the page doesn't have the auth modal, inject it.
        var modalHtml = `
        <div id="auth-modal" class="modal">
            <div class="modal-content" style="max-width: 400px; text-align: center;">
                <h2 style="margin-bottom: 20px;">系统鉴权</h2>
                <p style="margin-bottom: 20px; color: var(--text-secondary); font-size: 14px;">请输入管理员 Token 以继续操作</p>
                <input type="password" id="auth-token-input" placeholder="输入 Token..." 
                       style="width: 100%; padding: 10px; margin-bottom: 20px; background: var(--bg-level-1); border: 1px solid var(--border-color); color: var(--text-primary); border-radius: var(--radius-sm); outline: none;">
                <button class="btn btn-primary" onclick="submitAuthToken()" style="width: 100%;">验证</button>
            </div>
        </div>`;
        document.body.insertAdjacentHTML('beforeend', modalHtml);
        modal = document.getElementById("auth-modal");
        
        // Add enter key listener for the newly injected input
        document.getElementById("auth-token-input").addEventListener("keypress", function(e) {
            if (e.key === "Enter") submitAuthToken();
        });
    }
    modal.classList.add("show");
}

function hideAuthModal() {
    var modal = document.getElementById("auth-modal");
    if (modal) modal.classList.remove("show");
}

async function submitAuthToken() {
    var token = document.getElementById("auth-token-input").value.trim();
    if (token) {
        try {
            const res = await fetch(API.ADMIN + '/login', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ token })
            });

            if (res.ok) {
                authToken = token;
                localStorage.setItem("admin_token", token);
                authVerified = true; // Mark as manually verified
                var loginData = await res.json();
                authTokenType = loginData.token_type || "read";
                localStorage.setItem("admin_token_type", authTokenType);
                hideAuthModal();
                if (Object.keys(locationNames).length === 0) {
                    await initLocationMapping();
                }
                if (Object.keys(attributeMeta).length === 0) {
                    await initAttributeMeta();
                }
                
                // Safe dispatch based on current page
                var dashboardEl = document.getElementById("dashboard");
                var configListEl = document.getElementById("config-list");
                
                if (dashboardEl && dashboardEl.classList.contains("active") && typeof loadStats === "function") {
                    loadStats();
                } else if (configListEl && typeof loadConfigList === "function") {
                    loadConfigList();
                } else if (typeof loadActiveHistoryTab === "function") {
                    loadActiveHistoryTab();
                } else if (typeof loadChronicles === "function") {
                    loadChronicles();
                } else if (typeof loadExperiences === "function") {
                    loadExperiences();
                } else if (typeof loadConfig === "function") {
                    loadConfig();
                }
            } else {
                alert('Token 无效');
            }
        } catch (e) {
            alert('请求失败，请重试');
        }
    }
}

async function logout() {
    try {
        await fetch(API.ADMIN + '/logout', { method: 'POST' });
    } catch (e) {
        console.warn('Logout request failed', e);
    }
    localStorage.removeItem("admin_token");
    localStorage.removeItem("admin_token_type");
    authToken = null;
    authTokenType = null;
    authVerified = false;
    window.location.href = '/admin';
}

function initAuth() {
    if (!authToken) {
        showAuthModal();
    }
}
