//! server bin 单测（自 main.rs 外移）

use super::admin_static::{render_admin_token_file_content, write_admin_token_file};

#[test]
fn test_render_admin_token_file_content_is_ascii_without_emoji_header() {
    let content = render_admin_token_file_content("自动生成", "read-token", "配置", "write-token");
    assert!(!content.contains("🔐"));
    assert!(content.contains("Cyber-Jianghu 管理员访问凭证"));
}

/// 验证：axum::serve 必须链式调用 `.with_graceful_shutdown(...)`，
/// 否则 SIGTERM/SIGINT 触发时 in-flight HTTP 请求会被截断，
/// DB 写入半完成、Saga 状态不一致（高风险）。
#[test]
fn test_p1_9_axum_serve_uses_graceful_shutdown() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(manifest_dir.join("src/main_support/main_fn.rs"))
        .expect("read main_fn.rs source");
    // 仅扫描生产代码段（`mod tests` 之前），避免测试自身字符串假阳性
    let prod_slice = match source.find("#[cfg(test)]") {
        Some(idx) => &source[..idx],
        None => &source[..],
    };
    let serve_idx = prod_slice
        .find("axum::serve(")
        .expect("must call axum::serve in production code");
    let tail = &prod_slice[serve_idx..];
    let next_400 = tail.get(..400).unwrap_or(tail);
    assert!(
        next_400.contains(".with_graceful_shutdown("),
        "axum::serve 必须链式调用 .with_graceful_shutdown(...)，\n\
         否则 SIGTERM/SIGINT 触发时 in-flight HTTP 请求会被截断，\n\
         DB 写入半完成、Saga 状态不一致。\n\
         当前 axum::serve 后续 400 字符片段：\n{next_400}"
    );
}

/// 验证：tracing subscriber 必须消费 `RUST_LOG` 环境变量，
/// 而不是硬编码 `Level::INFO`。同时 `.env` 加载必须在 subscriber init 之前，
/// 否则 `.env` 里的 `RUST_LOG=debug` 不会生效。
#[test]
fn test_p1_f2_tracing_uses_env_filter_and_dotenv_first() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(manifest_dir.join("src/main_support/main_fn.rs"))
        .expect("read main_fn.rs source");
    let prod = match source.find("#[cfg(test)]") {
        Some(idx) => &source[..idx],
        None => &source[..],
    };

    // 1. 必须使用 EnvFilter::try_from_default_env
    assert!(
        prod.contains("EnvFilter::try_from_default_env"),
        "tracing subscriber 必须消费 RUST_LOG；\
         用 `EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(\"info\"))` 替代硬编码 `Level::INFO`"
    );
    // 2. 不应在生产路径上继续用 FmtSubscriber::builder().with_max_level(Level::INFO) 硬编码
    assert!(
        !prod.contains(".with_max_level(Level::INFO)"),
        "禁止继续用 `.with_max_level(Level::INFO)` 硬编码日志级别"
    );

    // 3. dotenv 必须在 tracing subscriber 初始化之前
    //    新代码用 `.init()`，旧代码用 `set_global_default`，都接受。
    let dotenv_idx = prod
        .find("dotenv::dotenv()")
        .expect("must call dotenv::dotenv()");
    let init_idx = prod
        .find("tracing_subscriber::fmt()")
        .or_else(|| prod.find("FmtSubscriber::builder"))
        .or_else(|| prod.find("set_global_default"))
        .expect("must initialize tracing subscriber");
    assert!(
        dotenv_idx < init_idx,
        "dotenv::dotenv() 必须在 tracing subscriber 初始化之前调用，\
         否则 .env 里的 RUST_LOG 不会生效。\
         dotenv_idx={dotenv_idx}, init_idx={init_idx}"
    );
}

/// 验证：workspace Cargo.toml 的 tracing-subscriber 必须启用 `json` feature，
/// 否则代码里写 `.json()` 编译失败。
#[test]
fn test_p1_f2_workspace_tracing_subscriber_enables_json_feature() {
    // workspace root is ../../ from crates/server
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_toml = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/server should be inside a workspace")
        .join("Cargo.toml");
    let content = std::fs::read_to_string(&workspace_toml).expect("read workspace Cargo.toml");
    // 找 tracing-subscriber = {...} 这行
    let ts_line = content
        .lines()
        .find(|l| l.contains("tracing-subscriber") && l.contains("="))
        .expect("workspace must declare tracing-subscriber");
    assert!(
        ts_line.contains("\"json\""),
        "workspace Cargo.toml 的 tracing-subscriber 必须启用 \"json\" feature（用于 JSON 化日志），当前行：\n{ts_line}"
    );
}

#[cfg(unix)]
#[test]
fn test_write_admin_token_file_enforces_0600_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("create temp dir");
    let file_path = dir.path().join("admin_tokens.txt");

    write_admin_token_file(&file_path, "自动生成", "read-token", "配置", "write-token")
        .expect("write admin token file");

    let mode = std::fs::metadata(&file_path)
        .expect("read metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}
