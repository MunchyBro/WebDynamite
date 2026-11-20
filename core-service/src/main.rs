mod db;
mod http;
mod network;
mod monitor;
mod proxy;
mod deploy;
mod git;
mod backup;
mod settings;
mod tunnel;

use std::{collections::{HashMap, HashSet}, env, net::SocketAddr, path::PathBuf, process::Child, sync::{Arc, Mutex}, time::Duration};
use rusqlite::Connection;

const MAX_CONCURRENT_BUILDS: usize = 2;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub build_slots: Arc<tokio::sync::Semaphore>,
    pub processes: Arc<Mutex<HashMap<String, Child>>>,
    pub process_started_at: Arc<Mutex<HashMap<String, std::time::Instant>>>,
    pub logs_dir: PathBuf,
    pub releases_dir: PathBuf,
    pub backups_dir: PathBuf,
    pub config_dir: PathBuf,
    pub caddy_bin: PathBuf,
    pub proxy_process: Arc<Mutex<Option<Child>>>,
    pub tunnel_processes: Arc<Mutex<HashMap<String, Child>>>,
    pub tunnel_urls: Arc<Mutex<HashMap<String, String>>>,
    pub tunnel_started_at: Arc<Mutex<HashMap<String, u64>>>,
    pub cloudflared_bin: PathBuf,
    pub started_at: std::time::Instant,
}

fn data_dir() -> PathBuf {
    if let Ok(path) = env::var("WEBDYNAMITE_DATA_DIR") { return PathBuf::from(path); }
    if let Ok(program_data) = env::var("PROGRAMDATA") { return PathBuf::from(program_data).join("WebDynamite"); }
    env::var("LOCALAPPDATA").map(PathBuf::from)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("WebDynamite")
}

fn restart_attempts_remaining(used: u64, limit: u64) -> u64 {
    limit.saturating_sub(used.min(limit))
}

fn supervise_processes(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        let restart_attempts = Arc::new(Mutex::new(HashMap::<String, u64>::new()));
        let restart_in_flight = Arc::new(Mutex::new(HashSet::<String>::new()));
        loop {
            interval.tick().await;
            let mut exited = Vec::new();
            {
                let mut processes = match state.processes.lock() {
                    Ok(value) => value,
                    Err(_) => break,
                };
                for (id, child) in processes.iter_mut() {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let started = state.process_started_at.lock().ok()
                                .and_then(|mut times| times.remove(id));
                            let uptime = started.map(|at| at.elapsed()).unwrap_or_default();
                            exited.push((id.clone(), status.success(), uptime));
                        }
                        Ok(None) => {}
                        Err(_) => exited.push((id.clone(), false, Duration::ZERO)),
                    }
                }
                for (id, _, _) in &exited { processes.remove(id); }
            }
            for (id, success, uptime) in exited {
                if let Ok(db) = state.db.lock() {
                    let status = if success { "stopped" } else { "error" };
                    let _ = db.execute(
                        "UPDATE websites SET status=?1,updated_at=CURRENT_TIMESTAMP WHERE id=?2",
                        rusqlite::params![status, id],
                    );
                }
                if success {
                    if let Ok(mut attempts) = restart_attempts.lock() { attempts.remove(&id); }
                    continue;
                }
                if uptime >= Duration::from_secs(300) {
                    if let Ok(mut attempts) = restart_attempts.lock() { attempts.remove(&id); }
                }
                if !crate::settings::setting_bool(&state, "runtime_auto_restart", false) { continue; }
                let limit = crate::settings::setting_number(&state, "runtime_restart_limit", 3).clamp(1, 10);
                let delay = crate::settings::setting_number(&state, "runtime_restart_delay_seconds", 5).clamp(1, 300);
                let used = restart_attempts.lock().ok()
                    .and_then(|attempts| attempts.get(&id).copied()).unwrap_or(0);
                if restart_attempts_remaining(used, limit) == 0 { continue; }
                let scheduled = restart_in_flight.lock()
                    .map(|mut pending| pending.insert(id.clone())).unwrap_or(false);
                if !scheduled { continue; }

                // Each website recovers independently. A long delay or slow startup for
                // one crashed site must never pause supervision of the other websites.
                let task_state = state.clone();
                let task_attempts = Arc::clone(&restart_attempts);
                let task_in_flight = Arc::clone(&restart_in_flight);
                tokio::spawn(async move {
                    for _ in 0..restart_attempts_remaining(used, limit) {
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        if !crate::settings::setting_bool(&task_state, "runtime_auto_restart", false) {
                            break;
                        }
                        let already_running = task_state.processes.lock()
                            .map(|processes| processes.contains_key(&id)).unwrap_or(true);
                        if already_running { break; }
                        let still_crashed = task_state.db.lock().ok().and_then(|db| {
                            db.query_row("SELECT status FROM websites WHERE id=?1", [&id], |row| row.get::<_, String>(0)).ok()
                        }).is_some_and(|status| status == "error");
                        if !still_crashed { break; }

                        let counted = task_attempts.lock().map(|mut attempts| {
                            let count = attempts.entry(id.clone()).or_insert(0);
                            if *count >= limit { false } else { *count += 1; true }
                        }).unwrap_or(false);
                        if !counted { break; }

                        if crate::http::restart_after_crash(task_state.clone(), id.clone()).await {
                            break;
                        }
                        if let Ok(db) = task_state.db.lock() {
                            let _ = db.execute(
                                "UPDATE websites SET status='error',updated_at=CURRENT_TIMESTAMP WHERE id=?1",
                                rusqlite::params![id],
                            );
                        }
                    }
                    if let Ok(mut pending) = task_in_flight.lock() { pending.remove(&id); }
                });
            }
        }
    });
}

fn stop_all_processes(state: &AppState) {
    if let Ok(mut processes) = state.processes.lock() {
        for (_, mut child) in processes.drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = env::var("WEBDYNAMITE_API_TOKEN")
        .map_err(|_| "WEBDYNAMITE_API_TOKEN must be configured before starting the Core Service")?;
    if token.trim().len() < 32 { return Err("WEBDYNAMITE_API_TOKEN must contain at least 32 characters".into()); }

    let root = data_dir();
    let database_dir = root.join("database");
    let logs_dir = root.join("logs");
    let config_dir = root.join("config");
    let releases_dir = root.join("releases");
    let backups_dir = root.join("backups");
    std::fs::create_dir_all(&database_dir)?;
    std::fs::create_dir_all(&logs_dir)?;
    std::fs::create_dir_all(&config_dir)?;
    std::fs::create_dir_all(&releases_dir)?;
    std::fs::create_dir_all(&backups_dir)?;
    let connection = Connection::open(database_dir.join("webdynamite.sqlite3"))?;
    db::initialize(&connection)?;

    let state = AppState {
        db: Arc::new(Mutex::new(connection)),
        build_slots: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_BUILDS)),
        processes: Arc::new(Mutex::new(HashMap::new())),
        process_started_at: Arc::new(Mutex::new(HashMap::new())),
        logs_dir,
        releases_dir,
        backups_dir,
        config_dir,
        caddy_bin: env::var_os("CADDY_BIN").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("caddy")),
        proxy_process: Arc::new(Mutex::new(None)),
        tunnel_processes: Arc::new(Mutex::new(HashMap::new())),
        tunnel_urls: Arc::new(Mutex::new(HashMap::new())),
        tunnel_started_at: Arc::new(Mutex::new(HashMap::new())),
        cloudflared_bin: env::var_os("CLOUDFLARED_BIN").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cloudflared")),
        started_at: std::time::Instant::now(),
    };
    supervise_processes(state.clone());
    backup::start_scheduler(state.clone());
    let app = http::router(state.clone(), token);
    let address = SocketAddr::from(([127, 0, 0, 1], 47821));
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("WebDynamite Core Service listening on http://{address}");
    let result = axum::serve(listener, app).with_graceful_shutdown(shutdown()).await;
    stop_all_processes(&state);
    tunnel::stop_all(&state);
    proxy::stop_on_shutdown(&state);
    result?;
    Ok(())
}

async fn shutdown() { let _ = tokio::signal::ctrl_c().await; }

#[cfg(test)]
mod supervisor_tests {
    use super::restart_attempts_remaining;

    #[test]
    fn restart_attempt_budget_is_bounded() {
        assert_eq!(restart_attempts_remaining(0, 3), 3);
        assert_eq!(restart_attempts_remaining(2, 3), 1);
        assert_eq!(restart_attempts_remaining(3, 3), 0);
        assert_eq!(restart_attempts_remaining(99, 3), 0);
        assert_eq!(restart_attempts_remaining(0, 1), 1);
    }

    #[tokio::test]
    async fn build_capacity_rejects_work_above_global_limit() {
        let slots = tokio::sync::Semaphore::new(super::MAX_CONCURRENT_BUILDS);
        let first = slots.clone().try_acquire_owned().unwrap();
        let second = slots.clone().try_acquire_owned().unwrap();
        assert!(slots.clone().try_acquire_owned().is_err());
        drop(first);
        assert!(slots.clone().try_acquire_owned().is_ok());
        drop(second);
    }
}
