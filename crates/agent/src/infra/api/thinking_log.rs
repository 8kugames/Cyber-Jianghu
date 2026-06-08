use chrono::Local;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tracing_appender::rolling::{RollingFileAppender, Rotation};

static WRITER: OnceLock<Mutex<RollingFileAppender>> = OnceLock::new();

pub fn init_thinking_log(log_dir: &Path) -> std::io::Result<PathBuf> {
    let logs_dir = log_dir.join("logs");
    fs::create_dir_all(&logs_dir)?;

    let appender = RollingFileAppender::new(Rotation::DAILY, &logs_dir, "thinking");
    let _ = WRITER.set(Mutex::new(appender));

    let today = Local::now().format("%Y-%m-%d").to_string();
    Ok(logs_dir.join(format!("thinking-{}.log", today)))
}

fn with_writer<F>(f: F)
where
    F: FnOnce(&mut dyn Write),
{
    let guard = WRITER
        .get()
        .and_then(|w: &Mutex<RollingFileAppender>| w.lock().ok());
    if let Some(mut writer) = guard {
        f(&mut *writer);
    }
}

fn soul_from_stage(stage: &str) -> &str {
    if stage == "ReflectorSoul" {
        "ReflectorSoul"
    } else {
        "ActorSoul"
    }
}

pub fn log_thinking(agent_name: &str, tick_id: i64, content: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    with_writer(|w| {
        let _ = writeln!(
            w,
            "\n[{}] [agent={}] [tick={}] [kind=thinking] [soul=ActorSoul]",
            timestamp, agent_name, tick_id
        );
        let _ = writeln!(w, "{}", content);
        let _ = writeln!(w, "{}", "-".repeat(60));
    });
}

pub fn log_llm(agent_name: &str, tick_id: i64, stage: &str, prompt: &str, response: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    with_writer(|w| {
        let _ = writeln!(
            w,
            "\n[{}] [agent={}] [tick={}] [kind=llm] [soul={}] [stage={}]",
            timestamp,
            agent_name,
            tick_id,
            soul_from_stage(stage),
            stage
        );
        let _ = writeln!(w, "-- Prompt:\n{}", prompt);
        let _ = writeln!(w, "-- Response:\n{}", response);
        let _ = writeln!(w, "{}", "-".repeat(60));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_log_llm_writes_full_prompt_and_structured_tags() {
        let temp_dir = TempDir::new().expect("temp dir");
        init_thinking_log(temp_dir.path()).expect("init thinking log");
        let prompt = "甲".repeat(5001);

        log_llm("裴无咎", 42, "Direct", &prompt, r#"{"ok":true}"#);

        let log_path = fs::read_dir(temp_dir.path().join("logs"))
            .expect("read logs dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("thinking"))
            })
            .expect("thinking log file");
        let content = fs::read_to_string(log_path).expect("read thinking log");
        assert!(content.contains("[agent=裴无咎]"));
        assert!(content.contains("[tick=42]"));
        assert!(content.contains("[kind=llm]"));
        assert!(content.contains("[stage=Direct]"));
        assert!(content.contains(&prompt));
        assert!(!content.contains("[truncated"));
    }
}
