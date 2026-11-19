use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, process::Command, time::SystemTime};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/settings", get(get_settings).put(update_settings))
        .route("/api/v1/diagnostics", get(diagnostics))
        .route("/api/v1/recovery/reconcile", axum::routing::post(reconcile))
}

fn defaults() -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("default_website_directory".into(), json!("")),
        ("php_executable".into(), json!("php")),
        ("python_executable".into(), json!("python")),
        ("log_retention_days".into(), json!(30)),
        ("monitoring_interval_seconds".into(), json!(5)),
        ("domain_suffix".into(), json!("localhost")),
        ("release_retention_count".into(), json!(10)),
        ("build_timeout_seconds".into(), json!(600)),
        ("runtime_auto_restart".into(), json!(false)),
        ("runtime_restart_limit".into(), json!(3)),
        ("runtime_restart_delay_seconds".into(), json!(5)),
        ("public_exposure_enabled".into(), json!(false)),
    ])
}

async fn get_settings(State(state): State<AppState>) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let mut values = defaults();
    let db = state.db.lock().map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Database lock is unavailable".into()))?;
    let mut query = db.prepare("SELECT key,value_json FROM settings").map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    let rows = query.query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    for row in rows {
        let (key, raw) = row.map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        if let Ok(value) = serde_json::from_str::<Value>(&raw) { values.insert(key,value); }
    }
    Ok(Json(Value::Object(values.into_iter().collect())))
}

fn validate_setting(key: &str, value: &Value) -> Result<(), String> {
    match key {
        "default_website_directory" => {
            let path=value.as_str().ok_or("Default website directory must be a string")?;
            if path.len()>1024 { return Err("Default website directory is too long".into()); }
            if !path.is_empty() && (!Path::new(path).is_absolute() || !Path::new(path).is_dir()) {
                return Err("Default website directory must be an existing absolute folder".into());
            }
        }
        "php_executable" | "python_executable" => {
            let path=value.as_str().ok_or("Runtime executable must be a string")?;
            if path.trim().is_empty() || path.len()>512 || path.contains('\0') {
                return Err("Runtime executable must contain 1 to 512 valid characters".into());
            }
        }
        "log_retention_days" => {
            let n=value.as_u64().ok_or("Log retention must be an integer")?;
            if !(1..=365).contains(&n) { return Err("Log retention must be between 1 and 365 days".into()); }
        }
        "monitoring_interval_seconds" => {
            let n=value.as_u64().ok_or("Monitoring interval must be an integer")?;
            if !(2..=60).contains(&n) { return Err("Monitoring interval must be between 2 and 60 seconds".into()); }
        }
        "runtime_auto_restart" => { value.as_bool().ok_or("Automatic runtime restart must be true or false")?; }
        "public_exposure_enabled" => { value.as_bool().ok_or("Public exposure must be true or false")?; }
        "runtime_restart_limit" => { let n=value.as_u64().ok_or("Runtime restart limit must be an integer")?; if !(1..=10).contains(&n) { return Err("Runtime restart limit must be between 1 and 10".into()); } }
        "runtime_restart_delay_seconds" => { let n=value.as_u64().ok_or("Runtime restart delay must be an integer")?; if !(1..=300).contains(&n) { return Err("Runtime restart delay must be between 1 and 300 seconds".into()); } }
        "build_timeout_seconds" => {
            let n=value.as_u64().ok_or("Build timeout must be an integer")?;
            if !(30..=1800).contains(&n) { return Err("Build timeout must be between 30 and 1800 seconds".into()); }
        }
        "release_retention_count" => {
            let n=value.as_u64().ok_or("Release retention must be an integer")?;
            if !(1..=100).contains(&n) { return Err("Release retention must be between 1 and 100 releases".into()); }
        }
        "domain_suffix" => {
            let suffix=value.as_str().ok_or("Domain suffix must be a string")?;
            if suffix!="localhost" && suffix!="test" { return Err("Domain suffix must be localhost or test".into()); }
        }
        _ => return Err(format!("Setting '{key}' is not editable")),
    }
    Ok(())
}

async fn update_settings(State(state): State<AppState>, Json(input): Json<Map<String,Value>>) -> Result<Json<Value>, (axum::http::StatusCode,String)> {
    for (key,value) in &input {
        validate_setting(key,value).map_err(|e|(axum::http::StatusCode::BAD_REQUEST,e))?;
    }
    let retention=input.get("log_retention_days").and_then(Value::as_u64);
    let public_exposure_change=input.get("public_exposure_enabled").and_then(Value::as_bool);
    {
        let mut db=state.db.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,"Database lock is unavailable".into()))?;
        let tx=db.transaction().map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        for (key,value) in input {
            tx.execute("INSERT INTO settings(key,value_json,updated_at) VALUES(?1,?2,CURRENT_TIMESTAMP) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,updated_at=CURRENT_TIMESTAMP",rusqlite::params![key,value.to_string()]).map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        }
        tx.commit().map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    }
    if let Some(days)=retention { prune_logs(&state.logs_dir,days); }
    if let Some(enabled)=public_exposure_change {
        if !enabled { crate::tunnel::stop_all(&state); }
        crate::proxy::refresh_running_config(&state);
    }
    get_settings(State(state)).await
}

pub fn setting_string(state:&AppState,key:&str,fallback:&str)->String {
    let Ok(db)=state.db.lock() else { return fallback.into(); };
    db.query_row("SELECT value_json FROM settings WHERE key=?1",[key],|row|row.get::<_,String>(0)).ok()
        .and_then(|raw|serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v|v.as_str().map(str::to_owned))
        .filter(|s|!s.trim().is_empty()).unwrap_or_else(||fallback.into())
}

fn prune_logs(directory:&Path,days:u64) {
    let Ok(entries)=fs::read_dir(directory) else { return; };
    let cutoff=SystemTime::now()-std::time::Duration::from_secs(days.saturating_mul(86400));
    for entry in entries.flatten() {
        let path=entry.path();
        if path.is_file() && entry.metadata().ok().and_then(|m|m.modified().ok()).is_some_and(|modified|modified<cutoff) {
            let _=fs::remove_file(path);
        }
    }
}

fn command_check(program:&str,args:&[&str])->Value {
    match Command::new(program).args(args).output() {
        Ok(output) => {
            let text=String::from_utf8_lossy(if output.stdout.is_empty(){&output.stderr}else{&output.stdout});
            json!({"available":output.status.success(),"program":program,"version":text.lines().next().unwrap_or("").trim()})
        }
        Err(_) => json!({"available":false,"program":program,"version":null}),
    }
}

fn directory_check(path:&Path)->Value {
    let exists=path.is_dir();
    let writable=if exists {
        let probe=path.join(format!(".wd-diagnostic-{}",uuid::Uuid::new_v4()));
        match fs::write(&probe,b"ok") { Ok(())=>{let _=fs::remove_file(probe);true},Err(_)=>false }
    } else { false };
    json!({"path":path.to_string_lossy(),"exists":exists,"writable":writable})
}

async fn diagnostics(State(state):State<AppState>)->Json<Value> {
    let s=state.clone();
    let result=tokio::task::spawn_blocking(move||{
        let (integrity,websites,databases)=match s.db.lock(){
            Ok(db)=>{
                let integrity=db.query_row("PRAGMA integrity_check",[],|r|r.get::<_,String>(0)).unwrap_or_else(|e|format!("error: {e}"));
                let websites=db.query_row("SELECT COUNT(*) FROM websites",[],|r|r.get::<_,u64>(0)).unwrap_or(0);
                let databases=db.query_row("SELECT COUNT(*) FROM managed_databases",[],|r|r.get::<_,u64>(0)).unwrap_or(0);
                (integrity,websites,databases)
            },Err(_)=>("database lock unavailable".into(),0,0)
        };
        let php=setting_string(&s,"php_executable","php");
        let python=setting_string(&s,"python_executable","python");
        let proxy=Command::new(&s.caddy_bin).arg("version").output().ok();
        let proxy_installed=proxy.as_ref().is_some_and(|o|o.status.success());
        let proxy_version=proxy.map(|o|String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").to_string());
        let process_count=s.processes.lock().map(|p|p.len()).unwrap_or(0);
        json!({
            "service":{"status":"online","uptime_seconds":s.started_at.elapsed().as_secs(),"management_bind":"127.0.0.1:47821"},
            "database":{"integrity":integrity,"healthy":integrity=="ok","website_count":websites,"managed_database_count":databases},
            "runtimes":{"node":command_check("node",&["--version"]),"npm":command_check("npm",&["--version"]),"php":command_check(&php,&["-v"]),"python":command_check(&python,&["--version"]),"git":command_check("git",&["--version"]),"caddy":{"available":proxy_installed,"program":s.caddy_bin.to_string_lossy(),"version":proxy_version}},
            "storage":{"logs":directory_check(&s.logs_dir),"releases":directory_check(&s.releases_dir),"backups":directory_check(&s.backups_dir),"config":directory_check(&s.config_dir)},
            "processes":{"managed_count":process_count},
            "security":{"management_api_loopback_only":true,"public_exposure_opt_in":true}
        })
    }).await;
    Json(result.unwrap_or_else(|_|json!({"error":"Diagnostics worker failed"})))
}

async fn reconcile(State(state):State<AppState>)->Json<Value>{
    let active_ids=state.processes.lock().map(|p|p.keys().cloned().collect::<std::collections::HashSet<_>>()).unwrap_or_default();
    let mut updated=0u64;
    if let Ok(db)=state.db.lock(){
        let rows=db.prepare("SELECT id,runtime,status FROM websites WHERE status IN ('starting','stopping','running')").and_then(|mut q|{
            let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;
            rows.collect::<Result<Vec<_>,_>>()
        }).unwrap_or_default();
        for (id,runtime,status) in rows {
            if runtime=="static" && status=="running" { continue; }
            if active_ids.contains(&id) { continue; }
            if db.execute("UPDATE websites SET status='stopped',updated_at=CURRENT_TIMESTAMP WHERE id=?1",[id]).is_ok(){updated+=1;}
        }
    }
    Json(json!({"reconciled":updated,"message":"Stale website states were reset. Static websites that are registered as running were left unchanged."}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_validation_rejects_unsafe_or_unknown_values() {
        assert!(validate_setting("monitoring_interval_seconds",&json!(1)).is_err());
        assert!(validate_setting("log_retention_days",&json!(999)).is_err());
        assert!(validate_setting("domain_suffix",&json!("example.com")).is_err());
        assert!(validate_setting("unknown",&json!(true)).is_err());
        assert!(validate_setting("monitoring_interval_seconds",&json!(10)).is_ok());
        assert!(validate_setting("domain_suffix",&json!("localhost")).is_ok());
        assert!(validate_setting("release_retention_count",&json!(1)).is_ok());
        assert!(validate_setting("build_timeout_seconds",&json!(30)).is_ok());
        assert!(validate_setting("build_timeout_seconds",&json!(1800)).is_ok());
        assert!(validate_setting("build_timeout_seconds",&json!(29)).is_err());
        assert!(validate_setting("build_timeout_seconds",&json!(1801)).is_err());
        assert!(validate_setting("runtime_auto_restart",&json!(true)).is_ok());
        assert!(validate_setting("runtime_auto_restart",&json!("yes")).is_err());
        assert!(validate_setting("public_exposure_enabled",&json!(false)).is_ok());
        assert!(validate_setting("public_exposure_enabled",&json!("off")).is_err());
        assert!(validate_setting("runtime_restart_limit",&json!(1)).is_ok());
        assert!(validate_setting("runtime_restart_limit",&json!(11)).is_err());
        assert!(validate_setting("runtime_restart_delay_seconds",&json!(300)).is_ok());
        assert!(validate_setting("runtime_restart_delay_seconds",&json!(0)).is_err());
        assert!(validate_setting("release_retention_count",&json!(101)).is_err());
    }
}

pub fn setting_number(state:&AppState,key:&str,fallback:u64)->u64 {
    let Ok(db)=state.db.lock() else { return fallback; };
    db.query_row("SELECT value_json FROM settings WHERE key=?1",[key],|row|row.get::<_,String>(0)).ok()
        .and_then(|raw|serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v|v.as_u64()).unwrap_or(fallback)
}


pub fn setting_bool(state:&AppState,key:&str,fallback:bool)->bool {
    let Ok(db)=state.db.lock() else { return fallback; };
    db.query_row("SELECT value_json FROM settings WHERE key=?1",[key],|row|row.get::<_,String>(0)).ok()
        .and_then(|raw|serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v|v.as_bool()).unwrap_or(fallback)
}
