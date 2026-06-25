use anyhow::{Context, Result};
use axum::http::HeaderMap;
use sqlx::PgPool;
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct AuditRequestContext {
    pub request_id: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

pub struct AuditLogEntry<'a> {
    pub event_type: &'a str,
    pub actor_type: &'a str,
    pub token_type: Option<&'a str>,
    pub resource_type: &'a str,
    pub resource_id: Option<String>,
    pub endpoint: &'a str,
    pub method: &'a str,
    pub result: &'a str,
    pub reason: Option<String>,
    pub payload: serde_json::Value,
    pub request_id: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub before_state: Option<serde_json::Value>,
    pub after_state: Option<serde_json::Value>,
}

pub fn build_audit_request_context(
    headers: &HeaderMap,
    addr: SocketAddr,
) -> AuditRequestContext {
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let ip = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| Some(addr.ip().to_string()));

    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);

    AuditRequestContext {
        request_id,
        ip,
        user_agent,
    }
}

pub async fn insert_audit_log(pool: &PgPool, entry: AuditLogEntry<'_>) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            event_type, actor_type, token_type, resource_type, resource_id,
            endpoint, method, result, reason, payload,
            request_id, ip, user_agent, before_state, after_state
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        "#,
    )
    .bind(entry.event_type)
    .bind(entry.actor_type)
    .bind(entry.token_type)
    .bind(entry.resource_type)
    .bind(entry.resource_id)
    .bind(entry.endpoint)
    .bind(entry.method)
    .bind(entry.result)
    .bind(entry.reason)
    .bind(entry.payload)
    .bind(entry.request_id)
    .bind(entry.ip)
    .bind(entry.user_agent)
    .bind(entry.before_state)
    .bind(entry.after_state)
    .execute(pool)
    .await
    .context("写入 audit_log 失败")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use super::build_audit_request_context;

    #[test]
    fn test_build_audit_request_context_prefers_forwarded_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.10, 10.0.0.1"),
        );
        headers.insert("user-agent", HeaderValue::from_static("CyberTest/1.0"));
        let ctx = build_audit_request_context(
            &headers,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
        );

        assert_eq!(ctx.ip.as_deref(), Some("203.0.113.10"));
        assert_eq!(ctx.user_agent.as_deref(), Some("CyberTest/1.0"));
        assert!(!ctx.request_id.is_empty());
    }
}
