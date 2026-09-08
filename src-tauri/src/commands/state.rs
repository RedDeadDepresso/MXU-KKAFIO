//! 状态查询命令

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use super::types::{AppState, LogEntryDto};
use super::utils::get_app_data_dir;

/// 由前端调用，将已格式化的日志行输出到 stdout
#[tauri::command]
pub fn log_to_stdout(message: String) {
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S.%3f");
    for line in message.lines() {
        println!("[{timestamp}] {line}");
    }
}

/// Resolve the on-disk path used to persist a given instance's runtime log
/// (kkafio subprocess output, MAA callback lines, etc.). Lives alongside the
/// app's own `.log` files in `debug/`, so `export_logs`'s existing directory
/// scan picks it up automatically.
pub(crate) fn runtime_log_path(instance_id: &str) -> Result<PathBuf, String> {
    let debug_dir = get_app_data_dir()?.join("debug");
    std::fs::create_dir_all(&debug_dir)
        .map_err(|e| format!("Cannot create debug dir: {}", e))?;

    let safe_id: String = instance_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let safe_id = if safe_id.is_empty() { "default".to_string() } else { safe_id };

    Ok(debug_dir.join(format!("runtime-{}.log", safe_id)))
}

/// Best-effort append of a single log entry to its instance's runtime log
/// file. Failures are logged but never propagated — persisting to disk must
/// not break the (in-memory) log buffer that actually drives the UI.
pub(crate) fn append_runtime_log_line(instance_id: &str, entry: &LogEntryDto) {
    let path = match runtime_log_path(instance_id) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[kkafio] cannot resolve runtime log path: {}", e);
            return;
        }
    };

    let line = format!("{} [{}] {}\n", entry.timestamp, entry.log_type.to_uppercase(), entry.message);

    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if let Err(e) = file.write_all(line.as_bytes()) {
                log::warn!("[kkafio] failed to write runtime log '{}': {}", path.display(), e);
            }
        }
        Err(e) => {
            log::warn!("[kkafio] failed to open runtime log '{}': {}", path.display(), e);
        }
    }
}

/// 前端推送一条运行日志到后端缓冲区
#[tauri::command]
pub fn push_log(
    state: State<Arc<AppState>>,
    instance_id: String,
    entry: LogEntryDto,
) -> Result<(), String> {
    append_runtime_log_line(&instance_id, &entry);

    let mut buffer = state.log_buffer.lock().map_err(|e| e.to_string())?;
    buffer.push(&instance_id, entry);
    Ok(())
}

/// 获取所有实例的运行日志（用于页面刷新后恢复）
#[tauri::command]
pub fn get_all_logs(
    state: State<Arc<AppState>>,
) -> Result<HashMap<String, Vec<LogEntryDto>>, String> {
    let buffer = state.log_buffer.lock().map_err(|e| e.to_string())?;
    Ok(buffer
        .get_all()
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
        .collect())
}

/// 清空指定实例的运行日志
#[tauri::command]
pub fn clear_instance_logs(
    state: State<Arc<AppState>>,
    instance_id: String,
) -> Result<(), String> {
    {
        let mut buffer = state.log_buffer.lock().map_err(|e| e.to_string())?;
        buffer.clear_instance(&instance_id);
    }

    // Best-effort: also drop the on-disk copy so a cleared instance doesn't
    // keep resurfacing stale output in exported log archives.
    if let Ok(path) = runtime_log_path(&instance_id) {
        let _ = std::fs::remove_file(path);
    }

    Ok(())
}
