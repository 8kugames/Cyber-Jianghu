// Auth Functions
// Note: authToken is declared in utils.js (loaded first)

var authVerified = false; // Set to true after user manually enters token
// Note: authTokenType is declared in utils.js (loaded first)

function getAuthHeaders() {
    return authToken ? { Authorization: "Bearer " + authToken } : {};
}

function handleAuthError(res) {
    if (res.status === 401) {
        // Only show modal if user hasn't manually verified with a token this session
        // Prevents modal from reappearing after successful login while data loads
        if (!authVerified) {
            showAuthModal();
        }
        return true;
    }
    return false;
}

function showAuthModal() {
    document.getElementById("auth-modal").style.display = "flex";
}

function hideAuthModal() {
    document.getElementById("auth-modal").style.display = "none";
}

async function submitAuthToken() {
    var token = document.getElementById("auth-token-input").value.trim();
    if (token) {
        try {
            const res = await fetch('/api/admin/login', {
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
                if (document.getElementById("dashboard").classList.contains("active")) {
                    loadStats();
                } else {
                    loadConfigList();
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
        await fetch('/api/admin/logout', { method: 'POST' });
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
