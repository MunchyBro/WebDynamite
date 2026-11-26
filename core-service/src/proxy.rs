use axum::{extract::{Path, State}, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use rusqlite::{params, OptionalExtension};
use std::{fs::{self, OpenOptions}, net::{SocketAddr, TcpStream}, process::{Command, Stdio}, time::Duration};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/proxy/status", get(status))
        .route("/api/v1/proxy/tls-check/:hostname", axum::routing::post(tls_check))
        .route("/api/v1/proxy/tls-details/:hostname", get(tls_details))
        .route("/api/v1/proxy/tls-history/:hostname", get(tls_history))
        .route("/api/v1/proxy/tls-monitoring", get(get_tls_monitoring).put(save_tls_monitoring))
        .route("/api/v1/proxy/tls-alerts", get(list_tls_alerts))
        .route("/api/v1/proxy/tls-alerts/:id/acknowledge", axum::routing::post(acknowledge_tls_alert))
        .route("/api/v1/proxy/validate", axum::routing::post(validate))
        .route("/api/v1/proxy/start", axum::routing::post(start))
        .route("/api/v1/proxy/stop", axum::routing::post(stop))
        .route("/api/v1/proxy/reload", axum::routing::post(reload))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TlsMonitoringSettings { enabled: bool, interval_hours: i64, warning_days: i64, next_run_at: Option<i64>, last_run_at: Option<i64>, last_status: Option<String>, last_error: Option<String> }
#[derive(Debug, Deserialize)]
struct SaveTlsMonitoring { enabled: bool, interval_hours: i64, warning_days: i64 }
fn unix_now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64 }
fn tls_settings_sync(state:&AppState)->Result<TlsMonitoringSettings,String>{
 let db=state.db.lock().map_err(|_|"Database lock is unavailable".to_string())?;
 db.query_row("SELECT enabled,interval_hours,warning_days,next_run_at,last_run_at,last_status,last_error FROM tls_monitoring_settings WHERE id=1",[],|r|Ok(TlsMonitoringSettings{enabled:r.get::<_,i64>(0)?!=0,interval_hours:r.get(1)?,warning_days:r.get(2)?,next_run_at:r.get(3)?,last_run_at:r.get(4)?,last_status:r.get(5)?,last_error:r.get(6)?})).map_err(|e|e.to_string())
}
async fn get_tls_monitoring(State(state):State<AppState>)->Result<Json<TlsMonitoringSettings>,(axum::http::StatusCode,Json<Value>)>{
 tls_settings_sync(&state).map(Json).map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"tls_settings_failed","message":e}}))))
}
async fn save_tls_monitoring(State(state):State<AppState>,Json(input):Json<SaveTlsMonitoring>)->Result<Json<TlsMonitoringSettings>,(axum::http::StatusCode,Json<Value>)>{
 if !(1..=168).contains(&input.interval_hours)||!(1..=90).contains(&input.warning_days){return Err((axum::http::StatusCode::BAD_REQUEST,Json(json!({"error":{"code":"invalid_tls_monitoring_settings","message":"Check interval must be 1–168 hours and expiry warning must be 1–90 days."}}))));}
 let next=if input.enabled{Some(unix_now()+input.interval_hours*3600)}else{None};
 {let db=state.db.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_locked","message":"Database lock is unavailable."}}))))?;
 db.execute("UPDATE tls_monitoring_settings SET enabled=?1,interval_hours=?2,warning_days=?3,next_run_at=?4,last_error=NULL WHERE id=1",params![input.enabled as i64,input.interval_hours,input.warning_days,next]).map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"tls_settings_save_failed","message":e.to_string()}}))))?;}
 get_tls_monitoring(State(state)).await
}
async fn list_tls_alerts(State(state):State<AppState>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 let db=state.db.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_locked","message":"Database lock is unavailable."}}))))?;
 let mut q=db.prepare("SELECT id,hostname,event,message,thumbprint,days_remaining,created_at,acknowledged_at FROM tls_certificate_alerts ORDER BY created_at DESC,id DESC LIMIT 200").map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"tls_alert_query_failed","message":e.to_string()}}))))?;
 let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"hostname":r.get::<_,String>(1)?,"event":r.get::<_,String>(2)?,"message":r.get::<_,String>(3)?,"thumbprint":r.get::<_,String>(4)?,"days_remaining":r.get::<_,Option<i64>>(5)?,"created_at":r.get::<_,i64>(6)?,"acknowledged_at":r.get::<_,Option<i64>>(7)?}))).map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"tls_alert_query_failed","message":e.to_string()}}))))?;
 let mut alerts=Vec::new();for row in rows{alerts.push(row.map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"tls_alert_query_failed","message":e.to_string()}}))))?);}
 Ok(Json(json!({"alerts":alerts,"count":alerts.len(),"retention":200})))
}
async fn acknowledge_tls_alert(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 let now=unix_now();let db=state.db.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_locked","message":"Database lock is unavailable."}}))))?;
 let changed=db.execute("UPDATE tls_certificate_alerts SET acknowledged_at=?1 WHERE id=?2 AND acknowledged_at IS NULL",params![now,id]).map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"tls_alert_ack_failed","message":e.to_string()}}))))?;
 if changed==0{return Err((axum::http::StatusCode::NOT_FOUND,Json(json!({"error":{"code":"tls_alert_not_found","message":"Alert was not found or was already acknowledged."}}))));}
 Ok(Json(json!({"id":id,"acknowledged_at":now})))
}
fn inspect_certificate_sync(hostname:&str)->Result<Value,String>{
 if !cfg!(windows){return Err("Automatic certificate inspection currently requires Windows PowerShell.".into());}
 const SCRIPT:&str=r#"$ErrorActionPreference='Stop'; $hostName=$env:WEBDYNAMITE_TLS_HOST; $client=[System.Net.Sockets.TcpClient]::new(); $client.ReceiveTimeout=8000; $client.SendTimeout=8000; try { $task=$client.ConnectAsync($hostName,443); if(-not $task.Wait(8000)){ throw 'Connection timed out while contacting port 443.' }; $callback=[System.Net.Security.RemoteCertificateValidationCallback]{ param($sender,$certificate,$chain,$errors) return $true }; $stream=[System.Net.Security.SslStream]::new($client.GetStream(),$false,$callback); try { $stream.ReadTimeout=8000; $stream.WriteTimeout=8000; $stream.AuthenticateAsClient($hostName); $cert=[System.Security.Cryptography.X509Certificates.X509Certificate2]::new($stream.RemoteCertificate); [pscustomobject]@{ subject=$cert.Subject; issuer=$cert.Issuer; not_before=$cert.NotBefore.ToUniversalTime().ToString('o'); not_after=$cert.NotAfter.ToUniversalTime().ToString('o'); thumbprint=$cert.Thumbprint } | ConvertTo-Json -Compress } finally { $stream.Dispose() } } finally { $client.Dispose() }"#;
 let output=Command::new("powershell.exe").args(["-NoLogo","-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-Command",SCRIPT]).env("WEBDYNAMITE_TLS_HOST",hostname).output().map_err(|e|format!("Could not start PowerShell certificate inspection: {e}"))?;
 if !output.status.success(){let detail=String::from_utf8_lossy(&output.stderr).trim().to_string();return Err(if detail.is_empty(){"Could not retrieve the remote TLS certificate.".into()}else{detail});}
 serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).map_err(|e|format!("PowerShell returned invalid certificate metadata: {e}"))
}
fn record_tls_scan(state:&AppState,hostname:&str,cert:&Value,warning_days:i64,now:i64)->Result<(),String>{
 let subject=cert.get("subject").and_then(Value::as_str).unwrap_or("");let issuer=cert.get("issuer").and_then(Value::as_str).unwrap_or("");
 let not_before=cert.get("not_before").and_then(Value::as_str).unwrap_or("");let not_after=cert.get("not_after").and_then(Value::as_str).unwrap_or("");
 let thumbprint=cert.get("thumbprint").and_then(Value::as_str).unwrap_or("");if thumbprint.is_empty(){return Err("Certificate response did not include a thumbprint.".into());}
 let expiry=chrono_free_parse_epoch(not_after).ok_or_else(||"Certificate expiry timestamp could not be parsed.".to_string())?;let days=(expiry-now).div_euclid(86400);
 let db=state.db.lock().map_err(|_|"Database lock is unavailable".to_string())?;
 let previous:Option<String>=db.query_row("SELECT thumbprint FROM tls_certificate_history WHERE hostname=?1 ORDER BY checked_at DESC,id DESC LIMIT 1",[hostname],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
 db.execute("INSERT INTO tls_certificate_history(hostname,subject,issuer,not_before,not_after,thumbprint,checked_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![hostname,subject,issuer,not_before,not_after,thumbprint,now]).map_err(|e|e.to_string())?;
 db.execute("DELETE FROM tls_certificate_history WHERE hostname=?1 AND id NOT IN (SELECT id FROM tls_certificate_history WHERE hostname=?1 ORDER BY checked_at DESC,id DESC LIMIT 30)",[hostname]).map_err(|e|e.to_string())?;
 let mut add_alert=|event:&str,message:String,fingerprint:String,remaining:Option<i64>|->Result<(),String>{
  db.execute("INSERT OR IGNORE INTO tls_certificate_alerts(id,hostname,event,message,thumbprint,days_remaining,created_at,fingerprint) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![uuid::Uuid::new_v4().to_string(),hostname,event,message,thumbprint,remaining,now,fingerprint]).map_err(|e|e.to_string())?;Ok(())
 };
 if let Some(previous_thumbprint)=previous.as_deref(){if previous_thumbprint!=thumbprint{add_alert("renewed",format!("A different TLS certificate is now presented by {hostname}."),format!("{hostname}:renewed:{thumbprint}"),Some(days))?;}}
 if days<0{add_alert("expired",format!("The TLS certificate for {hostname} expired {} day(s) ago.",days.saturating_abs()),format!("{hostname}:expired:{thumbprint}"),Some(days))?;}
 else if days<=warning_days{add_alert("expiring",format!("The TLS certificate for {hostname} expires in {days} day(s)."),format!("{hostname}:expiring:{thumbprint}"),Some(days))?;}
 Ok(())
}
pub fn start_tls_monitoring_scheduler(state:AppState){
 tokio::spawn(async move{let mut tick=tokio::time::interval(Duration::from_secs(30));loop{tick.tick().await;
  let settings=match tls_settings_sync(&state){Ok(value)=>value,Err(_)=>continue};if !settings.enabled||settings.next_run_at.is_none_or(|next|unix_now()<next){continue;}
  let now=unix_now();let hostnames={let db=match state.db.lock(){Ok(value)=>value,Err(_)=>continue};let mut q=match db.prepare("SELECT hostname FROM public_domains ORDER BY hostname"){Ok(value)=>value,Err(_)=>continue};let rows=match q.query_map([],|r|r.get::<_,String>(0)){Ok(value)=>value,Err(_)=>continue};rows.filter_map(Result::ok).collect::<Vec<_>>()};
  if let Ok(db)=state.db.lock(){let _=db.execute("UPDATE tls_monitoring_settings SET next_run_at=?1 WHERE id=1",params![now+settings.interval_hours*3600]);}
  let task_state=state.clone();let warning=settings.warning_days;let outcome=tokio::task::spawn_blocking(move||{let mut checked=0usize;let mut failures=Vec::new();for hostname in hostnames{match inspect_certificate_sync(&hostname).and_then(|cert|record_tls_scan(&task_state,&hostname,&cert,warning,now)){Ok(())=>checked+=1,Err(error)=>failures.push(format!("{hostname}: {error}"))}}(checked,failures)}).await;
  let (checked,failures)=match outcome{Ok(value)=>value,Err(error)=>(0,vec![error.to_string()])};let status=if failures.is_empty(){"success"}else if checked>0{"partial"}else{"failed"};
  let error=if failures.is_empty(){None}else{Some(failures.into_iter().take(5).collect::<Vec<_>>().join("; "))};
  if let Ok(db)=state.db.lock(){let _=db.execute("UPDATE tls_monitoring_settings SET last_run_at=?1,last_status=?2,last_error=?3 WHERE id=1",params![now,status,error]);}
 }});
}

fn config_path(state:&AppState)->std::path::PathBuf { state.config_dir.join("Caddyfile") }
fn caddy_log_path(state:&AppState)->std::path::PathBuf { state.logs_dir.join("caddy.log") }
fn caddy_admin_ready()->bool {
    TcpStream::connect_timeout(
        &SocketAddr::from(([127,0,0,1],2019)),
        Duration::from_millis(250),
    ).is_ok()
}
pub(crate) fn generate_config(state:&AppState)->Result<(String,usize,usize),String>{
 let public_exposure_enabled=crate::settings::setting_bool(state,"public_exposure_enabled",false);
 let db=state.db.lock().map_err(|_|"Database lock is unavailable".to_string())?;
 let mut q=db.prepare("SELECT d.hostname,d.website_id,w.runtime,w.port FROM domains d JOIN websites w ON w.id=d.website_id ORDER BY d.hostname").map_err(|e|e.to_string())?;
 let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<u16>>(3)?))).map_err(|e|e.to_string())?;
 let mut config=String::from("# Generated by WebDynamite. Do not edit manually.\n");
 let mut routes=0; let mut pending=0;
 for row in rows { let(host,id,runtime,port)=row.map_err(|e|e.to_string())?;
  if runtime=="node" || runtime=="php" || runtime=="python" { if let Some(port)=port { config.push_str(&format!("http://{host} {{\n    bind 127.0.0.1\n    reverse_proxy 127.0.0.1:{port}\n}}\n\n")); routes+=1; } else {pending+=1;} }
  else {config.push_str(&format!("http://{host} {{\n    bind 127.0.0.1\n    @root path /\n    rewrite @root /hosted/{id}/index.html\n    @assets not path /\n    rewrite @assets /hosted/{id}{{uri}}\n    reverse_proxy 127.0.0.1:47821\n}}\n\n")); routes+=1;}
 }
 // Public domains are explicit opt-ins. Caddy automatically provisions HTTPS when
 // public DNS and inbound HTTP/HTTPS reachability are correctly configured.
 let public_sites={
  let mut q=db.prepare("SELECT p.hostname,p.website_id,w.runtime,w.port FROM public_domains p JOIN websites w ON w.id=p.website_id ORDER BY p.hostname").map_err(|e|e.to_string())?;
  let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<u16>>(3)?))).map_err(|e|e.to_string())?;
  rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?
 };
 if public_exposure_enabled {
 for (host,id,runtime,port) in public_sites {
  if runtime=="static" {
   config.push_str(&format!("{host} {{\n    @root path /\n    rewrite @root /hosted/{id}/index.html\n    @assets not path /\n    rewrite @assets /hosted/{id}{{uri}}\n    reverse_proxy 127.0.0.1:47821\n}}\n\n"));
   routes+=1;
  } else if let Some(upstream)=port {
   config.push_str(&format!("{host} {{\n    reverse_proxy 127.0.0.1:{upstream}\n}}\n\n"));
   routes+=1;
  } else {pending+=1;}
 }
 }
 // LAN publishing is opt-in per website and binds to the private interface only.
 let lan_sites={
  let mut q=db.prepare("SELECT id,runtime,port,lan_port FROM websites WHERE lan_enabled=1 ORDER BY name").map_err(|e|e.to_string())?;
  let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<u16>>(2)?,r.get::<_,Option<u16>>(3)?))).map_err(|e|e.to_string())?;
  rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?
 };
 if !lan_sites.is_empty() {
  let ip=crate::network::lan_ipv4().ok_or_else(||"LAN websites are enabled, but no private IPv4 address is available. Connect to a private network and retry.".to_string())?;
  for (id,runtime,node_port,lan_port) in lan_sites {
   let Some(port)=lan_port else {pending+=1;continue;};
   if runtime=="node" || runtime=="php" || runtime=="python" {
    let Some(upstream)=node_port else {pending+=1;continue;};
    config.push_str(&format!("http://{ip}:{port} {{\n    bind {ip}\n    reverse_proxy 127.0.0.1:{upstream}\n}}\n\n"));
   } else {
    config.push_str(&format!("http://{ip}:{port} {{\n    bind {ip}\n    @root path /\n    rewrite @root /hosted/{id}/index.html\n    @assets not path /\n    rewrite @assets /hosted/{id}{{uri}}\n    reverse_proxy 127.0.0.1:47821\n}}\n\n"));
   }
   routes+=1;
  }
 }
 Ok((config,routes,pending))
}
fn caddy_output(state:&AppState,args:&[&str])->Result<String,String>{
 let output=Command::new(&state.caddy_bin).args(args).output().map_err(|e|format!("Caddy was not found. Install Caddy or set CADDY_BIN to its executable path: {e}"))?;
 let mut text=String::from_utf8_lossy(&output.stdout).to_string(); text.push_str(&String::from_utf8_lossy(&output.stderr));
 if !output.status.success(){return Err(format!("Caddy exited with {}: {}",output.status,text.trim()));}
 Ok(text.trim().to_string())
}
/// Regenerate and reload Caddy when it is already running. If a security-setting change
/// cannot be applied, stop the proxy rather than leave stale public routes active.
pub fn refresh_running_config(state:&AppState){
 let running=if let Ok(mut slot)=state.proxy_process.lock(){
  match slot.as_mut(){Some(child)=>match child.try_wait(){Ok(None)=>true,Ok(Some(_))=>{*slot=None;false},Err(_)=>true},None=>false}
 }else{false};
 if !running{return;}
 let result=validate_config(state).and_then(|_|{
  let path=config_path(state);let p=path.to_string_lossy().to_string();
  caddy_output(state,&["reload","--config",&p,"--adapter","caddyfile"]).map(|_|())
 });
 if result.is_err(){stop_on_shutdown(state);}
}

fn validate_config(state:&AppState)->Result<String,String>{
 let (config,_,_)=generate_config(state)?;
 let path=config_path(state); let temp=path.with_extension("candidate");
 fs::write(&temp,&config).map_err(|e|format!("Could not write candidate Caddyfile: {e}"))?;
 let candidate=temp.to_string_lossy().to_string();
 let output=match caddy_output(state,&["validate","--config",&candidate,"--adapter","caddyfile"]){Ok(output)=>output,Err(error)=>{let _=fs::remove_file(&temp);return Err(error);}};
 if path.exists(){let backup=path.with_extension("previous");fs::copy(&path,&backup).map_err(|e|format!("Could not preserve previous Caddyfile: {e}"))?;}
 fs::copy(&temp,&path).map_err(|e|format!("Could not activate validated Caddyfile: {e}"))?;
 let _=fs::remove_file(&temp);
 Ok(output)
}

async fn tls_check(State(state):State<AppState>,Path(hostname):Path<String>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 let registered={
  let db=state.db.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_locked","message":"Database lock is unavailable"}}))))?;
  db.query_row("SELECT EXISTS(SELECT 1 FROM public_domains WHERE hostname=?1)",[&hostname],|r|r.get::<_,bool>(0))
   .map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_error","message":e.to_string()}}))))?
 };
 if !registered{return Err((axum::http::StatusCode::NOT_FOUND,Json(json!({"error":{"code":"public_domain_not_found","message":"Only a hostname registered in Public Hosting can be checked."}}))));}
 let sink=if cfg!(windows){"NUL"}else{"/dev/null"};
 let output=Command::new("curl").args(["--head","--silent","--show-error","--connect-timeout","5","--max-time","15","--max-redirs","0","--output",sink,"--write-out","%{http_code}|%{ssl_verify_result}|%{time_total}"])
  .arg(format!("https://{hostname}/")).output()
  .map_err(|e|(axum::http::StatusCode::SERVICE_UNAVAILABLE,Json(json!({"error":{"code":"curl_unavailable","message":format!("HTTPS verification requires curl on PATH: {e}")}}))))?;
 let stdout=String::from_utf8_lossy(&output.stdout);
 let parts:Vec<&str>=stdout.trim().split('|').collect();
 let http_status=parts.first().and_then(|v|v.parse::<u16>().ok()).filter(|v|*v>0);
 let verify=parts.get(1).and_then(|v|v.parse::<i64>().ok());
 let elapsed=parts.get(2).and_then(|v|v.parse::<f64>().ok());
 let tls_valid=output.status.success()&&verify==Some(0);
 let error=if tls_valid{None}else{Some(String::from_utf8_lossy(&output.stderr).trim().to_string())};
 Ok(Json(json!({"hostname":hostname,"tls_valid":tls_valid,"http_status":http_status,"ssl_verify_result":verify,"elapsed_seconds":elapsed,"checked_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),"error":error,"method":"curl HEAD; certificate verification enabled; redirects not followed"})))
}

async fn tls_details(State(state):State<AppState>,Path(hostname):Path<String>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 let registered={
  let db=state.db.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_locked","message":"Database lock is unavailable"}}))))?;
  db.query_row("SELECT EXISTS(SELECT 1 FROM public_domains WHERE hostname=?1)",[&hostname],|r|r.get::<_,bool>(0))
   .map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_error","message":e.to_string()}}))))?
 };
 if !registered{return Err((axum::http::StatusCode::NOT_FOUND,Json(json!({"error":{"code":"public_domain_not_found","message":"Only a hostname registered in Public Hosting can be inspected."}}))));}
 if !cfg!(windows){return Err((axum::http::StatusCode::NOT_IMPLEMENTED,Json(json!({"error":{"code":"certificate_inspection_unavailable","message":"Certificate metadata inspection currently requires Windows PowerShell."}}))));}
 const SCRIPT:&str=r#"$ErrorActionPreference='Stop'; $hostName=$env:WEBDYNAMITE_TLS_HOST; $client=[System.Net.Sockets.TcpClient]::new(); $client.ReceiveTimeout=8000; $client.SendTimeout=8000; try { $task=$client.ConnectAsync($hostName,443); if(-not $task.Wait(8000)){ throw 'Connection timed out while contacting port 443.' }; $callback=[System.Net.Security.RemoteCertificateValidationCallback]{ param($sender,$certificate,$chain,$errors) return $true }; $stream=[System.Net.Security.SslStream]::new($client.GetStream(),$false,$callback); try { $stream.ReadTimeout=8000; $stream.WriteTimeout=8000; $stream.AuthenticateAsClient($hostName); $cert=[System.Security.Cryptography.X509Certificates.X509Certificate2]::new($stream.RemoteCertificate); [pscustomobject]@{ subject=$cert.Subject; issuer=$cert.Issuer; not_before=$cert.NotBefore.ToUniversalTime().ToString('o'); not_after=$cert.NotAfter.ToUniversalTime().ToString('o'); thumbprint=$cert.Thumbprint } | ConvertTo-Json -Compress } finally { $stream.Dispose() } } finally { $client.Dispose() }"#;
 let output=Command::new("powershell.exe").args(["-NoLogo","-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-Command",SCRIPT])
  .env("WEBDYNAMITE_TLS_HOST",&hostname).output().map_err(|e|(axum::http::StatusCode::SERVICE_UNAVAILABLE,Json(json!({"error":{"code":"powershell_unavailable","message":format!("Could not start PowerShell certificate inspection: {e}")}}))))?;
 if !output.status.success(){let detail=String::from_utf8_lossy(&output.stderr).trim().to_string();return Err((axum::http::StatusCode::BAD_GATEWAY,Json(json!({"error":{"code":"certificate_inspection_failed","message":if detail.is_empty(){"Could not retrieve the remote TLS certificate.".to_string()}else{detail}}}))));}
 let raw=String::from_utf8_lossy(&output.stdout);
 let cert:Value=serde_json::from_str(raw.trim()).map_err(|e|(axum::http::StatusCode::BAD_GATEWAY,Json(json!({"error":{"code":"certificate_parse_failed","message":format!("PowerShell returned invalid certificate metadata: {e}")}}))))?;
 let not_after=cert.get("not_after").and_then(Value::as_str).unwrap_or("");
 let expiry=chrono_free_parse_epoch(not_after);
 let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
 let days_remaining=expiry.map(|value|(value-now).div_euclid(86400));
 let subject=cert.get("subject").and_then(Value::as_str).unwrap_or("");
 let issuer=cert.get("issuer").and_then(Value::as_str).unwrap_or("");
 let not_before=cert.get("not_before").and_then(Value::as_str).unwrap_or("");
 let thumbprint=cert.get("thumbprint").and_then(Value::as_str).unwrap_or("");
 {
  let db=state.db.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_locked","message":"Database lock is unavailable"}}))))?;
  db.execute("INSERT INTO tls_certificate_history(hostname,subject,issuer,not_before,not_after,thumbprint,checked_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",rusqlite::params![hostname,subject,issuer,not_before,not_after,thumbprint,now])
   .map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"history_write_failed","message":e.to_string()}}))))?;
  db.execute("DELETE FROM tls_certificate_history WHERE hostname=?1 AND id NOT IN (SELECT id FROM tls_certificate_history WHERE hostname=?1 ORDER BY checked_at DESC,id DESC LIMIT 30)",[&hostname])
   .map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"history_retention_failed","message":e.to_string()}}))))?;
 }
 Ok(Json(json!({"hostname":hostname,"subject":subject,"issuer":issuer,"not_before":not_before,"not_after":not_after,"days_remaining":days_remaining,"thumbprint":thumbprint,"expired":days_remaining.is_some_and(|days|days<0),"checked_at":now,"note":"Certificate metadata is inspected without validating trust. Use Check HTTPS to verify the certificate chain and hostname."})))
}
async fn tls_history(State(state):State<AppState>,Path(hostname):Path<String>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 let db=state.db.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_locked","message":"Database lock is unavailable"}}))))?;
 let registered:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM public_domains WHERE hostname=?1)",[&hostname],|r|r.get(0))
  .map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"database_error","message":e.to_string()}}))))?;
 if !registered{return Err((axum::http::StatusCode::NOT_FOUND,Json(json!({"error":{"code":"public_domain_not_found","message":"Only a registered public hostname has certificate history."}}))));}
 let mut statement=db.prepare("SELECT subject,issuer,not_before,not_after,thumbprint,checked_at FROM tls_certificate_history WHERE hostname=?1 ORDER BY checked_at DESC,id DESC LIMIT 30")
  .map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"history_query_failed","message":e.to_string()}}))))?;
 let rows=statement.query_map([&hostname],|r|Ok(json!({"subject":r.get::<_,String>(0)?,"issuer":r.get::<_,String>(1)?,"not_before":r.get::<_,String>(2)?,"not_after":r.get::<_,String>(3)?,"thumbprint":r.get::<_,String>(4)?,"checked_at":r.get::<_,i64>(5)?})))
  .map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"history_query_failed","message":e.to_string()}}))))?;
 let mut history=Vec::new(); for row in rows {history.push(row.map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"history_query_failed","message":e.to_string()}}))))?);}
 Ok(Json(json!({"hostname":hostname,"count":history.len(),"history":history,"retention_per_hostname":30})))
}
fn chrono_free_parse_epoch(value:&str)->Option<i64>{
 let normalized=value.trim_end_matches('Z');
 let (date,time)=normalized.split_once('T')?;
 let mut date_parts=date.split('-');
 let year=date_parts.next()?.parse::<i64>().ok()?;
 let month=date_parts.next()?.parse::<i64>().ok()?;
 let day=date_parts.next()?.parse::<i64>().ok()?;
 let mut time_parts=time.split(':');
 let hour=time_parts.next()?.parse::<i64>().ok()?;
 let minute=time_parts.next()?.parse::<i64>().ok()?;
 let second=time_parts.next()?.split('.').next()?.parse::<i64>().ok()?;
 let adjusted_year=year-if month<=2{1}else{0};
 let era=adjusted_year.div_euclid(400);
 let year_of_era=adjusted_year-era*400;
 let adjusted_month=month+if month>2{-3}else{9};
 let day_of_year=(153*adjusted_month+2).div_euclid(5)+day-1;
 let day_of_era=year_of_era*365+year_of_era.div_euclid(4)-year_of_era.div_euclid(100)+day_of_year;
 Some((era*146097+day_of_era-719468)*86400+hour*3600+minute*60+second)
}

async fn status(State(state):State<AppState>)->Json<Value>{
 let installed=Command::new(&state.caddy_bin).arg("version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|result|result.success()).unwrap_or(false);
 let running=if let Ok(mut slot)=state.proxy_process.lock(){let exited=match slot.as_mut(){Some(child)=>matches!(child.try_wait(),Ok(Some(_))),None=>false};if exited{*slot=None;false}else{slot.is_some()}}else{false};
 let admin_ready=tokio::task::spawn_blocking(caddy_admin_ready).await.unwrap_or(false);
 Json(json!({"installed":installed,"running":running,"admin_ready":admin_ready,"binary":state.caddy_bin,"config_path":config_path(&state),"log_path":caddy_log_path(&state),"admin_endpoint":"http://127.0.0.1:2019","note":"Admin readiness confirms only that Caddy's local admin socket accepts TCP connections. Public exposure and HTTPS still require DNS, firewall and domain configuration."}))
}
async fn validate(State(state):State<AppState>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 match validate_config(&state){Ok(output)=>Ok(Json(json!({"valid":true,"output":output,"config_path":config_path(&state)}))),Err(e)=>Err((axum::http::StatusCode::BAD_REQUEST,Json(json!({"error":{"code":"caddy_validation_failed","message":e}}))))}
}
async fn start(State(state):State<AppState>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 let (mut response,started_here)=start_sync(state.clone())?;
 let log_path=caddy_log_path(&state);
 for _ in 0..40 {
  if tokio::task::spawn_blocking(caddy_admin_ready).await.unwrap_or(false) {
   response["ready"]=json!(true);
   response["admin_ready"]=json!(true);
   response["log_path"]=json!(log_path);
   return Ok(response);
  }
  let exited={
   let mut slot=state.proxy_process.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"proxy_lock","message":"Caddy process registry is unavailable"}}))))?;
   match slot.as_mut() {
    Some(child)=>match child.try_wait(){
     Ok(Some(status))=>{*slot=None;Some(status.to_string())},
     Ok(None)=>None,
     Err(error)=>return Err((axum::http::StatusCode::BAD_GATEWAY,Json(json!({"error":{"code":"caddy_status_failed","message":format!("Could not inspect the Caddy process: {error}. Logs: {}",log_path.display())}})))),
    },
    None=>Some("the process handle is unavailable".to_string()),
   }
  };
  if let Some(status)=exited {
   return Err((axum::http::StatusCode::BAD_GATEWAY,Json(json!({"error":{"code":"caddy_exited_during_startup","message":format!("Caddy exited before its local admin endpoint became ready ({status}). Review {}",log_path.display())}}))));
  }
  tokio::time::sleep(Duration::from_millis(125)).await;
 }
 if started_here {
  if let Ok(mut slot)=state.proxy_process.lock(){if let Some(mut child)=slot.take(){let _=child.kill();let _=child.wait();}}
 }
 Err((axum::http::StatusCode::GATEWAY_TIMEOUT,Json(json!({"error":{"code":"caddy_start_timeout","message":format!("Caddy did not open its local admin endpoint at 127.0.0.1:2019 within 5 seconds. {}. Review the Caddy log.",if started_here{"The newly started process was stopped."}else{"The existing process was left running."})},"log_path":log_path}))))
}
fn start_sync(state:AppState)->Result<(Json<Value>,bool),(axum::http::StatusCode,Json<Value>)>{
 if let Err(e)=validate_config(&state){return Err((axum::http::StatusCode::BAD_REQUEST,Json(json!({"error":{"code":"caddy_validation_failed","message":e}}))));}
 let mut slot=state.proxy_process.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"proxy_lock","message":"Caddy process registry is unavailable"}}))))?;
 let already_running=match slot.as_mut(){Some(child)=>match child.try_wait(){Ok(None)=>true,Ok(Some(_))=>false,Err(_)=>true},None=>false};
 if already_running {let pid=slot.as_ref().map(|process|process.id()).unwrap_or_default();return Ok((Json(json!({"running":true,"pid":pid,"message":"Caddy is already running"})),false));}
 if slot.is_some(){*slot=None;}
 fs::create_dir_all(&state.logs_dir).map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"caddy_log_directory_failed","message":format!("Could not create Caddy log directory: {e}")}}))))?;
 let log_path=caddy_log_path(&state);
 let log=OpenOptions::new().create(true).append(true).open(&log_path).map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"caddy_log_open_failed","message":format!("Could not open Caddy log {}: {e}",log_path.display())}}))))?;
 let stderr=log.try_clone().map_err(|e|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"caddy_log_clone_failed","message":format!("Could not prepare Caddy stderr log: {e}")}}))))?;
 let path=config_path(&state);
 let child=Command::new(&state.caddy_bin).args(["run","--config"]).arg(&path).args(["--adapter","caddyfile"]).stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(stderr)).spawn().map_err(|e|(axum::http::StatusCode::BAD_REQUEST,Json(json!({"error":{"code":"caddy_start_failed","message":format!("Could not start Caddy: {e}")}}))))?;
 let pid=child.id(); *slot=Some(child); drop(slot);
 Ok((Json(json!({"running":true,"pid":pid,"config_path":path,"log_path":log_path,"message":"Caddy process started; waiting for its local admin endpoint"})),true))
}
async fn stop(State(state):State<AppState>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 if state.proxy_process.lock().map(|slot|slot.is_some()).unwrap_or(false) { crate::hosting_supervisor::suppress_restart(&state,"caddy"); }
 let mut slot=state.proxy_process.lock().map_err(|_|(axum::http::StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":{"code":"proxy_lock","message":"Caddy process registry is unavailable"}}))))?;
 if let Some(mut child)=slot.take(){let _=child.kill();let _=child.wait();}
 Ok(Json(json!({"running":false,"message":"Caddy stopped"})))
}
async fn reload(State(state):State<AppState>)->Result<Json<Value>,(axum::http::StatusCode,Json<Value>)>{
 if let Err(e)=validate_config(&state){return Err((axum::http::StatusCode::BAD_REQUEST,Json(json!({"error":{"code":"caddy_config_failed","message":e}}))));}
 let path=config_path(&state);let p=path.to_string_lossy().to_string();
 match caddy_output(&state,&["reload","--config",&p,"--adapter","caddyfile"]){Ok(output)=>Ok(Json(json!({"running":true,"reloaded":true,"output":output}))),Err(e)=>Err((axum::http::StatusCode::BAD_REQUEST,Json(json!({"error":{"code":"caddy_reload_failed","message":e}}))))}
}
pub fn stop_on_shutdown(state:&AppState){if state.proxy_process.lock().map(|slot|slot.is_some()).unwrap_or(false){crate::hosting_supervisor::suppress_restart(state,"caddy");}if let Ok(mut slot)=state.proxy_process.lock(){if let Some(mut child)=slot.take(){let _=child.kill();let _=child.wait();}}}

/// Restart Caddy through the same validation and admin-readiness gate used by the API.
pub(crate) async fn restart_for_supervisor(state:AppState)->bool{
 if !crate::settings::setting_bool(&state,"hosting_auto_restart",false){return false;}
 let Ok((_response,started_here))=start_sync(state.clone()) else{return false;};
 for _ in 0..40 {
  if tokio::task::spawn_blocking(caddy_admin_ready).await.unwrap_or(false){return true;}
  let exited=state.proxy_process.lock().ok().and_then(|mut slot|match slot.as_mut(){Some(child)=>child.try_wait().ok().map(|status|status.is_some()),None=>Some(true)}).unwrap_or(true);
  if exited{return false;}
  tokio::time::sleep(Duration::from_millis(125)).await;
 }
 if started_here {if let Ok(mut slot)=state.proxy_process.lock(){if let Some(mut child)=slot.take(){let _=child.kill();let _=child.wait();}}}
 false
}

#[cfg(test)]
mod certificate_time_tests {
    use super::chrono_free_parse_epoch;

    #[test]
    fn parses_utc_certificate_expiry_timestamps() {
        assert_eq!(chrono_free_parse_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(chrono_free_parse_epoch("2000-02-29T12:34:56.0000000Z"), Some(951_827_696));
        assert_eq!(chrono_free_parse_epoch("not-a-date"), None);
    }
}
