mod db;
mod http;
mod network;
mod monitor;
mod proxy;
mod deploy;
mod git;
mod backup;

use std::{collections::HashMap, env, net::SocketAddr, path::PathBuf, process::Child, sync::{Arc, Mutex}, time::Duration};
use rusqlite::Connection;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub processes: Arc<Mutex<HashMap<String, Child>>>,
    pub logs_dir: PathBuf,
    pub releases_dir: PathBuf,
    pub backups_dir: PathBuf,
    pub config_dir: PathBuf,
    pub caddy_bin: PathBuf,
    pub proxy_process: Arc<Mutex<Option<Child>>>,
}

fn data_dir() -> PathBuf {
    if let Ok(path) = env::var("WEBDYNAMITE_DATA_DIR") { return PathBuf::from(path); }
    if let Ok(program_data) = env::var("PROGRAMDATA") { return PathBuf::from(program_data).join("WebDynamite"); }
    env::var("LOCALAPPDATA").map(PathBuf::from)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("WebDynamite")
}

fn supervise_processes(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
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
                        Ok(Some(status)) => exited.push((id.clone(), status.success())),
                        Ok(None) => {}
                        Err(_) => exited.push((id.clone(), false)),
                    }
                }
                for (id, _) in &exited { processes.remove(id); }
            }
            if !exited.is_empty() {
                if let Ok(db) = state.db.lock() {
                    for (id, success) in exited {
                        let status = if success { "stopped" } else { "crashed" };
                        let _ = db.execute(
                            "UPDATE websites SET status=?1,updated_at=CURRENT_TIMESTAMP WHERE id=?2",
                            rusqlite::params![status, id],
                        );
                    }
                }
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
        processes: Arc::new(Mutex::new(HashMap::new())),
        logs_dir,
        releases_dir,
        backups_dir,
        config_dir,
        caddy_bin: env::var_os("CADDY_BIN").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("caddy")),
        proxy_process: Arc::new(Mutex::new(None)),
    };
    supervise_processes(state.clone());
    let app = http::router(state.clone(), token);
    let address = SocketAddr::from(([127, 0, 0, 1], 47821));
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("WebDynamite Core Service listening on http://{address}");
    let result = axum::serve(listener, app).with_graceful_shutdown(shutdown()).await;
    stop_all_processes(&state);
    proxy::stop_on_shutdown(&state);
    result?;
    Ok(())
}

async fn shutdown() { let _ = tokio::signal::ctrl_c().await; }
