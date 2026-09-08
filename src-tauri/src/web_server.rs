//! Web 服务器
//!
//! 基于 axum 提供 HTTP API，供浏览器客户端（本机/局域网）访问。
//! 所有 Maa/截图/设备扫描路由已移除，仅保留配置、日志、心跳和系统端点。

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, OnceLock};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, HeaderMap, Method, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
#[cfg(not(debug_assertions))]
use rust_embed::RustEmbed;
use tower_http::cors::{Any, CorsLayer};

use crate::commands::{
    app_config::AppConfigState,
    state::{append_runtime_log_line, runtime_log_path},
    types::{AppState, LogEntryDto},
    utils::emit_config_changed,
};
use crate::ws_broadcast::WsBroadcast;
use tauri::Manager;

pub const DEFAULT_PORT: u16 = 12701;
const MAX_PORT_ATTEMPTS: u16 = 10;

static ACTUAL_PORT: AtomicU16 = AtomicU16::new(0);

pub fn get_actual_port() -> u16 {
    ACTUAL_PORT.load(Ordering::Relaxed)
}

static LOCAL_LAN_IP: OnceLock<Option<String>> = OnceLock::new();

fn detect_local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("223.5.5.5:80").ok()?;
    let addr = socket.local_addr().ok()?;
    Some(addr.ip().to_string())
}

pub fn get_local_ip() -> Option<&'static str> {
    LOCAL_LAN_IP.get_or_init(detect_local_ip).as_deref()
}

#[cfg(not(debug_assertions))]
#[derive(RustEmbed)]
#[folder = "../dist"]
struct FrontendAssets;

#[cfg(debug_assertions)]
const VITE_DEV_URL: &str = "http://localhost:1420";

#[cfg(debug_assertions)]
async fn serve_vite_proxy(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let url = format!("{}{}", VITE_DEV_URL, path);
    match reqwest::get(&url).await {
        Ok(resp) => {
            let status = axum::http::StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(axum::http::StatusCode::OK);
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let body = resp.bytes().await.unwrap_or_default().to_vec();
            (status, [(header::CONTENT_TYPE, content_type)], body).into_response()
        }
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            b"<html><body><p>Waiting for Vite dev server...</p>\
              <script>setTimeout(()=>location.reload(),2000)</script></body></html>"
                .to_vec(),
        )
            .into_response(),
    }
}

fn mime_from_extension(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" | "jsonc" => "application/json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(not(debug_assertions))]
fn guess_mime(path: &str) -> &'static str {
    mime_from_extension(path.rsplit('.').next().unwrap_or(""))
}

#[cfg(not(debug_assertions))]
async fn serve_embedded(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = FrontendAssets::get(path) {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, guess_mime(path))],
            file.data.into_owned(),
        )
            .into_response();
    }

    if let Some(file) = FrontendAssets::get("index.html") {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            file.data.into_owned(),
        )
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

// ============================================================================
// Shared axum state (no MaaState)
// ============================================================================

#[derive(Clone)]
struct WebState {
    app_config: Arc<AppConfigState>,
    app_state: Arc<AppState>,
    app_handle: tauri::AppHandle,
    ws_broadcast: Arc<WsBroadcast>,
}

// ============================================================================
// start_web_server
// ============================================================================

pub async fn start_web_server(
    app_config: Arc<AppConfigState>,
    app_handle: tauri::AppHandle,
    ws_broadcast: Arc<WsBroadcast>,
    port: u16,
    allow_lan_access: bool,
) {
    // Retrieve the AppState registered in the Tauri app
    let app_state = app_handle
        .try_state::<Arc<AppState>>()
        .map(|s| s.inner().clone())
        .unwrap_or_else(|| Arc::new(AppState::default()));

    let state = WebState {
        app_config,
        app_state,
        app_handle,
        ws_broadcast,
    };

    let api_routes = Router::new()
        // Interface & config
        .route("/interface", get(handle_get_interface))
        .route(
            "/config",
            get(handle_get_config)
                .put(handle_put_config)
                .post(handle_put_config),
        )
        .route("/background-image", get(handle_get_background_image))
        // WebSocket real-time push
        .route("/ws", get(handle_ws_upgrade))
        // Runtime logs (persisted across page refreshes)
        .route("/logs", get(handle_get_all_logs))
        .route(
            "/logs/:id",
            axum::routing::post(handle_push_log).delete(handle_clear_instance_logs),
        )
        // Heartbeat
        .route("/heartbeat", get(handle_heartbeat))
        // System
        .route("/system/is-elevated", get(handle_is_elevated))
        .route(
            "/system/restart-as-admin",
            axum::routing::post(handle_restart_as_admin),
        )
        // Local file proxy for browser clients
        .route("/local-file", get(handle_serve_local_file))
        .with_state(state);

    let mut app: Router = Router::new().nest("/api", api_routes);

    #[cfg(debug_assertions)]
    {
        log::info!(
            "Web server [dev]: proxying frontend to Vite dev server at {}",
            VITE_DEV_URL
        );
        app = app.fallback(serve_vite_proxy);
    }

    #[cfg(not(debug_assertions))]
    {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        let has_external_dist = exe_dir
            .as_ref()
            .map(|dir| dir.join("dist").exists())
            .unwrap_or(false);

        if has_external_dist {
            let dist_dir = exe_dir.unwrap().join("dist");
            log::info!("Web server: serving static files from {:?}", dist_dir);
            app = app.fallback_service(
                tower_http::services::ServeDir::new(&dist_dir)
                    .append_index_html_on_directories(true)
                    .fallback(tower_http::services::ServeFile::new(
                        dist_dir.join("index.html"),
                    )),
            );
        } else {
            log::info!("Web server: serving embedded frontend assets");
            app = app.fallback(serve_embedded);
        }
    }

    #[cfg(debug_assertions)]
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(Any);

    #[cfg(not(debug_assertions))]
    let cors =
        CorsLayer::new().allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE]);

    let app = app.layer(cors);

    let bind_host = if allow_lan_access { "0.0.0.0" } else { "127.0.0.1" };

    let listener = {
        let mut result = None;

        for attempt in 0..3 {
            let addr = format!("{}:{}", bind_host, port);
            match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => {
                    result = Some((l, port));
                    break;
                }
                Err(e) => {
                    log::warn!(
                        "Web server bind attempt {}/3 on port {}: {}, retrying in 1s...",
                        attempt + 1,
                        port,
                        e
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }

        if result.is_none() {
            for offset in 1..MAX_PORT_ATTEMPTS {
                let Some(try_port) = port.checked_add(offset) else {
                    break;
                };
                let addr = format!("{}:{}", bind_host, try_port);
                match tokio::net::TcpListener::bind(&addr).await {
                    Ok(l) => {
                        result = Some((l, try_port));
                        break;
                    }
                    Err(e) => {
                        log::warn!("Web server port {} unavailable: {}", try_port, e);
                    }
                }
            }
        }

        result
    };

    match listener {
        Some((listener, actual_port)) => {
            ACTUAL_PORT.store(actual_port, Ordering::Relaxed);
            if actual_port != port {
                log::info!(
                    "Web server listening on http://{}:{} (fallback from {})",
                    bind_host,
                    actual_port,
                    port
                );
            } else {
                log::info!("Web server listening on http://{}:{}", bind_host, actual_port);
            }
            if let Err(e) = axum::serve(listener, app).await {
                log::error!("Web server error: {}", e);
            }
        }
        None => {
            log::error!(
                "Web server failed to bind on any port in range {}-{}",
                port,
                port.saturating_add(MAX_PORT_ATTEMPTS - 1)
            );
        }
    }
}

// ============================================================================
// WebSocket
// ============================================================================

async fn handle_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<WebState>,
    _headers: HeaderMap,
) -> impl IntoResponse {
    #[cfg(not(debug_assertions))]
    if !is_same_origin_ws_request(&_headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state))
        .into_response()
}

#[cfg(not(debug_assertions))]
fn is_same_origin_ws_request(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    origin
        .parse::<axum::http::Uri>()
        .ok()
        .and_then(|uri| {
            uri.authority()
                .map(|a| a.as_str().eq_ignore_ascii_case(host))
        })
        .unwrap_or(false)
}

async fn handle_ws_connection(mut socket: WebSocket, state: WebState) {
    let mut rx = state.ws_broadcast.subscribe();
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    ping_interval.tick().await;

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        match serde_json::to_string(&event) {
                            Ok(json) => {
                                if socket.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                log::warn!("WS: failed to serialize event: {}", e);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("WS client lagged, skipped {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            _ = ping_interval.tick() => {
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

// ============================================================================
// API handlers
// ============================================================================

async fn handle_get_interface(State(state): State<WebState>) -> impl IntoResponse {
    let (pi, translations, base_path, data_path) = match (
        state.app_config.project_interface.lock(),
        state.app_config.translations.lock(),
        state.app_config.base_path.lock(),
        state.app_config.data_path.lock(),
    ) {
        (Ok(pi), Ok(translations), Ok(base_path), Ok(data_path)) => (
            pi.clone(),
            translations.clone(),
            base_path.clone(),
            data_path.clone(),
        ),
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "读取 interface 状态失败" })),
            )
                .into_response();
        }
    };

    match pi {
        Some(interface) => Json(serde_json::json!({
            "interface": interface,
            "translations": translations,
            "basePath": base_path,
            "dataPath": data_path,
            "webServerPort": get_actual_port(),
        }))
        .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "interface.json 尚未加载" })),
        )
            .into_response(),
    }
}

async fn handle_get_config(State(state): State<WebState>) -> impl IntoResponse {
    let config = match state.app_config.config.lock() {
        Ok(config) => config.clone(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "读取配置状态失败" })),
            )
                .into_response();
        }
    };
    Json(config).into_response()
}

async fn handle_put_config(
    State(state): State<WebState>,
    Json(new_config): Json<serde_json::Value>,
) -> impl IntoResponse {
    match state.app_config.save_config(new_config) {
        Ok(()) => {
            emit_config_changed(&state.app_handle);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

async fn handle_get_background_image(State(state): State<WebState>) -> impl IntoResponse {
    let config = match state.app_config.config.lock() {
        Ok(c) => c.clone(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "读取背景图配置失败" })),
            )
                .into_response();
        }
    };

    let image_path = config
        .get("settings")
        .and_then(|s| s.get("backgroundImage"))
        .and_then(|p| p.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    match image_path {
        Some(path) => match std::fs::read(&path) {
            Ok(data) => {
                let ext = path.rsplit('.').next().unwrap_or("");
                let content_type = mime_from_extension(ext);
                (StatusCode::OK, [(header::CONTENT_TYPE, content_type)], data).into_response()
            }
            Err(e) => (StatusCode::NOT_FOUND, format!("背景图读取失败: {}", e)).into_response(),
        },
        None => (StatusCode::NOT_FOUND, "未设置背景图片").into_response(),
    }
}

async fn handle_serve_local_file(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    use crate::commands::file_ops::resolve_local_file_path;

    let file_path = match params.get("path") {
        Some(p) if !p.is_empty() => p.as_str(),
        _ => return (StatusCode::BAD_REQUEST, "缺少 path 参数").into_response(),
    };

    let resolved = match resolve_local_file_path(file_path) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    match std::fs::read(&resolved) {
        Ok(data) => {
            let ext = resolved.extension().and_then(|e| e.to_str()).unwrap_or("");
            let content_type = mime_from_extension(ext);
            (StatusCode::OK, [(header::CONTENT_TYPE, content_type)], data).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "文件不存在").into_response(),
    }
}

async fn handle_get_all_logs(State(state): State<WebState>) -> impl IntoResponse {
    match state.app_state.log_buffer.lock() {
        Ok(buffer) => Json(buffer.get_all().clone()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn handle_push_log(
    State(state): State<WebState>,
    axum::extract::Path(instance_id): axum::extract::Path<String>,
    Json(entry): Json<LogEntryDto>,
) -> impl IntoResponse {
    append_runtime_log_line(&instance_id, &entry);

    match state.app_state.log_buffer.lock() {
        Ok(mut buffer) => {
            buffer.push(&instance_id, entry);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn handle_clear_instance_logs(
    State(state): State<WebState>,
    axum::extract::Path(instance_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let result = match state.app_state.log_buffer.lock() {
        Ok(mut buffer) => {
            buffer.clear_instance(&instance_id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if let Ok(path) = runtime_log_path(&instance_id) {
        let _ = std::fs::remove_file(path);
    }

    result
}

async fn handle_heartbeat() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true })).into_response()
}

async fn handle_is_elevated() -> impl IntoResponse {
    Json(serde_json::json!({
        "elevated": crate::commands::system::is_elevated(),
    }))
    .into_response()
}

async fn handle_restart_as_admin(State(state): State<WebState>) -> impl IntoResponse {
    match crate::commands::system::restart_as_admin(state.app_handle) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}
