use axum::{
    body::Body,
    extract::Request,
    extract::State,
    http::{header::SET_COOKIE, Response, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Json},
};
use std::sync::Arc;

use crate::state::AppState;

const SESSION_COOKIE_NAME: &str = "cyber_admin_session";
const SESSION_EXPIRY_HOURS: i64 = 24;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SessionData {
    token_type: String,
    expiry: i64,
}

#[derive(Debug, serde::Deserialize)]
pub struct LoginRequest {
    pub token: String,
}

fn sign_data(data: &str, secret: &str) -> String {
    use hmac::Mac;
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(data.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn create_session_token(data: &SessionData, secret: &str) -> String {
    let data_str = serde_json::to_string(data).expect("SessionData is serializable");
    let signature = sign_data(&data_str, secret);
    format!("{}.{}", data_str, signature)
}

fn verify_session_token(token: &str, secret: &str) -> Option<SessionData> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 2 {
        return None;
    }

    let data_str = parts[0];
    let provided_sig = parts[1];
    let expected_sig = sign_data(data_str, secret);

    if !constant_time_eq(expected_sig.as_bytes(), provided_sig.as_bytes()) {
        return None;
    }

    let data: SessionData = serde_json::from_str(data_str).ok()?;

    let now = chrono::Utc::now().timestamp();
    if data.expiry < now {
        return None;
    }

    Some(data)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

fn build_session_cookie(session_token: &str) -> String {
    format!(
        "{}={}; Path=/admin; Max-Age={}; SameSite=Strict; HttpOnly",
        SESSION_COOKIE_NAME,
        session_token,
        SESSION_EXPIRY_HOURS * 3600
    )
}

fn build_clear_cookie() -> String {
    format!(
        "{}={}; Path=/; Max-Age=0; SameSite=Strict; HttpOnly",
        SESSION_COOKIE_NAME,
        ""
    )
}

fn extract_session_cookie(req: &Request) -> Option<String> {
    req.headers()
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|cookie: &str| {
            let (name, value) = cookie.trim().split_once('=')?;
            if name == SESSION_COOKIE_NAME {
                Some(value.to_string())
            } else {
                None
            }
        })
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Result<Response<Body>, StatusCode> {
    let token = body.token.trim();

    let token_type = if token == state.admin_write_token {
        "write"
    } else if token == state.admin_read_token {
        "read"
    } else {
        tracing::warn!("Admin login failed: invalid token provided");
        return Err(StatusCode::UNAUTHORIZED);
    };

    let session_data = SessionData {
        token_type: token_type.to_string(),
        expiry: chrono::Utc::now().timestamp() + (SESSION_EXPIRY_HOURS * 3600),
    };

    let session_token = create_session_token(&session_data, &state.session_secret);
    let cookie = build_session_cookie(&session_token);

    tracing::info!(
        "Admin login successful: token_type={}, expiry_hours={}",
        token_type,
        SESSION_EXPIRY_HOURS
    );

    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(SET_COOKIE, cookie.parse().unwrap());
    Ok(response)
}

pub async fn logout() -> impl IntoResponse {
    let cookie = build_clear_cookie();
    tracing::info!("Admin logout: session cleared");

    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(SET_COOKIE, cookie.parse().unwrap());
    response
}

pub async fn check_session(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> impl IntoResponse {
    if let Some(token) = extract_session_cookie(&req)
        && let Some(session) = verify_session_token(&token, &state.session_secret)
    {
        return Json(serde_json::json!({
            "authenticated": true,
            "token_type": session.token_type
        })).into_response();
    }

    Json(serde_json::json!({
        "authenticated": false
    })).into_response()
}

pub async fn admin_cookie_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response<Body>, StatusCode> {
    let path = req.uri().path();
    if path == "/api/admin/login" || path == "/api/admin/logout" || path == "/api/admin/session" {
        return Ok(next.run(req).await);
    }

    if let Some(token) = extract_session_cookie(&req)
        && let Some(_session) = verify_session_token(&token, &state.session_secret)
    {
        return Ok(next.run(req).await);
    }

    tracing::warn!("Admin access denied: no valid session cookie for path={}", path);
    Err(StatusCode::UNAUTHORIZED)
}

// ============================================================================
// Login Page Handler (for when accessing /admin without session)
// ============================================================================

/// Returns a simple login page HTML
pub async fn login_page() -> Html<&'static str> {
    Html(r#"<!doctype html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>登录 | Cyber-Jianghu Admin</title>
    <style>
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            color: #fff;
        }
        .login-box {
            background: rgba(255,255,255,0.1);
            backdrop-filter: blur(10px);
            padding: 40px;
            border-radius: 12px;
            box-shadow: 0 8px 32px rgba(0,0,0,0.3);
            width: 320px;
        }
        h1 { font-size: 24px; margin-bottom: 8px; text-align: center; }
        .subtitle { color: #aaa; font-size: 14px; margin-bottom: 30px; text-align: center; }
        label { display: block; margin-bottom: 8px; font-size: 14px; color: #ccc; }
        input {
            width: 100%;
            padding: 12px;
            border: 1px solid rgba(255,255,255,0.2);
            border-radius: 6px;
            background: rgba(255,255,255,0.1);
            color: #fff;
            font-size: 14px;
            margin-bottom: 20px;
        }
        input::placeholder { color: #888; }
        button {
            width: 100%;
            padding: 12px;
            border: none;
            border-radius: 6px;
            background: #4a9eff;
            color: #fff;
            font-size: 16px;
            cursor: pointer;
            transition: background 0.2s;
        }
        button:hover { background: #3a8eef; }
        .error { color: #ff6b6b; font-size: 13px; margin-bottom: 15px; text-align: center; display: none; }
    </style>
</head>
<body>
    <div class="login-box">
        <h1>天道</h1>
        <p class="subtitle">Cyber-Jianghu Admin</p>
        <div class="error" id="error"></div>
        <form id="login-form">
            <label for="token">管理 Token</label>
            <input type="text" id="token" name="token" placeholder="输入 Token..." autocomplete="off" />
            <button type="submit">登录</button>
        </form>
    </div>
    <script>
        const form = document.getElementById('login-form');
        const errorEl = document.getElementById('error');

        form.addEventListener('submit', async (e) => {
            e.preventDefault();
            errorEl.style.display = 'none';

            const token = document.getElementById('token').value.trim();
            if (!token) {
                errorEl.textContent = '请输入 Token';
                errorEl.style.display = 'block';
                return;
            }

            try {
                const res = await fetch('/api/admin/login', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ token })
                });

                if (res.ok) {
                    window.location.href = '/admin/';
                } else {
                    errorEl.textContent = 'Token 无效';
                    errorEl.style.display = 'block';
                }
            } catch (e) {
                errorEl.textContent = '请求失败，请重试';
                errorEl.style.display = 'block';
            }
        });
    </script>
</body>
</html>"#)
}
