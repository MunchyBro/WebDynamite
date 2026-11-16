mod db;
mod http;

use std::{env, net::SocketAddr, path::PathBuf, sync::{Arc, Mutex}};
use rusqlite::Connection;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
}

fn data_dir() -> PathBuf {
    if let Ok(path) = env::var("WEBDYNAMITE_DATA_DIR") {
        return PathBuf::from(path);
    }
    if let Ok(program_data) = env::var("PROGRAMDATA") {
        return PathBuf::from(program_data).join("WebDynamite");
    }
    env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("WebDynamite")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = env::var("WEBDYNAMITE_API_TOKEN")
        .map_err(|_| "WEBDYNAMITE_API_TOKEN must be configured before starting the Core Service")?;
    if token.trim().len() < 32 {
        return Err("WEBDYNAMITE_API_TOKEN must contain at least 32 characters".into());
    }

    let directory = data_dir().join("database");
    std::fs::create_dir_all(&directory)?;
    let connection = Connection::open(directory.join("webdynamite.sqlite3"))?;
    db::initialize(&connection)?;

    let state = AppState { db: Arc::new(Mutex::new(connection)) };
    let app = http::router(state, token);
    let address = SocketAddr::from(([127, 0, 0, 1], 47821));
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("WebDynamite Core Service listening on http://{address}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
