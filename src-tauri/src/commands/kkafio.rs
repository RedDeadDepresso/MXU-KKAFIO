//! KKAFIO process management commands
//!
//! Spawns kkafio_cli.exe (or kkafio_cli.py via its own venv interpreter,
//! falling back to `uv run` if the venv doesn't exist yet) as a child
//! process, pipes its stdout/stderr line-by-line to the frontend via the
//! `kkafio-output` Tauri event, and exposes start/stop commands.

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
        line: line.to_string(),
    };
    if let Err(e) = app.emit("kkafio-output", payload) {
        warn!("[kkafio] failed to emit kkafio-output: {}", e);
    }
}

/// Resolve which executable + args to use.
/// Priority: kkafio_cli.exe > kkafio_cli.py via its own venv interpreter
/// directly > kkafio_cli.py via `uv run` (fallback only).
fn resolve_cli(cwd: &str) -> Result<(String, Vec<String>), String> {
    let exe = Path::new(cwd).join("kkafio_cli.exe");
    if exe.exists() {
        return Ok((exe.to_string_lossy().into_owned(), vec!["run".into()]));
    }

    let script = Path::new(cwd).join("kkafio_cli.py");
    if script.exists() {
        // [FIX-2026-09-14-GRACEFUL-STOP-DEV-MODE] Previously this always
        // ran `uv run --quiet python -u kkafio_cli.py run`, with `uv` as
        // the direct child process (the one Tauri spawns and applies
        // CREATE_NEW_PROCESS_GROUP to). GenerateConsoleCtrlEvent
        // (CTRL_BREAK_EVENT, pid) — used by kkafio_stop() to gracefully
        // stop the CLI — only reaches processes that are members of the
        // *same* console process group as the target PID. Whether uv's own
        // Python child ends up in that same group, or attached to a
        // console at all, is entirely up to uv's internal process-spawning
        // behavior, which is opaque to us here — in practice, the graceful
        // stop signal was not reliably reaching the actual Python process
        // running kkafio_cli.py through this `uv run` indirection, even
        // though the exact same mechanism works correctly for the packaged
        // kkafio_cli.exe (a direct child, no wrapper process in between).
        //
        // Fix: resolve uv's own virtualenv interpreter directly
        // (<cwd>/.venv/Scripts/python.exe on Windows) and spawn *that* as
        // the direct child, mirroring the .exe case exactly — no wrapper
        // process, so the same CREATE_NEW_PROCESS_GROUP +
        // GenerateConsoleCtrlEvent logic that already works for the
        // packaged build now works identically in dev mode.
        //
        // `uv run` is kept as a fallback for the case where the venv
        // hasn't been created yet (e.g. before an initial `uv sync`) —
        // being able to run at all is more useful than failing outright —
        // but graceful stop is not guaranteed to work reliably through
        // that fallback path specifically.
        #[cfg(windows)]
        let venv_python = Path::new(cwd).join(".venv").join("Scripts").join("python.exe");
        #[cfg(not(windows))]
        let venv_python = Path::new(cwd).join(".venv").join("bin").join("python");

        if venv_python.exists() {
            return Ok((
                venv_python.to_string_lossy().into_owned(),
                vec![
                    "-u".into(),
                    script.to_string_lossy().into_owned(),
                    "run".into(),
                ],
            ));
        }

        warn!(
            "[kkafio] venv interpreter not found at {} — falling back to \
             `uv run` (graceful stop via CTRL_BREAK_EVENT may not work \
             reliably through this path; run `uv sync` in {} to fix)",
            venv_python.display(),
            cwd
        );
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
    // Regardless of which branch resolve_cli() took (.exe: ["run"], venv
    // python: ["-u", "<script>", "run"], or the `uv run` fallback:
    // ["run", "--quiet", "python", "-u", "<script>", "run"]), "run" is
    // always the last element, so inserting before the final item is
    // correct in all three cases.
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
        // [FIX-2026-09-14-GRACEFUL-STOP] Required so kkafio_stop() can later
        // target this process (and only this process's group — not MXU's
        // own) with GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid). Without
        // this flag the child would share MXU's own console process group,
        // and a process can only be the target of GenerateConsoleCtrlEvent
        // if its own PID is also a valid process group ID, which requires
        // CREATE_NEW_PROCESS_GROUP at creation time. This does not affect
        // CREATE_NO_WINDOW's existing behavior (the child still gets a
        // hidden console, which GenerateConsoleCtrlEvent also requires —
        // unlike DETACHED_PROCESS, which would remove the console entirely
        // and break this).
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        let mut c = Command::new(&program);
        c.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
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

/// [FIX-2026-09-14-GRACEFUL-STOP] Raw FFI bindings for the Win32 functions
/// this module needs, all from kernel32.dll. Minimal hand-written
/// `extern "system"` declarations are used here instead of pulling in a
/// broader Windows API crate purely for these calls — kernel32 is always
/// implicitly linked on the standard Rust/MSVC Windows target, so this
/// requires no extra linkage configuration.
#[cfg(windows)]
extern "system" {
    fn GenerateConsoleCtrlEvent(dw_ctrl_event: u32, dw_process_group_id: u32) -> i32;
    fn AttachConsole(dw_process_id: u32) -> i32;
    fn FreeConsole() -> i32;
    fn SetConsoleCtrlHandler(handler_routine: *const std::ffi::c_void, add: i32) -> i32;
}

#[cfg(windows)]
const CTRL_BREAK_EVENT: u32 = 1;

/// How long to wait for the CLI process to exit gracefully after sending it
/// CTRL_BREAK_EVENT, before falling back to an unconditional TerminateProcess
/// kill.
const GRACEFUL_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Stop the running KKAFIO CLI process (if any).
///
/// [FIX-2026-09-14-GRACEFUL-STOP]
///
/// Previously this called `child.kill()` immediately, which on Windows is
/// `TerminateProcess()` — an OS-level hard kill that the target process
/// cannot intercept or react to in any way. No Python cleanup code runs at
/// all: no signal handler, no `finally` block, nothing. In particular, the
/// download daemon that kkafio_cli.exe spawns internally (via Python's
/// `multiprocessing.Process`, a separate OS process with its own PID that
/// this Rust code has never tracked or had a handle to) never received any
/// notice that its parent was being stopped. It would simply become
/// orphaned and keep running — including any download it was actively in
/// the middle of — until it separately noticed its IPC connection to the
/// now-dead orchestrator had broken. From the user's perspective, clicking
/// Stop while a download was active did not actually stop that download;
/// it just killed the visible window while the orphaned daemon kept
/// running in the background for some additional time.
///
/// Fix: try a graceful stop first. `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT,
/// pid)` delivers a signal the target process *can* catch (as
/// `signal.SIGBREAK` on Windows Python) — kkafio_cli.py installs a handler
/// for this that raises `KeyboardInterrupt`, which propagates up through its
/// running asyncio task exactly like Ctrl+C would, triggering the existing
/// `finally: await teleget_downloader.shutdown()` cleanup path in
/// tasks/download_missing_mods.py. That, in turn, sends the daemon a proper
/// IPC ShutdownRequest and waits for its ACK — the same graceful daemon
/// shutdown machinery already used everywhere else, instead of leaving it to
/// find out on its own that something went wrong.
///
/// [FIX-2026-09-18-ATTACH-CONSOLE] `GenerateConsoleCtrlEvent` requires the
/// *calling* process to have a valid console context to operate through.
/// Tauri apps (like MXU) run under the `windows` GUI subsystem and have no
/// console of their own by default — calling GenerateConsoleCtrlEvent from
/// such a process silently fails (observed in the wild: the CLI process
/// vanished with zero graceful-shutdown activity in its own log, and the
/// daemon noticed its IPC connection break well under a second later —
/// nowhere near the 8s graceful window, meaning the "no console" failure
/// was falling straight through to the immediate-kill fallback below every
/// time, without the fallback's warning even being visible because... it
/// *was* logged, just easy to miss/not paired with the Python-side log at
/// a glance). The standard, documented workaround is to temporarily
/// `AttachConsole(child_pid)` before sending the event (borrowing the
/// child's own console, which it does have — it was spawned with
/// CREATE_NO_WINDOW, not DETACHED_PROCESS, specifically so it would have
/// one), and `FreeConsole()` immediately after. While attached,
/// `SetConsoleCtrlHandler(NULL, TRUE)` tells this (MXU's own) process to
/// ignore the console event once it's broadcast, since after attaching we
/// briefly share the same console process group as the child and would
/// otherwise receive our own broadcast signal too.
///
/// If the process hasn't exited within GRACEFUL_STOP_TIMEOUT (it may not
/// have any running task to interrupt, or something may have gone wrong),
/// this still falls back to the original `child.kill()` as an unconditional
/// last resort, so Stop is guaranteed to actually terminate the process
/// either way.
#[tauri::command]
pub fn kkafio_stop(state: State<'_, Arc<KkafioState>>) -> Result<(), String> {
    let mut guard = state.child.lock().map_err(|e| e.to_string())?;
    if let Some(mut child) = guard.take() {
        info!("[kkafio] stopping child process");

        #[cfg(windows)]
        let sent_graceful = {
            let pid = child.id();
            // SAFETY: these are plain FFI calls with integer/null-pointer
            // arguments only, following the standard documented sequence
            // for sending a console control event from a console-less
            // process to a child that has its own console.
            unsafe {
                if AttachConsole(pid) == 0 {
                    warn!(
                        "[kkafio] AttachConsole failed (err={}), \
                         falling back to immediate kill",
                        std::io::Error::last_os_error()
                    );
                    false
                } else {
                    // Ignore the event in *this* process once broadcast —
                    // we're only borrowing the child's console to send the
                    // signal, not asking to be affected by it ourselves.
                    SetConsoleCtrlHandler(std::ptr::null(), 1);

                    let ok = GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid);
                    let err = std::io::Error::last_os_error();

                    // Detach again regardless of whether sending succeeded —
                    // we don't want to keep borrowing the child's console.
                    FreeConsole();
                    SetConsoleCtrlHandler(std::ptr::null(), 0);

                    if ok == 0 {
                        warn!(
                            "[kkafio] GenerateConsoleCtrlEvent failed (err={}), \
                             falling back to immediate kill",
                            err
                        );
                        false
                    } else {
                        info!(
                            "[kkafio] sent CTRL_BREAK_EVENT (pid={}), waiting up \
                             to {:?} for graceful exit",
                            pid, GRACEFUL_STOP_TIMEOUT
                        );
                        true
                    }
                }
            }
        };
        #[cfg(not(windows))]
        let sent_graceful = {
            // Best-effort graceful stop on non-Windows targets: SIGTERM,
            // which kkafio_cli.py also handles the same way as SIGBREAK.
            let pid = child.id() as i32;
            let ok = unsafe { libc::kill(pid, libc::SIGTERM) } == 0;
            if ok {
                info!("[kkafio] sent SIGTERM (pid={}), waiting up to {:?} for graceful exit", pid, GRACEFUL_STOP_TIMEOUT);
            }
            ok
        };

        // Reap in a background thread to avoid blocking the command handler.
        // If a graceful signal was sent, poll for natural exit first and
        // only escalate to a hard kill if it doesn't exit in time.
        thread::spawn(move || {
            if sent_graceful {
                let deadline = std::time::Instant::now() + GRACEFUL_STOP_TIMEOUT;
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            info!("[kkafio] child exited gracefully: {:?}", status);
                            return;
                        }
                        Ok(None) => {
                            if std::time::Instant::now() >= deadline {
                                break;
                            }
                            thread::sleep(std::time::Duration::from_millis(200));
                        }
                        Err(e) => {
                            warn!("[kkafio] try_wait failed while polling for graceful exit: {}", e);
                            break;
                        }
                    }
                }
                warn!(
                    "[kkafio] child did not exit within {:?} of graceful stop request, \
                     escalating to TerminateProcess",
                    GRACEFUL_STOP_TIMEOUT
                );
            }

            let _ = child.kill();
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
///   "Koikatsu"         → "Koikatu.exe"
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
        "Koikatsu"         => candidates.extend(["Koikatu.exe", "Koikatsu Party.exe", "KoikatsuSunshine.exe", "Koikatsu Sunshine.exe"]),
        "KoikatsuSunshine" => candidates.extend(["KoikatsuSunshine.exe", "Koikatsu Sunshine.exe", "Koikatsu Party.exe", "Koikatu.exe"]),
        _                  => candidates.extend(["Koikatsu Party.exe", "Koikatu.exe", "KoikatsuSunshine.exe", "Koikatsu Sunshine.exe"]),
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

// ============================================================================
// Run Studio command
// ============================================================================

/// Try to launch CharaStudio from `game_path`.
/// CharaStudio.exe lives directly in the game's install directory for all
/// Koikatsu variants (Koikatsu, Koikatsu Party, Koikatsu Sunshine).
#[tauri::command]
pub fn kkafio_run_studio(game_path: String) -> RunGameResult {
    use std::path::Path;
    use std::process::Command;

    let base = Path::new(&game_path);
    let candidates = ["CharaStudio.exe"];

    let exe_path = candidates.iter().map(|name| base.join(name)).find(|p| p.exists());

    let exe = match exe_path {
        Some(p) => p,
        None => {
            return RunGameResult {
                ok: false,
                exe: String::new(),
                error: format!("No CharaStudio executable found in '{}'", game_path),
            };
        }
    };

    let exe_str = exe.to_string_lossy().into_owned();
    let cwd = exe.parent().unwrap_or(base);

    log::info!("[kkafio] launching studio: {}", exe_str);

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