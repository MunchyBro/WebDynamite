use crate::AppState;
use std::time::Duration;

/// Reconcile Caddy and Quick Tunnel child handles independently of UI/API polling.
/// All process inspection runs on Tokio's blocking pool; no std mutex guard crosses
/// an await point. Inspection errors retain the handle and metadata conservatively.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            let snapshot = state.clone();
            match tokio::task::spawn_blocking(move || reconcile(&snapshot)).await {
                Ok((caddy_exited, tunnels_exited)) if caddy_exited || tunnels_exited > 0 => {
                    eprintln!("Hosting process supervisor reaped Caddy: {caddy_exited}, tunnels: {tunnels_exited}");
                }
                Ok(_) => {}
                Err(error) => eprintln!("Hosting process supervision task failed: {error}"),
            }
        }
    });
}

fn reconcile(state: &AppState) -> (bool, usize) {
    let caddy_exited = match state.proxy_process.lock() {
        Ok(mut slot) => match slot.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(_status)) => { *slot = None; true }
                Ok(None) | Err(_) => false,
            },
            None => false,
        },
        Err(_) => false,
    };

    let mut exited_ids = Vec::new();
    if let Ok(mut children) = state.tunnel_processes.lock() {
        for (id, child) in children.iter_mut() {
            if matches!(child.try_wait(), Ok(Some(_))) {
                exited_ids.push(id.clone());
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
    }
    (caddy_exited, exited_ids.len())
}

fn remove_tunnel_metadata(
    urls: &mut std::collections::HashMap<String, String>,
    started: &mut std::collections::HashMap<String, u64>,
    exited_ids: &[String],
) {
    for id in exited_ids { urls.remove(id); started.remove(id); }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The pure cleanup contract is that only IDs reported exited are removed from
    // the two metadata registries; unrelated active tunnel metadata is untouched.
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
}
