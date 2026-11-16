use axum::{
    body::Body,
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs::File, io::Read, path::{Path as FsPath, PathBuf}, process::{Command, Stdio}};
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, Serialize)]
struct Website {
    id: String, name: String, project_path: String, runtime: String,
    status: String, created_at: String, updated_at: String,
}
#[derive(Debug, Deserialize)]
struct CreateWebsite {
    name: String, project_path: String, runtime: String,
}

#[derive(Debug, Serialize)]
struct Domain { id: String, website_id: String, website_name: String, hostname: String, access_mode: String, created_at: String }
#[derive(Debug, Deserialize)]
struct CreateDomain { website_id: String, hostname: String }
fn valid_local_hostname(value: &str) -> bool {
    let host = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if !(host.ends_with(".localhost") || host.ends_with(".test")) || host.len() > 253 { return false; }
    host.split('.').all(|label| !label.is_empty() && label.len() <= 63
        && label.as_bytes()[0].is_ascii_alphanumeric()
        && label.as_bytes()[label.len()-1].is_ascii_alphanumeric()
        && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
}
#[derive(Debug)]
struct ApiError(StatusCode, &'static str, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":{"code":self.1,"message":self.2}}))).into_response()
    }
}
fn database_error(error: rusqlite::Error) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database_error", error.to_string())
}
async fn require_bearer(State(token): State<String>, request: Request<Body>, next: Next) -> Response {
    let expected = format!("Bearer {token}");
    let supplied = request.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    if supplied != Some(expected.as_str()) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":{"code":"unauthorized","message":"A valid bearer token is required."}}))).into_response();
    }
    next.run(request).await
}
pub fn router(state: AppState, token: String) -> Router {
    let protected = Router::new()
        .route("/api/v1/websites", get(list_websites).post(create_website))
        .route("/api/v1/websites/:id", get(get_website).delete(delete_website))
        .route("/api/v1/websites/:id/start", axum::routing::post(start_website))
        .route("/api/v1/websites/:id/stop", axum::routing::post(stop_website))
        .route("/api/v1/websites/:id/restart", axum::routing::post(restart_website))
        .route("/api/v1/websites/:id/logs", get(website_logs))
        .route("/api/v1/domains", get(list_domains).post(create_domain))
        .route("/api/v1/domains/:id", axum::routing::delete(delete_domain))
        .route("/api/v1/domains/caddyfile", get(export_caddyfile))
        .layer(middleware::from_fn_with_state(token, require_bearer));
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/hosted/:id/*path", get(serve_static))
        .merge(protected)
        .with_state(state)
}

async fn list_domains(State(state): State<AppState>) -> Result<Json<Vec<Domain>>, ApiError> {
    let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let mut q = db.prepare("SELECT d.id,d.website_id,w.name,d.hostname,d.access_mode,d.created_at FROM domains d JOIN websites w ON w.id=d.website_id ORDER BY d.hostname").map_err(database_error)?;
    let rows = q.query_map([], |r| Ok(Domain { id:r.get(0)?, website_id:r.get(1)?, website_name:r.get(2)?, hostname:r.get(3)?, access_mode:r.get(4)?, created_at:r.get(5)? })).map_err(database_error)?;
    Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(database_error)?))
}
async fn create_domain(State(state): State<AppState>, Json(input): Json<CreateDomain>) -> Result<(StatusCode,Json<Domain>),ApiError> {
    let hostname=input.hostname.trim().trim_end_matches('.').to_ascii_lowercase();
    if !valid_local_hostname(&hostname) {
        return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_local_hostname","Use a valid local hostname ending in .localhost or .test (for example, shop.localhost).".into()));
    }
    let id=Uuid::new_v4().to_string();
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let runtime: String=db.query_row("SELECT runtime FROM websites WHERE id=?1",params![input.website_id],|r|r.get(0))
        .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"website_not_found","Select an existing website.".into())}else{database_error(e)})?;
    if runtime!="static" { return Err(ApiError(StatusCode::NOT_IMPLEMENTED,"runtime_domain_unavailable","Local domain routing currently supports static websites. Node.js domain routing will be enabled with per-site port allocation.".into())); }
    if db.query_row("SELECT EXISTS(SELECT 1 FROM domains WHERE hostname=?1)",params![hostname],|r|r.get::<_,bool>(0)).map_err(database_error)? {
        return Err(ApiError(StatusCode::CONFLICT,"hostname_in_use","That hostname is already assigned.".into()));
    }
    db.execute("INSERT INTO domains(id,website_id,hostname) VALUES (?1,?2,?3)",params![id,input.website_id,hostname]).map_err(database_error)?;
    let domain=db.query_row("SELECT d.id,d.website_id,w.name,d.hostname,d.access_mode,d.created_at FROM domains d JOIN websites w ON w.id=d.website_id WHERE d.id=?1",params![id],|r|Ok(Domain{id:r.get(0)?,website_id:r.get(1)?,website_name:r.get(2)?,hostname:r.get(3)?,access_mode:r.get(4)?,created_at:r.get(5)?})).map_err(database_error)?;
    Ok((StatusCode::CREATED,Json(domain)))
}
async fn delete_domain(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode,ApiError> {
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let n=db.execute("DELETE FROM domains WHERE id=?1",params![id]).map_err(database_error)?;
    if n==0 { return Err(ApiError(StatusCode::NOT_FOUND,"domain_not_found","No domain exists with that ID.".into())); }
    Ok(StatusCode::NO_CONTENT)
}
async fn export_caddyfile(State(state): State<AppState>) -> Result<Json<serde_json::Value>,ApiError> {
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let mut q=db.prepare("SELECT d.hostname,d.website_id FROM domains d JOIN websites w ON w.id=d.website_id WHERE w.runtime='static' ORDER BY d.hostname").map_err(database_error)?;
    let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(database_error)?;
    let mut config=String::from("# Generated by WebDynamite. Do not edit this file manually.\n");
    let mut count=0usize;
    for row in rows {
        let (host,id)=row.map_err(database_error)?;
        config.push_str(&format!("{host} {{\n    rewrite * /hosted/{id}{{uri}}\n    reverse_proxy 127.0.0.1:47821\n}}\n\n"));
        count+=1;
    }
    Ok(Json(json!({"config":config,"routes":count,"status":"generated_not_applied","note":"Caddy must be installed and started separately. This endpoint only exports a validated Caddyfile; WebDynamite does not yet modify the Windows hosts file or control a Caddy process."})))
}
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok","service":"webdynamite-core","api_version":1}))
}
async fn list_websites(State(state): State<AppState>) -> Result<Json<Vec<Website>>, ApiError> {
    let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let mut statement = db.prepare("SELECT id,name,project_path,runtime,status,created_at,updated_at FROM websites ORDER BY created_at DESC").map_err(database_error)?;
    let rows = statement.query_map([], |r| Ok(Website {
        id:r.get(0)?, name:r.get(1)?, project_path:r.get(2)?, runtime:r.get(3)?,
        status:r.get(4)?, created_at:r.get(5)?, updated_at:r.get(6)?,
    })).map_err(database_error)?;
    Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(database_error)?))
}
async fn create_website(State(state): State<AppState>, Json(input): Json<CreateWebsite>) -> Result<(StatusCode,Json<Website>),ApiError> {
    let name=input.name.trim(); let path=input.project_path.trim();
    if name.is_empty() || name.len()>80 { return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_name","Website name must contain 1 to 80 characters.".into())); }
    if path.is_empty() || path.len()>1024 || !FsPath::new(path).is_absolute() { return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_project_path","Project path must be an absolute path containing 1 to 1024 characters.".into())); }
    if !FsPath::new(path).is_dir() { return Err(ApiError(StatusCode::BAD_REQUEST,"project_not_found","The selected project directory does not exist.".into())); }
    let runtime=input.runtime.to_ascii_lowercase();
    if !["static","node","php","python"].contains(&runtime.as_str()) { return Err(ApiError(StatusCode::BAD_REQUEST,"unsupported_runtime","Runtime must be static, node, php or python.".into())); }
    let id=Uuid::new_v4().to_string();
    let db=state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    db.execute("INSERT INTO websites(id,name,project_path,runtime) VALUES (?1,?2,?3,?4)",params![id,name,path,runtime]).map_err(database_error)?;
    let website=db.query_row("SELECT id,name,project_path,runtime,status,created_at,updated_at FROM websites WHERE id=?1",params![id],|r| Ok(Website{
        id:r.get(0)?,name:r.get(1)?,project_path:r.get(2)?,runtime:r.get(3)?,status:r.get(4)?,created_at:r.get(5)?,updated_at:r.get(6)?,
    })).map_err(database_error)?;
    Ok((StatusCode::CREATED,Json(website)))
}
async fn get_website(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<Website>,ApiError>{
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    match db.query_row("SELECT id,name,project_path,runtime,status,created_at,updated_at FROM websites WHERE id=?1",params![id],|r|Ok(Website{
        id:r.get(0)?,name:r.get(1)?,project_path:r.get(2)?,runtime:r.get(3)?,status:r.get(4)?,created_at:r.get(5)?,updated_at:r.get(6)?,
    })){
        Ok(w)=>Ok(Json(w)),Err(rusqlite::Error::QueryReturnedNoRows)=>Err(ApiError(StatusCode::NOT_FOUND,"website_not_found","No website exists with that ID.".into())),Err(e)=>Err(database_error(e))
    }
}
async fn delete_website(State(state):State<AppState>,Path(id):Path<String>)->Result<StatusCode,ApiError>{
    stop_process(&state,&id)?;
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let n=db.execute("DELETE FROM websites WHERE id=?1",params![id]).map_err(database_error)?;
    if n==0{return Err(ApiError(StatusCode::NOT_FOUND,"website_not_found","No website exists with that ID.".into()));}
    Ok(StatusCode::NO_CONTENT)
}
fn project_details(state:&AppState,id:&str)->Result<(String,String),ApiError>{
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    db.query_row("SELECT project_path,runtime FROM websites WHERE id=?1",params![id],|r|Ok((r.get(0)?,r.get(1)?)))
      .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"website_not_found","No website exists with that ID.".into())}else{database_error(e)})
}
fn set_status(state:&AppState,id:&str,status:&str)->Result<(),ApiError>{
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let n=db.execute("UPDATE websites SET status=?1,updated_at=CURRENT_TIMESTAMP WHERE id=?2",params![status,id]).map_err(database_error)?;
    if n==0{return Err(ApiError(StatusCode::NOT_FOUND,"website_not_found","No website exists with that ID.".into()));} Ok(())
}
fn stop_process(state:&AppState,id:&str)->Result<(),ApiError>{
    let mut map=state.processes.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"process_registry_locked","Process registry is unavailable.".into()))?;
    if let Some(mut child)=map.remove(id){
        let pid=child.id();
        #[cfg(windows)]
        { let _=Command::new("taskkill").args(["/PID",&pid.to_string(),"/T","/F"]).output(); }
        #[cfg(not(windows))]
        { let _=child.kill(); }
        let _=child.wait();
    }
    Ok(())
}
async fn start_website(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let (project,runtime)=project_details(&state,&id)?;
    if !FsPath::new(&project).is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"project_not_found","The project directory is no longer available.".into()));}
    stop_process(&state,&id)?;
    if runtime=="static" {
        if !FsPath::new(&project).join("index.html").is_file(){return Err(ApiError(StatusCode::BAD_REQUEST,"index_missing","Static projects must contain an index.html file.".into()));}
        set_status(&state,&id,"running")?;
        return Ok(Json(json!({"id":id,"status":"running","url":format!("http://127.0.0.1:47821/hosted/{id}/index.html")})));
    }
    if runtime!="node"{return Err(ApiError(StatusCode::NOT_IMPLEMENTED,"runtime_unavailable","This runtime adapter is not implemented yet. Static and Node.js projects are supported in this build.".into()));}
    let log_path=state.logs_dir.join(format!("{id}.log"));
    let stdout=File::create(&log_path).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_file_error",e.to_string()))?;
    let stderr=stdout.try_clone().map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_file_error",e.to_string()))?;
    #[cfg(windows)]
    let mut command={let mut c=Command::new("cmd");c.args(["/C","npm run start"]);c};
    #[cfg(not(windows))]
    let mut command={let mut c=Command::new("npm");c.args(["run","start"]);c};
    let child=command.current_dir(&project).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr)).spawn()
      .map_err(|e|ApiError(StatusCode::BAD_REQUEST,"runtime_start_failed",format!("Could not start npm. Verify Node.js and npm are installed: {e}")))?;
    let pid=child.id();
    state.processes.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"process_registry_locked","Process registry is unavailable.".into()))?.insert(id.clone(),child);
    set_status(&state,&id,"running")?;
    Ok(Json(json!({"id":id,"status":"running","pid":pid,"log_path":log_path})))
}
async fn stop_website(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let _=project_details(&state,&id)?; stop_process(&state,&id)?; set_status(&state,&id,"stopped")?;
    Ok(Json(json!({"id":id,"status":"stopped"})))
}
async fn restart_website(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let _=project_details(&state,&id)?; stop_process(&state,&id)?; start_website(State(state),Path(id)).await
}
async fn website_logs(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let _=project_details(&state,&id)?;
    let path=state.logs_dir.join(format!("{id}.log"));
    if !path.exists(){return Ok(Json(json!({"id":id,"logs":"","message":"No process log has been created yet."})));}
    let mut file=File::open(path).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_read_error",e.to_string()))?;
    let mut data=String::new(); file.read_to_string(&mut data).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_read_error",e.to_string()))?;
    let tail: String = data.chars().rev().take(24000).collect::<String>().chars().rev().collect();
    Ok(Json(json!({"id":id,"logs":tail})))
}
async fn serve_static(State(state):State<AppState>,Path((id,relative)):Path<(String,String)>)->Response{
    let project=match project_details(&state,&id){Ok((p,r)) if r=="static"=>p,_=>return StatusCode::NOT_FOUND.into_response()};
    let is_running=state.db.lock().ok().and_then(|db|db.query_row("SELECT status FROM websites WHERE id=?1",params![id],|row|row.get::<_,String>(0)).ok()).map(|s|s=="running").unwrap_or(false);
    if !is_running{return StatusCode::NOT_FOUND.into_response();}
    let root=match std::fs::canonicalize(&project){Ok(p)=>p,Err(_)=>return StatusCode::NOT_FOUND.into_response()};
    let requested=match std::fs::canonicalize(root.join(relative.trim_start_matches('/'))){Ok(p)=>p,Err(_)=>return StatusCode::NOT_FOUND.into_response()};
    if !requested.starts_with(&root)||!requested.is_file(){return StatusCode::NOT_FOUND.into_response();}
    let bytes=match std::fs::read(&requested){Ok(b)=>b,Err(_)=>return StatusCode::INTERNAL_SERVER_ERROR.into_response()};
    let mime=mime_type(&requested);
    Response::builder().status(StatusCode::OK).header("content-type",HeaderValue::from_static(mime)).body(Body::from(bytes)).unwrap_or_else(|_|StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
fn mime_type(path:&PathBuf)->&'static str{
    match path.extension().and_then(|x|x.to_str()).unwrap_or("").to_ascii_lowercase().as_str(){
        "html"|"htm"=>"text/html; charset=utf-8","css"=>"text/css; charset=utf-8","js"=>"text/javascript; charset=utf-8",
        "json"=>"application/json","svg"=>"image/svg+xml","png"=>"image/png","jpg"|"jpeg"=>"image/jpeg","gif"=>"image/gif",
        "webp"=>"image/webp","ico"=>"image/x-icon","txt"=>"text/plain; charset=utf-8","woff"=>"font/woff","woff2"=>"font/woff2",
        _=>"application/octet-stream"
    }
}

#[cfg(test)]
mod domain_tests {
    use super::valid_local_hostname;

    #[test]
    fn accepts_reserved_local_suffixes() {
        assert!(valid_local_hostname("portfolio.localhost"));
        assert!(valid_local_hostname("shop.example.test"));
        assert!(valid_local_hostname("my-site.localhost"));
    }

    #[test]
    fn rejects_public_or_malformed_hostnames() {
        assert!(!valid_local_hostname("example.com"));
        assert!(!valid_local_hostname("localhost"));
        assert!(!valid_local_hostname("-bad.localhost"));
        assert!(!valid_local_hostname("bad-.localhost"));
        assert!(!valid_local_hostname("bad name.localhost"));
        assert!(!valid_local_hostname("a..localhost"));
    }
}
