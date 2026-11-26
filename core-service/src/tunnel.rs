use axum::{extract::{Path, State}, Json, http::StatusCode};
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use crate::AppState;

#[derive(Serialize)]
pub struct TunnelInfo {
    website_id: String,
    website_name: String,
    runtime: String,
    running: bool,
    url: Option<String>,
    started_at: Option<u64>,
    log_path: Option<String>,
}

fn tunnel_log_path(state: &AppState, website_id: &str) -> std::path::PathBuf {
    let safe_id: String = website_id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    state.logs_dir.join("tunnels").join(format!("{}.log", if safe_id.is_empty() { "unknown" } else { &safe_id }))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({"error":{"code":"tunnel_error","message":message.into()}})))
}

fn quick_tunnel_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let candidate = &line[start..];
    let end = candidate.find(char::is_whitespace).unwrap_or(candidate.len());
    let url = candidate[..end].trim_end_matches(|c: char| matches!(c, ')' | ',' | '.' | ']' | '"'));
    url.ends_with(".trycloudflare.com").then(|| url.to_string())
}

fn capture_urls<R: std::io::Read + Send + 'static>(
    stream: R,
    urls: Arc<Mutex<std::collections::HashMap<String, String>>>,
    website_id: String,
    log_path: std::path::PathBuf,
    logged_bytes: Arc<Mutex<usize>>,
) {
    thread::spawn(move || {
        let mut log = OpenOptions::new().create(true).append(true).open(log_path).ok();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break; };
            let line_bytes = line.len().saturating_add(1);
            if let Ok(mut used) = logged_bytes.lock() {
                if used.saturating_add(line_bytes) <= 512 * 1024 {
                    if let Some(file) = log.as_mut() {
                        if writeln!(file, "{line}").is_ok() {
                            *used += line_bytes;
                        }
                    }
                }
            }
            if let Some(url) = quick_tunnel_url(&line) {
                if let Ok(mut registered) = urls.lock() {
                    registered.entry(website_id.clone()).or_insert(url);
                }
            }
        }
    });
}

pub async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<TunnelInfo>>, (StatusCode, Json<serde_json::Value>)> {
    let sites = {
        let db = state.db.lock().map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Database lock unavailable"))?;
        let mut query = db.prepare("SELECT id,name,runtime FROM websites ORDER BY name")
            .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let rows = query.query_map([], |row| Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))).map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    let mut children = state.tunnel_processes.lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel process registry unavailable"))?;
    let mut urls = state.tunnel_urls.lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel URL registry unavailable"))?;
    let mut started = state.tunnel_started_at.lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel start registry unavailable"))?;

    let mut output = Vec::with_capacity(sites.len());
    for (id, name, runtime) in sites {
        let running = children.get_mut(&id)
            .map(|child| child.try_wait().map(|status| status.is_none()).unwrap_or(true))
            .unwrap_or(false);
        if !running {
            children.remove(&id);
            urls.remove(&id);
            started.remove(&id);
        }
        output.push(TunnelInfo {
            website_id: id.clone(),
            website_name: name,
            runtime,
            running,
            url: if running { urls.get(&id).cloned() } else { None },
            started_at: if running { started.get(&id).copied() } else { None },
            log_path: Some(tunnel_log_path(&state, &id).to_string_lossy().into_owned()),
        });
    }
    Ok(Json(output))
}

pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TunnelInfo>, (StatusCode, Json<serde_json::Value>)> {
    tokio::task::spawn_blocking(move || start_blocking(state, id))
        .await
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, format!("Tunnel startup task failed: {e}")))?
}

fn start_blocking(state: AppState, id: String) -> Result<Json<TunnelInfo>, (StatusCode, Json<serde_json::Value>)> {
    if !crate::settings::setting_bool(&state, "public_exposure_enabled", false) {
        return Err(error(StatusCode::FORBIDDEN, "Public exposure is disabled globally. Enable it in Networking before starting a tunnel."));
    }
    let (name, runtime, status, port) = {
        let db = state.db.lock().map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Database lock unavailable"))?;
        db.query_row(
            "SELECT name,runtime,status,port FROM websites WHERE id=?1",
            [&id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<u16>>(3)?,
            )),
        ).map_err(|_| error(StatusCode::NOT_FOUND, "Website not found"))?
    };

    if runtime != "static" && status != "running" {
        return Err(error(StatusCode::CONFLICT, "Start the website before publishing it through a tunnel."));
    }
    let origin = if runtime == "static" {
        format!("http://127.0.0.1:47821/hosted/{id}")
    } else {
        format!("http://127.0.0.1:{}", port.ok_or_else(|| error(StatusCode::CONFLICT, "Website has no allocated runtime port."))?)
    };

    let mut children = state.tunnel_processes.lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel process registry unavailable"))?;
    if let Some(child) = children.get_mut(&id) {
        match child.try_wait() {
            Ok(None) => return Err(error(StatusCode::CONFLICT, "A tunnel is already running for this website.")),
            Ok(Some(_)) => { children.remove(&id); }
            Err(e) => return Err(error(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not inspect the existing tunnel process: {e}"))),
        }
    }

    let log_path = tunnel_log_path(&state, &id);
    let log_dir = log_path.parent().ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel log directory is unavailable"))?;
    fs::create_dir_all(log_dir).map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not create tunnel log directory: {e}")))?;
    OpenOptions::new().create(true).write(true).truncate(true).open(&log_path)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not create tunnel log {}: {e}", log_path.display())))?;

    let mut child = Command::new(&state.cloudflared_bin)
        .args(["tunnel", "--url", &origin, "--no-autoupdate"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, format!(
            "cloudflared could not start. Install cloudflared or set CLOUDFLARED_BIN. {e}"
        )))?;

    // Clear old metadata before reader threads can publish the replacement URL.
    let started_at = now();
    state.tunnel_urls.lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel URL registry unavailable"))?
        .remove(&id);
    state.tunnel_started_at.lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel start registry unavailable"))?
        .insert(id.clone(), started_at);

    let stdout = child.stdout.take().ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "Could not capture tunnel stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "Could not capture tunnel stderr"))?;
    let logged_bytes = Arc::new(Mutex::new(0usize));
    capture_urls(stdout, state.tunnel_urls.clone(), id.clone(), log_path.clone(), logged_bytes.clone());
    capture_urls(stderr, state.tunnel_urls.clone(), id.clone(), log_path.clone(), logged_bytes);
    children.insert(id.clone(), child);
    drop(children);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let url = loop {
        let process_exit = {
            let mut children = state.tunnel_processes.lock()
                .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel process registry unavailable"))?;
            match children.get_mut(&id) {
                Some(child) => match child.try_wait() {
                    Ok(None) => None,
                    Ok(Some(status)) => Some(Ok(status.code())),
                    Err(e) => Some(Err(e.to_string())),
                },
                None => Some(Err("Tunnel process disappeared during startup.".to_string())),
            }
        };
        if let Some(exit) = process_exit {
            match exit {
                Ok(code) => {
                    if let Ok(mut children) = state.tunnel_processes.lock() { children.remove(&id); }
                    if let Ok(mut urls) = state.tunnel_urls.lock() { urls.remove(&id); }
                    if let Ok(mut started) = state.tunnel_started_at.lock() { started.remove(&id); }
                    return Err(error(StatusCode::BAD_GATEWAY, format!(
                        "cloudflared exited before announcing a Quick Tunnel URL (exit code: {}). See {}",
                        code.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()),
                        log_path.display()
                    )));
                }
                Err(detail) => return Err(error(StatusCode::INTERNAL_SERVER_ERROR, format!(
                    "Could not verify tunnel startup: {detail}. The process handle was retained for safe inspection."
                ))),
            }
        }
        let announced = state.tunnel_urls.lock()
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel URL registry unavailable"))?
            .get(&id).cloned();
        if let Some(url) = announced { break url; }
        if std::time::Instant::now() >= deadline {
            let mut children = state.tunnel_processes.lock()
                .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel process registry unavailable"))?;
            if let Some(child) = children.get_mut(&id) {
                child.kill().map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, format!(
                    "Tunnel startup timed out and could not be stopped: {e}. See {}", log_path.display()
                )))?;
                child.wait().map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, format!(
                    "Tunnel startup timed out and could not be reaped: {e}. See {}", log_path.display()
                )))?;
            }
            children.remove(&id);
            drop(children);
            if let Ok(mut urls) = state.tunnel_urls.lock() { urls.remove(&id); }
            if let Ok(mut started) = state.tunnel_started_at.lock() { started.remove(&id); }
            return Err(error(StatusCode::GATEWAY_TIMEOUT, format!(
                "cloudflared did not announce a Quick Tunnel URL within 10 seconds. The process was stopped; inspect {}",
                log_path.display()
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    Ok(Json(TunnelInfo {
        website_id: id,
        website_name: name,
        runtime,
        running: true,
        url: Some(url),
        started_at: Some(started_at),
        log_path: Some(log_path.to_string_lossy().into_owned()),
    }))
}
pub async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let mut children = state.tunnel_processes.lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Tunnel process registry unavailable"))?;
    if let Some(child) = children.get_mut(&id) {
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                child.kill().map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not stop tunnel process: {e}")))?;
                child.wait().map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not reap tunnel process: {e}")))?;
            }
            Err(e) => return Err(error(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not inspect tunnel process before stopping: {e}"))),
        }
        children.remove(&id);
    }
    drop(children);
    if let Ok(mut urls) = state.tunnel_urls.lock() { urls.remove(&id); }
    if let Ok(mut started) = state.tunnel_started_at.lock() { started.remove(&id); }
    Ok(StatusCode::NO_CONTENT)
}

pub fn stop_all(state: &AppState) {
    if let Ok(mut children) = state.tunnel_processes.lock() {
        for (_, mut child) in children.drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    if let Ok(mut urls) = state.tunnel_urls.lock() { urls.clear(); }
    if let Ok(mut started) = state.tunnel_started_at.lock() { started.clear(); }
}

#[cfg(test)]
mod tests {
    use super::quick_tunnel_url;

    #[test]
    fn extracts_quick_tunnel_url_from_plain_output() {
        assert_eq!(
            quick_tunnel_url("Your quick Tunnel has been created! Visit it at https://bright-river.trycloudflare.com"),
            Some("https://bright-river.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn extracts_url_with_cli_punctuation() {
        assert_eq!(
            quick_tunnel_url("INF | https://bright-river.trycloudflare.com."),
            Some("https://bright-river.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn ignores_non_quick_tunnel_urls() {
        assert_eq!(quick_tunnel_url("https://example.com"), None);
        assert_eq!(quick_tunnel_url("Tunnel is starting"), None);
    }
}
