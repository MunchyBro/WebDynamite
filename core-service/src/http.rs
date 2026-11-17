use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header::AUTHORIZATION, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs::File, io::Read, path::{Path as FsPath, PathBuf}, process::{Command, Stdio}, net::TcpListener};
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
#[derive(Debug, Deserialize)]
struct SetLanAccess { enabled: bool }
#[derive(Debug, Deserialize)]
struct DiagnoseDomain { hostname: String, expected_ipv4: Option<String> }
#[derive(Debug, Serialize)]
struct PublicDomain { id:String, website_id:String, website_name:String, hostname:String, runtime:String, port:Option<u16>, created_at:String }
#[derive(Debug, Deserialize)]
struct CreatePublicDomain { website_id:String, hostname:String }
fn valid_public_hostname(value:&str)->bool {
    let host=value.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.len()>253 || !host.contains('.') || host.ends_with(".localhost") || host.ends_with(".test") || host.parse::<std::net::IpAddr>().is_ok(){return false;}
    host.split('.').all(|label|!label.is_empty()&&label.len()<=63&&label.as_bytes()[0].is_ascii_alphanumeric()
        &&label.as_bytes()[label.len()-1].is_ascii_alphanumeric()
        &&label.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'-'))
}
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
        .route("/api/v1/websites/:id/files", get(list_website_files))
        .route("/api/v1/websites/:id/file", get(read_website_file).put(write_website_file).delete(delete_website_file))
        .route("/api/v1/domains", get(list_domains).post(create_domain))
        .route("/api/v1/domains/diagnose", axum::routing::post(diagnose_domain))
        .route("/api/v1/public-domains", get(list_public_domains).post(create_public_domain))
        .route("/api/v1/public-domains/:id", axum::routing::delete(delete_public_domain))
        .route("/api/v1/domains/:id", axum::routing::delete(delete_domain))
        .route("/api/v1/domains/caddyfile", get(export_caddyfile))
        .route("/api/v1/networking/status", get(networking_status))
        .route("/api/v1/monitoring/metrics", get(system_metrics))
        .route("/api/v1/databases", get(list_databases).post(create_database))
        .route("/api/v1/databases/:id", axum::routing::delete(delete_database))
        .route("/api/v1/databases/:id/test", axum::routing::post(test_database))
        .route("/api/v1/websites/:id/lan", axum::routing::put(set_lan_access))
        .merge(crate::proxy::router())
        .merge(crate::deploy::router())
        .merge(crate::git::router())
        .merge(crate::backup::router())
        .merge(crate::settings::router())
        .layer(middleware::from_fn_with_state(token, require_bearer));
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/hosted/:id/*path", get(serve_static))
        .merge(protected)
        .with_state(state)
}


#[derive(Debug, Deserialize)]
struct FilePathQuery { path: Option<String> }
#[derive(Debug, Deserialize)]
struct WriteWebsiteFile { path: String, content: String }

fn website_root(state:&AppState,id:&str)->Result<PathBuf,ApiError>{
    let project:String={
        let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
        db.query_row("SELECT project_path FROM websites WHERE id=?1",[id],|r|r.get(0))
            .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into())}else{database_error(e)})?
    };
    let root=FsPath::new(&project).canonicalize()
        .map_err(|e|ApiError(StatusCode::NOT_FOUND,"website_folder_unavailable",format!("Website folder is unavailable: {e}")))?;
    if !root.is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"website_root_not_directory","Website project path is not a directory.".into()));}
    Ok(root)
}
fn safe_relative_path(value:&str)->Result<PathBuf,ApiError>{
    let path=FsPath::new(value);
    if path.is_absolute() || path.components().any(|c|!matches!(c,std::path::Component::Normal(_))) {
        return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_file_path","Use a relative path inside the website folder.".into()));
    }
    Ok(path.to_path_buf())
}
fn contained_existing(root:&FsPath,relative:&str)->Result<PathBuf,ApiError>{
    let rel=safe_relative_path(relative)?;
    let mut current=root.to_path_buf();
    for component in rel.components(){
        if let std::path::Component::Normal(part)=component {
            current.push(part);
            let metadata=std::fs::symlink_metadata(&current)
                .map_err(|e|ApiError(StatusCode::NOT_FOUND,"file_not_found",format!("File or folder was not found: {e}")))?;
            if metadata.file_type().is_symlink(){
                return Err(ApiError(StatusCode::FORBIDDEN,"symbolic_link_forbidden","Symbolic links are not followed by the File Manager.".into()));
            }
        }
    }
    let target=current.canonicalize()
        .map_err(|e|ApiError(StatusCode::NOT_FOUND,"file_not_found",format!("File or folder was not found: {e}")))?;
    if !target.starts_with(root){return Err(ApiError(StatusCode::FORBIDDEN,"file_path_forbidden","The requested path escapes the website folder.".into()));}
    Ok(target)
}
async fn list_website_files(State(state):State<AppState>,Path(id):Path<String>,Query(query):Query<FilePathQuery>)->Result<Json<serde_json::Value>,ApiError>{
    let root=website_root(&state,&id)?;
    let relative=query.path.as_deref().unwrap_or("");
    let directory=if relative.is_empty(){root.clone()}else{contained_existing(&root,relative)?};
    if !directory.is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"not_a_directory","The requested path is not a directory.".into()));}
    let mut entries=Vec::new();
    for entry in std::fs::read_dir(&directory).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"directory_read_failed",e.to_string()))?{
        let entry=entry.map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"directory_read_failed",e.to_string()))?;
        let kind=entry.file_type().map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_metadata_failed",e.to_string()))?;
        let name=entry.file_name().to_string_lossy().to_string();
        if name=="."||name==".."{continue;}
        let metadata=entry.metadata().ok();
        entries.push(json!({"name":name,"kind":if kind.is_symlink(){"symlink"}else if kind.is_dir(){"directory"}else if kind.is_file(){"file"}else{"other"},"size_bytes":if kind.is_file(){metadata.map(|m|m.len()).unwrap_or(0)}else{0}}));
    }
    entries.sort_by(|a,b|{
        let ak=a["kind"].as_str().unwrap_or("");let bk=b["kind"].as_str().unwrap_or("");
        (ak!="directory").cmp(&(bk!="directory")).then_with(||a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()))
    });
    Ok(Json(json!({"website_id":id,"path":relative,"entries":entries})))
}
async fn read_website_file(State(state):State<AppState>,Path(id):Path<String>,Query(query):Query<FilePathQuery>)->Result<Json<serde_json::Value>,ApiError>{
    let root=website_root(&state,&id)?;
    let relative=query.path.as_deref().unwrap_or("");
    let target=contained_existing(&root,relative)?;
    if !target.is_file(){return Err(ApiError(StatusCode::BAD_REQUEST,"not_a_file","The requested path is not a file.".into()));}
    let metadata=std::fs::metadata(&target).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_metadata_failed",e.to_string()))?;
    if metadata.len()>1_048_576{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"file_too_large","Text editor supports files up to 1 MiB.".into()));}
    let content=std::fs::read_to_string(&target).map_err(|e|ApiError(StatusCode::UNSUPPORTED_MEDIA_TYPE,"file_not_text","This file is not valid UTF-8 text and cannot be opened in the text editor.".into()))?;
    Ok(Json(json!({"website_id":id,"path":relative,"content":content,"size_bytes":metadata.len()})))
}
async fn write_website_file(State(state):State<AppState>,Path(id):Path<String>,Json(input):Json<WriteWebsiteFile>)->Result<Json<serde_json::Value>,ApiError>{
    if input.content.len()>1_048_576{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"file_too_large","Text editor supports files up to 1 MiB.".into()));}
    let root=website_root(&state,&id)?;
    let relative=safe_relative_path(&input.path)?;
    if relative.as_os_str().is_empty(){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_file_path","A file name is required.".into()));}
    let parent_relative=relative.parent().unwrap_or_else(||FsPath::new(""));
    let parent_text=parent_relative.to_string_lossy();
    let canonical_parent=if parent_text.is_empty(){root.clone()}else{contained_existing(&root,&parent_text)?};
    if !canonical_parent.is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"parent_not_directory","The parent path is not a directory.".into()));}
    let file_name=relative.file_name().ok_or_else(||ApiError(StatusCode::BAD_REQUEST,"invalid_file_path","A file name is required.".into()))?;
    let target=canonical_parent.join(file_name);
    if let Ok(metadata)=std::fs::symlink_metadata(&target){
        if metadata.file_type().is_symlink()||!metadata.is_file(){return Err(ApiError(StatusCode::FORBIDDEN,"file_path_forbidden","The target must be a regular file, not a symbolic link or directory.".into()));}
    }
    std::fs::write(&target,input.content.as_bytes()).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_write_failed",e.to_string()))?;
    Ok(Json(json!({"website_id":id,"path":input.path,"size_bytes":input.content.len(),"saved":true})))
}
async fn delete_website_file(State(state):State<AppState>,Path(id):Path<String>,Query(query):Query<FilePathQuery>)->Result<StatusCode,ApiError>{
    let root=website_root(&state,&id)?;
    let relative=query.path.as_deref().unwrap_or("");
    let target=contained_existing(&root,relative)?;
    if target==root{return Err(ApiError(StatusCode::FORBIDDEN,"cannot_delete_website_root","The website root cannot be deleted.".into()));}
    if target.is_dir(){std::fs::remove_dir(&target).map_err(|e|ApiError(StatusCode::CONFLICT,"directory_not_empty",format!("Only empty directories can be deleted: {e}")))?;}
    else{std::fs::remove_file(&target).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_delete_failed",e.to_string()))?;}
    Ok(StatusCode::NO_CONTENT)
}

async fn list_databases(State(state): State<AppState>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let mut q = db.prepare("SELECT id,name,engine,file_path,created_at FROM managed_databases ORDER BY name").map_err(database_error)?;
    let rows = q.query_map([], |r| {
        let id:String=r.get(0)?; let name:String=r.get(1)?; let engine:String=r.get(2)?;
        let path:String=r.get(3)?; let created_at:String=r.get(4)?;
        let size=std::fs::metadata(&path).map(|m|m.len()).unwrap_or(0);
        Ok(json!({"id":id,"name":name,"engine":engine,"size_bytes":size,"created_at":created_at}))
    }).map_err(database_error)?;
    Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(database_error)?))
}

async fn create_database(State(state): State<AppState>, Json(input): Json<serde_json::Value>) -> Result<(StatusCode,Json<serde_json::Value>),ApiError> {
    let name=input.get("name").and_then(|v|v.as_str()).unwrap_or("").trim();
    if name.is_empty() || name.len()>48 || !name.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'||b==b'-') {
        return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_database_name","Use 1–48 letters, numbers, underscores or hyphens for the database name.".into()));
    }
    let id=Uuid::new_v4().to_string();
    let directory=state.config_dir.join("databases");
    std::fs::create_dir_all(&directory).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_storage_unavailable",e.to_string()))?;
    let path=directory.join(format!("{id}.sqlite3"));
    let path_text=path.to_string_lossy().to_string();
    let mut connection=rusqlite::Connection::open(&path).map_err(database_error)?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;").map_err(database_error)?;
    connection.close().map_err(|(_,e)|database_error(e))?;
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    if db.query_row("SELECT EXISTS(SELECT 1 FROM managed_databases WHERE name=?1)",[name],|r|r.get::<_,bool>(0)).map_err(database_error)? {
        let _=std::fs::remove_file(&path);
        return Err(ApiError(StatusCode::CONFLICT,"database_name_in_use","A managed database with that name already exists.".into()));
    }
    db.execute("INSERT INTO managed_databases(id,name,engine,file_path) VALUES (?1,?2,'sqlite',?3)",params![id,name,path_text]).map_err(database_error)?;
    let created_at:String=db.query_row("SELECT created_at FROM managed_databases WHERE id=?1",[&id],|r|r.get(0)).map_err(database_error)?;
    Ok((StatusCode::CREATED,Json(json!({"id":id,"name":name,"engine":"sqlite","size_bytes":0,"file_path":path_text,"created_at":created_at}))))
}

async fn test_database(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>,ApiError> {
    let path:String={
        let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
        db.query_row("SELECT file_path FROM managed_databases WHERE id=?1",[&id],|r|r.get(0))
            .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"database_not_found","Managed database was not found.".into())}else{database_error(e)})?
    };
    let connection=rusqlite::Connection::open_with_flags(&path,rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e|ApiError(StatusCode::BAD_GATEWAY,"database_connection_failed",e.to_string()))?;
    let result:String=connection.query_row("PRAGMA integrity_check",[],|r|r.get(0)).map_err(database_error)?;
    Ok(Json(json!({"database_id":id,"connected":result=="ok","integrity":result})))
}

async fn delete_database(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode,ApiError> {
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let path: String=db.query_row("SELECT file_path FROM managed_databases WHERE id=?1",[&id],|r|r.get(0))
        .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"database_not_found","Managed database was not found.".into())}else{database_error(e)})?;
    db.execute("DELETE FROM managed_databases WHERE id=?1",[&id]).map_err(database_error)?;
    drop(db);
    let _=std::fs::remove_file(&path);
    let _=std::fs::remove_file(format!("{path}-wal"));
    let _=std::fs::remove_file(format!("{path}-shm"));
    Ok(StatusCode::NO_CONTENT)
}

async fn system_metrics()->Result<Json<serde_json::Value>,ApiError>{
    let metrics=tokio::task::spawn_blocking(crate::monitor::snapshot).await
        .map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"metrics_collection_failed",e.to_string()))?;
    Ok(Json(metrics))
}

async fn networking_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>,ApiError> {
    let lan_ip=crate::network::lan_ipv4().map(|ip|ip.to_string());
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let mut q=db.prepare("SELECT id,name,runtime,status,lan_enabled,lan_port FROM websites ORDER BY name").map_err(database_error)?;
    let rows=q.query_map([],|r|Ok(json!({
        "id":r.get::<_,String>(0)?,
        "name":r.get::<_,String>(1)?,
        "runtime":r.get::<_,String>(2)?,
        "status":r.get::<_,String>(3)?,
        "lan_enabled":r.get::<_,i64>(4)? != 0,
        "lan_port":r.get::<_,Option<u16>>(5)?
    }))).map_err(database_error)?;
    let websites=rows.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
    Ok(Json(json!({"lan_ipv4":lan_ip,"websites":websites,"management_api":"loopback_only","public_exposure_enabled":false})))
}

async fn set_lan_access(
    State(state):State<AppState>,
    Path(id):Path<String>,
    Json(input):Json<SetLanAccess>
)->Result<Json<serde_json::Value>,ApiError>{
    let lan_ip=crate::network::lan_ipv4().map(|ip|ip.to_string());
    if input.enabled && lan_ip.is_none() {
        return Err(ApiError(StatusCode::CONFLICT,"lan_address_unavailable","No private IPv4 address was found on the default network route. Connect to a private network and retry.".into()));
    }
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let (runtime,existing_port):(String,Option<u16>)=db.query_row(
        "SELECT runtime,lan_port FROM websites WHERE id=?1",[&id],|row|Ok((row.get(0)?,row.get(1)?))
    ).map_err(|e|match e { rusqlite::Error::QueryReturnedNoRows => ApiError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into()), other => database_error(other) })?;
    if input.enabled && runtime!="static" && runtime!="node" && runtime!="php" && runtime!="python" {
        return Err(ApiError(StatusCode::BAD_REQUEST,"lan_runtime_unsupported","LAN publishing currently supports static, Node.js and PHP websites.".into()));
    }
    let port=if input.enabled {
        Some(match existing_port {Some(value)=>value,None=>crate::network::allocate_lan_port(&db).map_err(|e|ApiError(StatusCode::CONFLICT,"lan_port_unavailable",e))?})
    } else {existing_port};
    db.execute("UPDATE websites SET lan_enabled=?1,lan_port=?2,updated_at=CURRENT_TIMESTAMP WHERE id=?3",
        rusqlite::params![if input.enabled {1} else {0},port,id]).map_err(database_error)?;
    Ok(Json(json!({
        "website_id":id,"enabled":input.enabled,"lan_ipv4":lan_ip,"port":port,
        "url":if input.enabled {format!("http://{}:{}",lan_ip.unwrap_or_default(),port.unwrap_or_default())} else {String::new()},
        "note":"Reload Caddy to apply this change. Windows Firewall may need a private-profile inbound rule for this website port."
    })))
}

async fn diagnose_domain(Json(input): Json<DiagnoseDomain>) -> Result<Json<serde_json::Value>, ApiError> {
    let hostname=input.hostname.trim().trim_end_matches('.').to_ascii_lowercase();
    if hostname.len()>253 || !hostname.contains('.') || hostname.ends_with(".localhost") || hostname.ends_with(".test")
        || hostname.parse::<std::net::IpAddr>().is_ok()
        || !hostname.split('.').all(|label| !label.is_empty() && label.len()<=63
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label.as_bytes()[label.len()-1].is_ascii_alphanumeric()
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b==b'-')) {
        return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_public_hostname","Enter a valid public domain name, not an IP address or a .localhost/.test name.".into()));
    }
    let expected=match input.expected_ipv4.as_deref().map(str::trim).filter(|v|!v.is_empty()) {
        Some(value)=>Some(value.parse::<std::net::Ipv4Addr>().map_err(|_|ApiError(StatusCode::BAD_REQUEST,"invalid_expected_ipv4","Expected IPv4 must be a valid IPv4 address.".into()))?),
        None=>None
    };
    let resolved=tokio::net::lookup_host((hostname.as_str(),80)).await
        .map_err(|e|ApiError(StatusCode::BAD_GATEWAY,"dns_lookup_failed",format!("DNS lookup failed: {e}")))?;
    let mut addresses=resolved.map(|a|a.ip()).collect::<Vec<_>>();
    addresses.sort(); addresses.dedup();
    let ipv4=addresses.iter().filter_map(|a|if let std::net::IpAddr::V4(v)=a {Some(v.to_string())}else{None}).collect::<Vec<_>>();
    let ipv6=addresses.iter().filter_map(|a|if let std::net::IpAddr::V6(v)=a {Some(v.to_string())}else{None}).collect::<Vec<_>>();
    let matches=expected.map(|ip|ipv4.iter().any(|a|a==&ip.to_string()));
    Ok(Json(json!({"hostname":hostname,"resolved":!addresses.is_empty(),"ipv4":ipv4,"ipv6":ipv6,
        "expected_ipv4":expected.map(|v|v.to_string()),"matches_expected_ipv4":matches,
        "ready_for_https":!addresses.is_empty() && matches.unwrap_or(true),
        "https_note":"DNS resolution is only a readiness signal. HTTPS issuance still requires the domain to reach this machine publicly and Caddy's ACME challenge to succeed.",
        "public_exposure_enabled":false})))
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
    if runtime!="static" && runtime!="node" && runtime!="php" && runtime!="python" { return Err(ApiError(StatusCode::NOT_IMPLEMENTED,"runtime_domain_unavailable","Local domain routing currently supports static, Node.js, PHP and Python websites.".into())); }
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
async fn list_public_domains(State(state):State<AppState>)->Result<Json<Vec<PublicDomain>>,ApiError>{
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let mut q=db.prepare("SELECT p.id,p.website_id,w.name,p.hostname,w.runtime,w.port,p.created_at FROM public_domains p JOIN websites w ON w.id=p.website_id ORDER BY p.hostname").map_err(database_error)?;
    let rows=q.query_map([],|r|Ok(PublicDomain{id:r.get(0)?,website_id:r.get(1)?,website_name:r.get(2)?,hostname:r.get(3)?,runtime:r.get(4)?,port:r.get(5)?,created_at:r.get(6)?})).map_err(database_error)?;
    Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(database_error)?))
}
async fn create_public_domain(State(state):State<AppState>,Json(input):Json<CreatePublicDomain>)->Result<(StatusCode,Json<PublicDomain>),ApiError>{
    let hostname=input.hostname.trim().trim_end_matches('.').to_ascii_lowercase();
    if !valid_public_hostname(&hostname){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_public_hostname","Enter a valid public DNS hostname, not an IP address or a .localhost/.test name.".into()));}
    let id=Uuid::new_v4().to_string();
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let (runtime,port):(String,Option<u16>)=db.query_row("SELECT runtime,port FROM websites WHERE id=?1",params![input.website_id],|r|Ok((r.get(0)?,r.get(1)?)))
      .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"website_not_found","Select an existing website.".into())}else{database_error(e)})?;
    let used:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM public_domains WHERE hostname=?1 UNION SELECT 1 FROM domains WHERE hostname=?1)",params![hostname],|r|r.get(0)).map_err(database_error)?;
    if used{return Err(ApiError(StatusCode::CONFLICT,"hostname_in_use","That hostname is already assigned to a local or public website.".into()));}
    db.execute("INSERT INTO public_domains(id,website_id,hostname) VALUES (?1,?2,?3)",params![id,input.website_id,hostname]).map_err(database_error)?;
    let website_name:String=db.query_row("SELECT name FROM websites WHERE id=?1",params![input.website_id],|r|r.get(0)).map_err(database_error)?;
    let created_at:String=db.query_row("SELECT created_at FROM public_domains WHERE id=?1",params![id],|r|r.get(0)).map_err(database_error)?;
    Ok((StatusCode::CREATED,Json(PublicDomain{id,website_id:input.website_id,website_name,hostname,runtime,port,created_at})))
}
async fn delete_public_domain(State(state):State<AppState>,Path(id):Path<String>)->Result<StatusCode,ApiError>{
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    let n=db.execute("DELETE FROM public_domains WHERE id=?1",params![id]).map_err(database_error)?;
    if n==0{return Err(ApiError(StatusCode::NOT_FOUND,"public_domain_not_found","No public domain exists with that ID.".into()));}
    Ok(StatusCode::NO_CONTENT)
}
async fn export_caddyfile(State(state): State<AppState>) -> Result<Json<serde_json::Value>,ApiError> {
    let (config,routes,pending)=crate::proxy::generate_config(&state)
        .map_err(|e|ApiError(StatusCode::BAD_REQUEST,"caddy_config_failed",e))?;
    Ok(Json(json!({"config":config,"routes":routes,"pending":pending,"status":"generated_not_applied","note":"Validate and reload Caddy to apply domain and LAN routes. LAN routes bind only to the detected private IPv4 address; firewall access remains under your control."})))
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
fn allocate_port(state:&AppState,id:&str)->Result<u16,ApiError>{
    for _ in 0..32 {
        let listener=TcpListener::bind(("127.0.0.1",0)).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"port_allocation_failed",e.to_string()))?;
        let port=listener.local_addr().map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"port_allocation_failed",e.to_string()))?.port();
        let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
        let used:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM websites WHERE port=?1 AND id<>?2)",params![port,id],|r|r.get(0)).map_err(database_error)?;
        drop(db); drop(listener);
        if !used { return Ok(port); }
    }
    Err(ApiError(StatusCode::SERVICE_UNAVAILABLE,"port_allocation_failed","Could not allocate an unused local port.".into()))
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
    if runtime!="node" && runtime!="php" && runtime!="python"{return Err(ApiError(StatusCode::NOT_IMPLEMENTED,"runtime_unavailable","This runtime adapter is not implemented yet. Static, Node.js, PHP and Python projects are supported in this build.".into()));}
    let port=allocate_port(&state,&id)?;
    {
        let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
        db.execute("UPDATE websites SET port=?1 WHERE id=?2",params![port,id]).map_err(database_error)?;
    }
    let log_path=state.logs_dir.join(format!("{id}.log"));
    let stdout=File::create(&log_path).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_file_error",e.to_string()))?;
    let stderr=stdout.try_clone().map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_file_error",e.to_string()))?;
    let mut command=if runtime=="php" {
        let mut c=Command::new(crate::settings::setting_string(&state,"php_executable","php"));
        c.args(["-S",&format!("127.0.0.1:{port}"),"-t",&project]);
        c
    } else if runtime=="python" {
        let mut c=Command::new(crate::settings::setting_string(&state,"python_executable","python"));
        c.args(["-m","http.server",&port.to_string(),"--bind","127.0.0.1","--directory",&project]);
        c
    } else {
        #[cfg(windows)]
        { let mut c=Command::new("cmd");c.args(["/C","npm run start"]);c }
        #[cfg(not(windows))]
        { let mut c=Command::new("npm");c.args(["run","start"]);c }
    };
    let child=command.current_dir(&project).env("PORT",port.to_string()).env("HOST","127.0.0.1").stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr)).spawn()
      .map_err(|e|ApiError(StatusCode::BAD_REQUEST,"runtime_start_failed",if runtime=="php"{format!("Could not start PHP. Verify PHP is installed and available on PATH: {e}")}else if runtime=="python"{format!("Could not start Python. Verify Python is installed and available as `python` on PATH: {e}")}else{format!("Could not start npm. Verify Node.js and npm are installed: {e}")}))?;
    let pid=child.id();
    state.processes.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"process_registry_locked","Process registry is unavailable.".into()))?.insert(id.clone(),child);
    set_status(&state,&id,"running")?;
    Ok(Json(json!({"id":id,"status":"running","pid":pid,"port":port,"url":format!("http://127.0.0.1:{port}"),"log_path":log_path})))
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
    let active_release=state.db.lock().ok().and_then(|db|db.query_row("SELECT release_path FROM releases WHERE website_id=?1 AND active=1",params![id],|row|row.get::<_,String>(0)).ok());
    let candidate=active_release.clone().map(PathBuf::from).unwrap_or_else(||PathBuf::from(project));
    let root=match std::fs::canonicalize(&candidate){Ok(p)=>p,Err(_)=>return StatusCode::NOT_FOUND.into_response()};
    if active_release.is_some(){
        let releases_root=match state.releases_dir.canonicalize(){Ok(p)=>p,Err(_)=>return StatusCode::NOT_FOUND.into_response()};
        if !root.starts_with(releases_root){return StatusCode::NOT_FOUND.into_response();}
    }
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
