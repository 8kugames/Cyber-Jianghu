//! updater 模块单测（自 updater.rs 外移，内容未改）

use super::*;

#[test]
fn sha256_file_matches_known_empty_hash() {
    let dir = std::env::temp_dir().join(format!("cyj-upd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("empty");
    std::fs::write(&f, b"").unwrap();
    let d = Updater::sha256_file(&f).unwrap();
    assert_eq!(
        d,
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn release_info_parse_and_pick_asset() {
    let json = serde_json::json!({
        "tag_name": "v0.1.345",
        "published_at": "2026-09-15T06:38:16Z",
        "assets": [
            {
                "name": "cyber-jianghu-agent-linux-arm64",
                "size": 17474416,
                "digest": "sha256:1388455bfa7eca7d8eaceca0b738b4c95f33c343679910845a8b9e351ac6f88e",
                "browser_download_url": "https://example.com/a"
            },
            {
                "name": "cyber-jianghu-agent-linux-x86_64",
                "size": 20037128,
                "digest": "sha256:586162703a30560b0131e20eca1fbb8b3ecce3bf5eb7d7f7dd50bab1f20deabb",
                "browser_download_url": "https://example.com/b"
            },
            {
                "name": "unrelated-asset.txt",
                "size": 10,
                "browser_download_url": "https://example.com/c"
            }
        ]
    });
    let release: ReleaseInfo = serde_json::from_value(json).unwrap();
    assert_eq!(release.tag_name, "v0.1.345");
    // 无 digest 字段的资产反序列化为 None
    assert!(release.assets[2].digest.is_none());

    // pick_asset 按当前平台资产名精确匹配，跳过无关资产
    let picked = Updater::pick_asset(&release);
    if Updater::platform_asset_name() == Some("cyber-jianghu-agent-linux-x86_64") {
        assert_eq!(picked.unwrap().name, "cyber-jianghu-agent-linux-x86_64");
    } else if Updater::platform_asset_name() == Some("cyber-jianghu-agent-linux-arm64") {
        assert_eq!(picked.unwrap().name, "cyber-jianghu-agent-linux-arm64");
    } else {
        assert!(picked.is_none());
    }
}

#[test]
fn platform_asset_matches_ci_naming() {
    let name = Updater::platform_asset_name();
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    assert_eq!(name, Some("cyber-jianghu-agent-linux-x86_64"));
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    assert_eq!(name, Some("cyber-jianghu-agent-linux-arm64"));
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(name, Some("cyber-jianghu-agent-macos-arm64"));
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    assert_eq!(name, Some("cyber-jianghu-agent-windows-x86_64.exe"));
}

#[test]
fn dev_build_detection() {
    assert!(Updater::is_dev_build(Path::new(
        "/home/u/proj/target/release/cyber-jianghu-agent"
    )));
    assert!(Updater::is_dev_build(Path::new(
        "/home/u/proj/target/debug/cyber-jianghu-agent"
    )));
    assert!(Updater::is_dev_build(Path::new(
        "/home/u/proj/target/x86_64-unknown-linux-musl/release/cyber-jianghu-agent"
    )));
    assert!(!Updater::is_dev_build(Path::new(
        "/usr/local/bin/cyber-jianghu-agent"
    )));
    assert!(!Updater::is_dev_build(Path::new(
        "/Users/u/bin/cyber-jianghu-agent"
    )));
}

#[test]
fn update_state_roundtrip() {
    let st = UpdateState {
        last_check_unix: Some(1718448000),
        installed_tag: Some("v0.1.345".to_string()),
        latest: Some(LatestSummary {
            tag_name: "v0.1.345".to_string(),
            published_at: None,
            asset_name: "cyber-jianghu-agent-linux-x86_64".to_string(),
            asset_digest: Some("sha256:abc".to_string()),
            asset_size: 42,
        }),
        ..UpdateState::default()
    };
    let json = serde_json::to_string(&st).unwrap();
    let back: UpdateState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.installed_tag.as_deref(), Some("v0.1.345"));
    assert_eq!(back.latest.unwrap().asset_size, 42);
}

#[cfg(unix)]
#[test]
fn install_replaces_binary_with_exec_bit() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("cyj-inst-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("agent");
    let downloaded = dir.join("agent.download");
    std::fs::write(&exe, b"old-binary").unwrap();
    std::fs::write(&downloaded, b"new-binary").unwrap();

    Updater::install(&downloaded, &exe).unwrap();

    assert_eq!(std::fs::read(&exe).unwrap(), b"new-binary");
    assert!(!downloaded.exists(), "临时文件应已被 rename 消费");
    let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0, "安装后必须保留执行位");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hard_disable_env_off_values() {
    // nextest 按进程隔离测试，env 变更安全；2024 edition 需 unsafe
    unsafe { std::env::set_var(ENV_SELF_UPDATE_DISABLE, "0") };
    assert!(Updater::hard_disabled());
    unsafe { std::env::set_var(ENV_SELF_UPDATE_DISABLE, "FALSE") };
    assert!(Updater::hard_disabled());
    unsafe { std::env::set_var(ENV_SELF_UPDATE_DISABLE, "1") };
    assert!(!Updater::hard_disabled());
    unsafe { std::env::remove_var(ENV_SELF_UPDATE_DISABLE) };
    assert!(!Updater::hard_disabled());
}

#[test]
fn load_update_config_defaults_without_file() {
    // 指向一个不存在的配置目录，验证回退默认值
    let dir = std::env::temp_dir().join(format!("cyj-nonexist-{}", std::process::id()));
    unsafe { std::env::set_var("CYBER_JIANGHU_CONFIG_DIR", &dir) };
    let cfg = load_update_config();
    unsafe { std::env::remove_var("CYBER_JIANGHU_CONFIG_DIR") };
    assert!(cfg.enabled);
    assert!(cfg.auto_apply);
    assert_eq!(cfg.repo, "8kugames/Cyber-Jianghu");
}
