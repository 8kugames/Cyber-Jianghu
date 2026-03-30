// ============================================================================
// Auth Functions
// ============================================================================

function getAuthHeaders() {
    return authToken ? { Authorization: "Bearer " + authToken } : {};
}

function handleAuthError(res) {
    if (res.status === 401) {
        showAuthModal();
        return true;
    }
    return false;
}

function showAuthModal() {
    document.getElementById("auth-modal").style.display = "flex";
}

async function submitAuthToken() {
    var token = document.getElementById("auth-token-input").value.trim();
    if (token) {
        authToken = token;
        localStorage.setItem("admin_token", token);
        document.getElementById("auth-modal").style.display = "none";
        if (Object.keys(locationNames).length === 0) {
            await initLocationMapping();
        }
        if (document.getElementById("dashboard").classList.contains("active")) {
            loadStats();
        } else {
            loadConfigList();
        }
    }
}
