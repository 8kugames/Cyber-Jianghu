//! 启动 sweep `.tmp` 残留测试。

use cyber_jianghu_server::training_export::checkpoint::sweep_tmp_files;

#[tokio::test]
async fn sweep_removes_only_tmp_files() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let dir = temp_dir.path();

    tokio::fs::write(dir.join("run=abc.jsonl.tmp"), "partial")
        .await
        .expect("write tmp jsonl");
    tokio::fs::write(dir.join("run=def.meta.json.tmp"), "partial")
        .await
        .expect("write tmp metadata");
    tokio::fs::write(dir.join("run=good.jsonl"), "complete")
        .await
        .expect("write completed jsonl");
    tokio::fs::write(dir.join("run=good.meta.json"), "{}")
        .await
        .expect("write completed metadata");

    assert_eq!(sweep_tmp_files(dir).await.expect("sweep tmp files"), 2);
    assert!(
        tokio::fs::try_exists(dir.join("run=good.jsonl"))
            .await
            .unwrap()
    );
    assert!(
        tokio::fs::try_exists(dir.join("run=good.meta.json"))
            .await
            .unwrap()
    );
    assert!(
        !tokio::fs::try_exists(dir.join("run=abc.jsonl.tmp"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn sweep_missing_directory_is_empty() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let missing = temp_dir.path().join("missing");
    assert_eq!(
        sweep_tmp_files(&missing).await.expect("sweep missing dir"),
        0
    );
}
