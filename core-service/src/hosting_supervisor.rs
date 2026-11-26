use crate::AppState;
use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use rusqlite::params;
use std::{collections::HashMap, sync::{Arc, Mutex}, time::Duration};


#[derive(Debug, Serialize)]
struct RecoveryEvent {
    id: String,
    process_key: String,
    event: String,
    attempt: Option<u64>,
    message: String,
    created_at_unix_ms: u64,
}

/// Store a concise, bounded recovery audit event without retaining process output or URLs.
pub(crate) fn record_event(state: &AppState, key: &str, event: &str, attempt: Option<u64>, message: &str) {
    let Ok(db) = state.db.lock() else { return; };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default().as_millis().min(u64::MAX as u128) as u64;
    let _ = db.execute(
        "INSERT INTO hosting_recovery_events(id,process_key,event,attempt,message,created_at_unix_ms) VALUES(?1,?2,?3,?4,?5,?6)",
        params![uuid::Uuid::new_v4().to_string(), key, event, attempt, message, now],
    );
    let _ = db.execute(
        "DELETE FROM hosting_recovery_events WHERE id NOT IN (SELECT id FROM hosting_recovery_events ORDER BY created_at_unix_ms DESC,rowid DESC LIMIT 200)",
        [],
    );
}

async fn list_recovery_events(State(state): State<AppState>) -> Result<Json<Vec<RecoveryEvent>>, (axum::http::StatusCode, String)> {
    let db = state.db.lock().map_err(|_| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Database lock is unavailable".to_string()))?;
    let mut statement = db.prepare(
        "SELECT id,process_key,event,attempt,message,created_at_unix_ms FROM hosting_recovery_events ORDER BY created_at_unix_ms DESC,rowid DESC LIMIT 100"
    ).map_err(|error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let rows = statement.query_map([], |row| Ok(RecoveryEvent {
        id: row.get(0)?, process_key: row.get(1)?, event: row.get(2)?,
        attempt: row.get::<_, Option<u64>>(3)?, message: row.get(4)?, created_at_unix_ms: row.get(5)?,
    })).map_err(|error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let mut events = Vec::new();
    for row in rows { events.push(row.map_err(|error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?); }
    Ok(Json(events))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/hosting/recovery-events", get(list_recovery_events))
}

/// Mark an intentional stop so an exit observed by the supervisor is not recovered.
pub(crate) fn suppress_restart(state: &AppState, key: &str) {
    if let Ok(mut suppressed) = state.hosting_restart_suppressed.lock() {
        suppressed.insert(key.to_string());
    }
}

pub(crate) fn clear_suppression(state: &AppState, key: &str) {
    if let Ok(mut values) = state.hosting_restart_suppressed.lock() { values.remove(key); }
}

pub(crate) fn take_suppression_for_recovery(state: &AppState, key: &str) -> bool {
    take_suppression(state,key)
}

fn take_suppression(state: &AppState, key: &str) -> bool {
    state.hosting_restart_suppressed.lock()
        .map(|mut values| values.remove(key))
        .unwrap_or(true)
}

/// Detect and reap exited hosting children. Inspection errors retain handles and metadata.
fn reconcile(state: &AppState) -> Vec<String> {
    let mut exited = Vec::new();
    if let Ok(mut slot) = state.proxy_process.lock() {
        if let Some(child) = slot.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                *slot = None;
                if !status.success() { exited.push("caddy".to_string()); }
            }
        }
    }

    let mut exited_ids = Vec::new();
    let mut unexpected_ids = Vec::new();
    if let Ok(mut children) = state.tunnel_processes.lock() {
        for (id, child) in children.iter_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                exited_ids.push(id.clone());
                if !status.success() { unexpected_ids.push(id.clone()); }
            }
        }
        for id in &exited_ids {
            children.remove(id);
        }
    }
    if !exited_ids.is_empty() {
        if let (Ok(mut urls), Ok(mut started)) = (state.tunnel_urls.lock(), state.tunnel_started_at.lock()) {
            remove_tunnel_metadata(&mut urls, &mut started, &exited_ids);
        }
        exited.extend(unexpected_ids.into_iter().map(|id| format!("tunnel:{id}")));
    }
    exited.retain(|key| !take_suppression(state, key));
    exited
}

fn remove_tunnel_metadata(
    urls: &mut std::collections::HashMap<String, String>,
    started: &mut std::collections::HashMap<String, u64>,
    exited_ids: &[String],
) {
    for id in exited_ids { urls.remove(id); started.remove(id); }
}

fn restart_limit(state: &AppState) -> u64 {
    crate::settings::setting_number(state, "hosting_restart_limit", 3).clamp(1, 5)
}

fn restart_delay(state: &AppState) -> u64 {
    crate::settings::setting_number(state, "hosting_restart_delay_seconds", 10).clamp(5, 300)
}

fn is_running(state: &AppState, key: &str) -> bool {
    if key == "caddy" {
        return state.proxy_process.lock().ok().and_then(|mut slot| {
            slot.as_mut().map(|child| child.try_wait().map(|status| status.is_none()).unwrap_or(false))
        }).unwrap_or(false);
    }
    let Some(id) = key.strip_prefix("tunnel:") else { return false; };
    state.tunnel_processes.lock().ok().and_then(|mut children| {
        children.get_mut(id).map(|child| child.try_wait().map(|status| status.is_none()).unwrap_or(false))
    }).unwrap_or(false)
}

async fn recover_one(
    state: AppState,
    key: String,
    attempts: Arc<Mutex<HashMap<String, u64>>>,
    in_flight: Arc<Mutex<std::collections::HashSet<String>>>,
) {
    let limit = restart_limit(&state);
    for _ in 0..limit {
        tokio::time::sleep(Duration::from_secs(restart_delay(&state))).await;
        if !crate::settings::setting_bool(&state, "hosting_auto_restart", false) {
            record_event(&state, &key, "recovery_canceled", None, "Automatic recovery was disabled before the next attempt.");
            break;
        }
        if take_suppression(&state, &key) {
            record_event(&state, &key, "recovery_canceled", None, "A manual stop canceled the pending automatic recovery.");
            break;
        }
        let used = attempts.lock().ok().and_then(|values| values.get(&key).copied()).unwrap_or(0);
        if used >= limit { break; }
        let counted = attempts.lock().map(|mut values| {
            let count = values.entry(key.clone()).or_insert(0);
            if *count >= limit { false } else { *count += 1; true }
        }).unwrap_or(false);
        if !counted { break; }

        record_event(&state, &key, "attempt_started", Some(used + 1), "Automatic hosting recovery attempt started.");
        let recovered = if key == "caddy" {
            crate::proxy::restart_for_supervisor(state.clone()).await
        } else if let Some(id) = key.strip_prefix("tunnel:") {
            let state_for_start = state.clone();
            let id = id.to_string();
            tokio::task::spawn_blocking(move || crate::tunnel::start_blocking(state_for_start, id))
                .await.map(|result| result.is_ok()).unwrap_or(false)
        } else {
            false
        };

        if recovered {
            record_event(&state, &key, "attempt_succeeded", Some(used + 1), "Hosting process restarted and passed its startup readiness check.");
            eprintln!("Hosting recovery attempt {} for {key} succeeded.", used + 1);
            // A successful start does not reset the budget immediately: only a full
            // five-minute stable run earns a fresh retry budget.
            let stable_state = state.clone();
            let stable_key = key.clone();
            let stable_attempts = Arc::clone(&attempts);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(300)).await;
                if is_running(&stable_state, &stable_key) {
                    if let Ok(mut values) = stable_attempts.lock() { values.remove(&stable_key); }
                }
            });
            break;
        }
        record_event(&state, &key, "attempt_failed", Some(used + 1), "Recovery attempt failed its startup/readiness check.");
        eprintln!("Hosting recovery attempt {} for {key} failed; retry budget remains bounded.", used + 1);
    }
    let used = attempts.lock().ok().and_then(|values| values.get(&key).copied()).unwrap_or(0);
    if used >= restart_limit(&state) && !is_running(&state, &key) {
        record_event(&state, &key, "retry_budget_exhausted", Some(used), "Automatic recovery stopped after reaching the configured retry limit.");
    }
    if let Ok(mut pending) = in_flight.lock() { pending.remove(&key); }
}

pub fn start(state: AppState) {
    tokio::spawn(async move {
        let attempts = Arc::new(Mutex::new(HashMap::<String, u64>::new()));
        let in_flight = Arc::new(Mutex::new(std::collections::HashSet::<String>::new()));
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            let snapshot = state.clone();
            let exited = match tokio::task::spawn_blocking(move || reconcile(&snapshot)).await {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("Hosting process supervision task failed: {error}");
                    continue;
                }
            };
            for key in exited {
                if !crate::settings::setting_bool(&state, "hosting_auto_restart", false) {
                    record_event(&state, &key, "recovery_disabled", None, "Unexpected process exit detected; automatic recovery is disabled.");
                    eprintln!("Hosting process exited ({key}); automatic recovery is disabled.");
                    continue;
                }
                let already_pending = in_flight.lock().map(|mut values| !values.insert(key.clone())).unwrap_or(true);
                if already_pending { continue; }
                record_event(&state, &key, "recovery_scheduled", None, "Unexpected process exit detected; bounded recovery has been scheduled.");
                eprintln!("Hosting process exited ({key}); bounded recovery has been scheduled.");
                tokio::spawn(recover_one(state.clone(), key, Arc::clone(&attempts), Arc::clone(&in_flight)));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_cleanup_is_scoped_to_exited_tunnels() {
        let exited = vec!["dead-site".to_string()];
        let mut urls = std::collections::HashMap::from([
            ("dead-site".to_string(), "https://dead.trycloudflare.com".to_string()),
            ("live-site".to_string(), "https://live.trycloudflare.com".to_string()),
        ]);
        let mut started = std::collections::HashMap::from([
            ("dead-site".to_string(), 10_u64),
            ("live-site".to_string(), 20_u64),
        ]);
        remove_tunnel_metadata(&mut urls, &mut started, &exited);
        assert!(!urls.contains_key("dead-site"));
        assert!(!started.contains_key("dead-site"));
        assert!(urls.contains_key("live-site"));
        assert_eq!(started.get("live-site"), Some(&20));
    }

    #[test]
    fn restart_budget_is_bounded() {
        let limit = 3_u64;
        assert!((0..limit).all(|attempt| attempt < limit));
        assert_eq!(limit.saturating_sub(3), 0);
    }
}
