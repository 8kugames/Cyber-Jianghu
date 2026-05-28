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

pub fn log_thinking(agent_name: &str, tick_id: i64, content: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    with_writer(|w| {
        let _ = writeln!(w, "\n[{}] [{} - Tick {}]", timestamp, agent_name, tick_id);
        let _ = writeln!(w, "{}", content);
        let _ = writeln!(w, "{}", "-".repeat(60));
    });
}

pub fn log_llm(agent_name: &str, tick_id: i64, stage: &str, prompt: &str, response: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    with_writer(|w| {
        let _ = writeln!(
            w,
            "\n[{}] [{} - Tick {}] LLM: {}",
            timestamp, agent_name, tick_id, stage
        );
        let prompt_preview = if prompt.len() > 5000 {
            let mut end = 5000;
            while end > 0 && !prompt.is_char_boundary(end) {
                end -= 1;
            }
            format!(
                "{}...[truncated {} chars]",
                &prompt[..end],
                prompt.len() - end
            )
        } else {
            prompt.to_string()
        };
        let _ = writeln!(w, "-- Prompt:\n{}", prompt_preview);
        let _ = writeln!(w, "-- Response:\n{}", response);
        let _ = writeln!(w, "{}", "-".repeat(60));
    });
}
