//! KKAFIO process management commands
//!
//! Spawns kkafio_cli.exe (or kkafio_cli.py via `uv run`) as a child process, pipes its
//! stdout/stderr line-by-line to the frontend via the `kkafio-output` Tauri
//! event, and exposes start/stop commands.

use log::{info, warn};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::Serialize;
use tauri::{Emitter, State};

// ============================================================================
// Shared child-process state
// ============================================================================

#[derive(Default)]
pub struct KkafioState {
    pub child: Mutex<Option<Child>>,
}

// ============================================================================
// Event payload
// ============================================================================

/// Payload emitted on the `kkafio-output` event for every line produced by the
/// CLI.  `stream` is either `"stdout"` or `"stderr"`.
#[derive(Clone, Serialize)]
pub struct KkafioOutputEvent {
    pub stream: String,
    pub line: String,
}

// ============================================================================
// Helpers
// ============================================================================

fn emit_line(app: &tauri::AppHandle, stream: &str, line: &str) {
    let payload = KkafioOutputEvent {
        stream: stream.to_string(),
        line: strip_ansi(line),
    };
    if let Err(e) = app.emit("kkafio-output", payload) {
        warn!("[kkafio] failed to emit kkafio-output: {}", e);
    }
}

/// Remove ANSI escape sequences (e.g. `\x1b[94m`, `\x1b[0m`) from a line.
/// Operates on the string as a char iterator so multi-byte UTF-8 characters
/// are never corrupted.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        // ESC [ ... m — the only form the Python logger produces
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            // skip digits and semicolons until the final command letter
            loop {
                match chars.peek() {
                    Some(&d) if d.is_ascii_digit() || d == ';' => { chars.next(); }
                    Some(_) => { chars.next(); break; } // consume command letter
                    None    => break,
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Resolve which executable + args to use.
/// Priority: kkafio_cli.exe > kkafio_cli.py (via `uv run`).
/// Returns (program, args) or an error string.
fn resolve_cli(cwd: &str) -> Result<(String, Vec<String>), String> {
    let exe = Path::new(cwd).join("kkafio_cli.exe");
    if exe.exists() {
        return Ok((exe.to_string_lossy().into_owned(), vec!["run".into()]));
    }

    let script = Path::new(cwd).join("kkafio_cli.py");
    if script.exists() {
        // Use `uv run` so the correct virtualenv and all dependencies are
        // active regardless of what Python is on PATH.
        // -q suppresses uv's own output so only the CLI's stdout/stderr
        // reaches the log panel.
        return Ok((
            "uv".to_string(),
            vec![
                "run".into(),
                "--quiet".into(),
                "python".into(),
                "-u".into(),
                script.to_string_lossy().into_owned(),
                "run".into(),
            ],
        ));
    }

    Err(format!(
        "Neither kkafio_cli.exe nor kkafio_cli.py found in {}",
        cwd
    ))
}

// ============================================================================
// Tauri commands
// ============================================================================

/// Start the KKAFIO CLI process.
///
/// `cwd` must be the directory that contains `kkafio_cli.exe` (or
/// `kkafio_cli.py`). The CLI reads its `config.json` from `%APPDATA%/KKAFIO/`
/// automatically; MXU just launches it and streams the output.
#[tauri::command]
pub fn kkafio_start(
    app: tauri::AppHandle,
    state: State<'_, Arc<KkafioState>>,
    cwd: String,
    instance_index: Option<usize>,
) -> Result<(), String> {
    // Kill any previously running instance first.
    {
        let mut guard = state.child.lock().map_err(|e| e.to_string())?;
        if let Some(mut old) = guard.take() {
            info!("[kkafio] killing previous child process");
            let _ = old.kill();
            let _ = old.wait();
        }
    }

    let (program, mut args) = resolve_cli(&cwd)?;

    // Insert --instance N as a global CLI flag *before* the "run" subcommand.
    // For the .exe case args = ["run"], for the .py case args = ["-u", "<script>", "run"].
    // In both cases "run" is the last element, so we insert before the final item.
    if let Some(idx) = instance_index {
        let run_pos = args.len().saturating_sub(1);
        args.insert(run_pos, idx.to_string());
        args.insert(run_pos, "--instance".to_string());
    }

    info!("[kkafio] spawning: {} {:?} in {}", program, args, cwd);

    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = Command::new(&program);
        c.creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new(&program);

    cmd.args(&args)
        .current_dir(&cwd)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn KKAFIO CLI: {}", e))?;

    // ── stdout reader thread ─────────────────────────────────────────────────
    if let Some(stdout) = child.stdout.take() {
        let app_h = app.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => emit_line(&app_h, "stdout", &l),
                    Err(_) => break,
                }
            }
            // Process ended — emit a synthetic sentinel so the frontend knows.
            emit_line(&app_h, "stdout", "--------------------------------------------------------------------");
            info!("[kkafio] stdout reader thread finished");
        });
    }

    // ── stderr reader thread ─────────────────────────────────────────────────
    if let Some(stderr) = child.stderr.take() {
        let app_h = app.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) if !l.trim().is_empty() => emit_line(&app_h, "stderr", &l),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            info!("[kkafio] stderr reader thread finished");
        });
    }

    // Store child so we can kill it later.
    {
        let mut guard = state.child.lock().map_err(|e| e.to_string())?;
        *guard = Some(child);
    }

    Ok(())
}

/// Stop the running KKAFIO CLI process (if any).
#[tauri::command]
pub fn kkafio_stop(state: State<'_, Arc<KkafioState>>) -> Result<(), String> {
    let mut guard = state.child.lock().map_err(|e| e.to_string())?;
    if let Some(mut child) = guard.take() {
        info!("[kkafio] stopping child process");
        let _ = child.kill();
        // Reap in a background thread to avoid blocking the command handler.
        thread::spawn(move || {
            let _ = child.wait();
            info!("[kkafio] child process reaped");
        });
    } else {
        info!("[kkafio] kkafio_stop called but no process is running");
    }
    Ok(())
}

/// Returns true if the CLI child process is currently alive.
#[tauri::command]
pub fn kkafio_is_running(state: State<'_, Arc<KkafioState>>) -> bool {
    let mut guard = match state.child.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if let Some(child) = guard.as_mut() {
        match child.try_wait() {
            Ok(None) => true,       // still running
            Ok(Some(_)) => {
                // Process exited naturally — clean up the slot.
                *guard = None;
                false
            }
            Err(_) => false,
        }
    } else {
        false
    }
}

// ============================================================================
// Group-chara export (copy) command
// ============================================================================

/// Result returned by kkafio_group_chara_export.
#[derive(serde::Serialize)]
pub struct GroupCharaExportResult {
    /// The prompt + JSON text ready to be placed on the clipboard.
    pub text: String,
    /// Non-empty when an error occurred; text will be empty.
    pub error: String,
}

/// Run  `kkafio_cli group-chara --export --input <folder>`  synchronously
/// and return stdout as the clipboard text.
///
/// The caller supplies the user's custom prompt; this command splices it in
/// front of the JSON that the CLI prints, matching the Python worker logic.
#[tauri::command]
pub async fn kkafio_group_chara_export(
    cwd: String,
    folder: String,
    prompt: String,
) -> GroupCharaExportResult {
    use std::process::Command;

    let (program, mut base_args) = match resolve_cli(&cwd) {
        Ok(pair) => pair,
        Err(e) => {
            return GroupCharaExportResult { text: String::new(), error: e };
        }
    };

    // Replace the trailing "run" with group-chara --export --input <folder>
    // For .exe: args = ["run"]  →  pop → push subcommand args
    // For .py:  args = ["-u", "<script>", "run"]  →  pop → push subcommand args
    base_args.pop(); // remove "run"
    base_args.extend([
        "group-chara".to_string(),
        "--export".to_string(),
        "--input".to_string(),
        folder,
    ]);

    log::info!("[kkafio] group-chara export: {} {:?}", program, base_args);

    #[cfg(windows)]
    let output = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new(&program)
            .args(&base_args)
            .current_dir(&cwd)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    };

    #[cfg(not(windows))]
    let output = Command::new(&program)
        .args(&base_args)
        .current_dir(&cwd)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return GroupCharaExportResult {
                text: String::new(),
                error: format!("Failed to run CLI: {}", e),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let error = if !stderr.is_empty() { stderr } else { stdout };
        let error = if error.is_empty() {
            format!("CLI exited with code {:?}", output.status.code())
        } else {
            error
        };
        return GroupCharaExportResult { text: String::new(), error };
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return GroupCharaExportResult {
            text: String::new(),
            error: "No character cards found in the selected folder.".to_string(),
        };
    }

    // Splice user prompt in front of the JSON block
    let json_start = stdout.find('{').unwrap_or(0);
    let json_only = &stdout[json_start..];
    let full_text = format!("{}\n{}", prompt.trim_end(), json_only);

    GroupCharaExportResult { text: full_text, error: String::new() }
}

// ============================================================================
// Rename-chara export command
// ============================================================================

/// Result returned by kkafio_rename_chara_export.
#[derive(serde::Serialize)]
pub struct RenameCharaExportResult {
    /// The prompt + JSON text ready to be placed on the clipboard.
    /// Empty when there is nothing to translate (all chars already cached).
    pub text: String,
    /// Non-empty when an error occurred.
    pub error: String,
}

/// Run `kkafio_cli rename-chara --export --input <folder>` synchronously
/// and return the user prompt spliced in front of the JSON (same pattern as
/// kkafio_group_chara_export).
#[tauri::command]
pub async fn kkafio_rename_chara_export(
    cwd: String,
    folder: String,
    prompt: String,
) -> RenameCharaExportResult {
    use std::process::Command;

    let (program, mut base_args) = match resolve_cli(&cwd) {
        Ok(pair) => pair,
        Err(e) => {
            return RenameCharaExportResult { text: String::new(), error: e };
        }
    };

    base_args.pop(); // remove "run"
    base_args.extend([
        "rename-chara".to_string(),
        "--export".to_string(),
        "--input".to_string(),
        folder,
    ]);

    log::info!("[kkafio] rename-chara export: {} {:?}", program, base_args);

    #[cfg(windows)]
    let output = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new(&program)
            .args(&base_args)
            .current_dir(&cwd)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    };

    #[cfg(not(windows))]
    let output = Command::new(&program)
        .args(&base_args)
        .current_dir(&cwd)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return RenameCharaExportResult {
                text: String::new(),
                error: format!("Failed to run CLI: {}", e),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let error  = if !stderr.is_empty() { stderr } else { stdout };
        let error  = if error.is_empty() {
            format!("CLI exited with code {:?}", output.status.code())
        } else { error };
        return RenameCharaExportResult { text: String::new(), error };
    }

    let stdout = strip_ansi(
        &String::from_utf8_lossy(&output.stdout).trim().to_string()
    );

    if stdout.is_empty() {
        // All characters already cached — nothing to send
        return RenameCharaExportResult { text: String::new(), error: String::new() };
    }

    // Splice user prompt in front of the JSON block (same pattern as group_chara_export)
    let json_start = stdout.find('{').unwrap_or(0);
    let json_only  = &stdout[json_start..];
    let full_text  = format!("{}\n{}", prompt.trim_end(), json_only);
    RenameCharaExportResult { text: full_text, error: String::new() }
}

/// Result of a trash operation.
#[derive(serde::Serialize)]
pub struct TrashResult {
    pub ok: bool,
    pub error: String,
}

/// Send a file or directory (and optionally the directory itself) to the
/// Recycle Bin / Trash using the `trash` crate.
///
/// - `path`          — absolute path to a file or folder.
/// - `delete_folder` — when true the folder at `path` itself is trashed;
///                     when false only its *contents* are trashed.
#[tauri::command]
pub fn kkafio_trash(path: String, delete_folder: bool) -> TrashResult {
    use std::fs;

    let target = std::path::Path::new(&path);

    if !target.exists() {
        return TrashResult {
            ok: false,
            error: format!("Path does not exist: {}", path),
        };
    }

    // Helper: trash a single item, return error string on failure.
    let trash_one = |p: &std::path::Path| -> Result<(), String> {
        trash::delete(p).map_err(|e| format!("{}: {}", p.display(), e))
    };

    if delete_folder {
        // Trash the whole folder/file in one shot.
        if let Err(e) = trash_one(target) {
            return TrashResult { ok: false, error: e };
        }
    } else {
        // Trash contents only.
        if target.is_dir() {
            let entries = match fs::read_dir(target) {
                Ok(e) => e,
                Err(e) => return TrashResult { ok: false, error: e.to_string() },
            };
            let mut errors: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                if let Err(e) = trash_one(&entry.path()) {
                    errors.push(e);
                }
            }
            if !errors.is_empty() {
                return TrashResult {
                    ok: false,
                    error: errors.join("\n"),
                };
            }
        } else {
            // It's a file — trash it directly.
            if let Err(e) = trash_one(target) {
                return TrashResult { ok: false, error: e };
            }
        }
    }

    TrashResult { ok: true, error: String::new() }
}

// ============================================================================
// Run Game command
// ============================================================================

/// Result of kkafio_run_game.
#[derive(serde::Serialize)]
pub struct RunGameResult {
    pub ok: bool,
    /// The exe that was launched, or empty string on failure.
    pub exe: String,
    pub error: String,
}

/// Try to launch Koikatsu from `game_path`.
/// `game_type` selects the expected exe name:
///   "KoikatsuParty"    → "Koikatsu Party.exe"
///   "Koikatsu"         → "Koikatsu.exe"
///   "KoikatsuSunshine" → "KoikatsuSunshine.exe" | "Koikatsu Sunshine.exe"
/// Falls back to scanning all known exe names if the primary is not found.
#[tauri::command]
pub fn kkafio_run_game(game_path: String, game_type: Option<String>) -> RunGameResult {
    use std::path::Path;
    use std::process::Command;

    let base = Path::new(&game_path);

    // Build candidate list — primary exe first based on game_type, then fallbacks
    let mut candidates: Vec<&str> = Vec::new();
    match game_type.as_deref().unwrap_or("KoikatsuParty") {
        "Koikatsu"         => candidates.extend(["Koikatsu.exe", "Koikatsu Party.exe", "KoikatsuSunshine.exe", "Koikatsu Sunshine.exe"]),
        "KoikatsuSunshine" => candidates.extend(["KoikatsuSunshine.exe", "Koikatsu Sunshine.exe", "Koikatsu Party.exe", "Koikatsu.exe"]),
        _                  => candidates.extend(["Koikatsu Party.exe", "Koikatsu.exe", "KoikatsuSunshine.exe", "Koikatsu Sunshine.exe"]),
    }

    let exe_path = candidates.iter().map(|name| base.join(name)).find(|p| p.exists());

    let exe = match exe_path {
        Some(p) => p,
        None => {
            return RunGameResult {
                ok: false,
                exe: String::new(),
                error: format!("No Koikatsu executable found in '{}'", game_path),
            };
        }
    };

    let exe_str = exe.to_string_lossy().into_owned();
    let cwd = exe.parent().unwrap_or(base);

    log::info!("[kkafio] launching game: {}", exe_str);

    #[cfg(windows)]
    let result = {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP so it survives MXU exit
        const DETACHED_PROCESS: u32       = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        Command::new(&exe)
            .current_dir(cwd)
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
    };

    #[cfg(not(windows))]
    let result = Command::new(&exe).current_dir(cwd).spawn();

    match result {
        Ok(_) => RunGameResult { ok: true, exe: exe_str, error: String::new() },
        Err(e) => RunGameResult { ok: false, exe: exe_str, error: e.to_string() },
    }
}