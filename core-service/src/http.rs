use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header::AUTHORIZATION, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs::File, io::{Cursor, Read, Seek, SeekFrom, Write}, path::{Path as FsPath, PathBuf}, process::{Command, Stdio}, net::{SocketAddr, TcpListener, TcpStream}, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}}, time::{Duration, Instant}};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};
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
        .route("/api/v1/runtime-profiles", get(list_runtime_profiles).put(save_runtime_profile))
        .route("/api/v1/runtime-profiles/:website_id", axum::routing::delete(delete_runtime_profile))
        .route("/api/v1/websites/:id/start", axum::routing::post(start_website))
        .route("/api/v1/websites/:id/stop", axum::routing::post(stop_website))
        .route("/api/v1/websites/:id/restart", axum::routing::post(restart_website))
        .route("/api/v1/websites/:id/logs", get(website_logs))
        .route("/api/v1/websites/:id/files", get(list_website_files))
        .route("/api/v1/websites/:id/storage", get(website_storage_usage))
        .route("/api/v1/websites/:id/file", get(read_website_file).put(write_website_file).delete(delete_website_file))
        .route("/api/v1/websites/:id/files/upload", axum::routing::post(upload_website_file))
        .route("/api/v1/websites/:id/files/download", get(download_website_file))
        .route("/api/v1/websites/:id/files/archive", get(download_website_archive))
        .route("/api/v1/websites/:id/files/extract", axum::routing::post(extract_website_archive))
        .route("/api/v1/websites/:id/files/rename", axum::routing::post(rename_website_file))
        .route("/api/v1/websites/:id/files/directory", axum::routing::post(create_website_directory))
        .route("/api/v1/domains", get(list_domains).post(create_domain))
        .route("/api/v1/domains/diagnose", axum::routing::post(diagnose_domain))
        .route("/api/v1/public-domains", get(list_public_domains).post(create_public_domain))
        .route("/api/v1/public-domains/:id", axum::routing::delete(delete_public_domain))
        .route("/api/v1/domains/:id", axum::routing::delete(delete_domain))
        .route("/api/v1/domains/caddyfile", get(export_caddyfile))
        .route("/api/v1/networking/status", get(networking_status))
        .route("/api/v1/tunnels", get(crate::tunnel::list))
        .route("/api/v1/tunnels/:website_id/start", axum::routing::post(crate::tunnel::start))
        .route("/api/v1/tunnels/:website_id/stop", axum::routing::post(crate::tunnel::stop))
        .route("/api/v1/monitoring/metrics", get(system_metrics))
        .route("/api/v1/databases", get(list_databases).post(create_database))
        .route("/api/v1/databases/:id", axum::routing::delete(delete_database))
        .route("/api/v1/databases/:id/test", axum::routing::post(test_database))
        .route("/api/v1/databases/:id/query", axum::routing::post(query_database))
        .route("/api/v1/databases/:id/export", get(export_database))
        .route("/api/v1/databases/import", axum::routing::post(import_database))
        .route("/api/v1/postgres-connections", get(list_postgres_connections).post(create_postgres_connection))
        .route("/api/v1/postgres-connections/:id", axum::routing::delete(delete_postgres_connection))
        .route("/api/v1/postgres-connections/:id/test", axum::routing::post(test_postgres_connection))
        .route("/api/v1/postgres-connections/:id/query", axum::routing::post(query_postgres_connection))
        .route("/api/v1/mysql-connections", get(list_mysql_connections).post(create_mysql_connection))
        .route("/api/v1/mysql-connections/:id", axum::routing::delete(delete_mysql_connection))
        .route("/api/v1/mysql-connections/:id/test", axum::routing::post(test_mysql_connection))
        .route("/api/v1/mysql-connections/:id/query", axum::routing::post(query_mysql_connection))
        .route("/api/v1/websites/:id/lan", axum::routing::put(set_lan_access))
        .merge(crate::proxy::router())
        .merge(crate::deploy::router())
        .merge(crate::git::router())
        .merge(crate::backup::router())
        .merge(crate::settings::router())
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
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
#[derive(Debug, Deserialize)]
struct RenameWebsiteFile { source_path: String, new_path: String }
#[derive(Debug, Deserialize)]
struct CreateWebsiteDirectory { path: String }

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
async fn website_storage_usage(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let root=website_root(&state,&id)?;
    let mut pending=vec![root.clone()];
    let mut total_bytes:u64=0;
    let mut file_count:u64=0;
    let mut directory_count:u64=0;
    let mut skipped_count:u64=0;
    let mut truncated=false;
    const MAX_ENTRIES:u64=100_000;
    while let Some(directory)=pending.pop(){
        let entries=std::fs::read_dir(&directory)
            .map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"storage_scan_failed",format!("Could not scan website storage: {e}")))?;
        for entry in entries{
            if file_count+directory_count+skipped_count>=MAX_ENTRIES{truncated=true;break;}
            let entry=entry.map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"storage_scan_failed",e.to_string()))?;
            let metadata=std::fs::symlink_metadata(entry.path())
                .map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"storage_scan_failed",e.to_string()))?;
            let kind=metadata.file_type();
            if kind.is_symlink(){skipped_count+=1;continue;}
            if kind.is_dir(){directory_count+=1;pending.push(entry.path());}
            else if kind.is_file(){file_count+=1;total_bytes=total_bytes.saturating_add(metadata.len());}
            else{skipped_count+=1;}
        }
        if truncated{break;}
    }
    Ok(Json(json!({"website_id":id,"total_bytes":total_bytes,"file_count":file_count,"directory_count":directory_count,"skipped_count":skipped_count,"truncated":truncated,"max_entries":MAX_ENTRIES})))
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

#[derive(Debug, Deserialize)]
struct UploadWebsiteFile { path: String, content_base64: String }
fn base64_value(byte:u8)->Option<u8>{match byte {b'A'..=b'Z'=>Some(byte-b'A'),b'a'..=b'z'=>Some(byte-b'a'+26),b'0'..=b'9'=>Some(byte-b'0'+52),b'+'=>Some(62),b'/'=>Some(63),_=>None}}
fn decode_base64(value:&str)->Result<Vec<u8>,ApiError>{
 let bytes=value.as_bytes();
 if bytes.len()%4!=0||bytes.len()>23*1024*1024{return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_file_data","The uploaded file data is invalid or too large.".into()));}
 let mut output=Vec::with_capacity(bytes.len()/4*3);
 for (index,chunk) in bytes.chunks_exact(4).enumerate(){
  let last=index==bytes.len()/4-1;let a=base64_value(chunk[0]);let b=base64_value(chunk[1]);let c=if chunk[2]==b'='{None}else{base64_value(chunk[2])};let d=if chunk[3]==b'='{None}else{base64_value(chunk[3])};
  if a.is_none()||b.is_none()||(!last&&(chunk[2]==b'='||chunk[3]==b'='))||(chunk[2]==b'='&&chunk[3]!=b'=')||(chunk[2]!=b'='&&c.is_none())||(chunk[3]!=b'='&&d.is_none()){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_file_data","The uploaded file data is not valid Base64.".into()));}
  let a=a.unwrap();let b=b.unwrap();let c=c.unwrap_or(0);let d=d.unwrap_or(0);output.push((a<<2)|(b>>4));if chunk[2]!=b'='{output.push((b<<4)|(c>>2));}if chunk[3]!=b'='{output.push((c<<6)|d);}
 }
 if output.len()>16*1024*1024{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"file_too_large","File uploads are limited to 16 MiB.".into()));}Ok(output)
}
fn encode_base64(bytes:&[u8])->String{
 const TABLE:&[u8;64]=b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";let mut output=String::with_capacity((bytes.len()+2)/3*4);
 for chunk in bytes.chunks(3){let a=chunk[0];let b=*chunk.get(1).unwrap_or(&0);let c=*chunk.get(2).unwrap_or(&0);output.push(TABLE[(a>>2) as usize] as char);output.push(TABLE[(((a&3)<<4)|(b>>4)) as usize] as char);output.push(if chunk.len()>1{TABLE[(((b&15)<<2)|(c>>6)) as usize] as char}else{'='});output.push(if chunk.len()>2{TABLE[(c&63) as usize] as char}else{'='});}output
}
async fn upload_website_file(State(state):State<AppState>,Path(id):Path<String>,Json(input):Json<UploadWebsiteFile>)->Result<(StatusCode,Json<serde_json::Value>),ApiError>{
 let bytes=decode_base64(&input.content_base64)?;let root=website_root(&state,&id)?;let relative=safe_relative_path(&input.path)?;
 if relative.as_os_str().is_empty(){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_file_path","A file name is required.".into()));}
 let parent_relative=relative.parent().unwrap_or_else(||FsPath::new(""));let parent_text=parent_relative.to_string_lossy();let parent=if parent_text.is_empty(){root.clone()}else{contained_existing(&root,&parent_text)?};
 if !parent.is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"parent_not_directory","The parent path is not a directory.".into()));}
 let name=relative.file_name().ok_or_else(||ApiError(StatusCode::BAD_REQUEST,"invalid_file_path","A file name is required.".into()))?;let target=parent.join(name);
 let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e|if e.kind()==std::io::ErrorKind::AlreadyExists{ApiError(StatusCode::CONFLICT,"destination_exists","A file or folder already exists at this path; uploads never overwrite.".into())}else{ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_upload_failed",e.to_string())})?;
 file.write_all(&bytes).map_err(|e|{let _=std::fs::remove_file(&target);ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_upload_failed",e.to_string())})?;
 Ok((StatusCode::CREATED,Json(json!({"website_id":id,"path":input.path,"size_bytes":bytes.len(),"uploaded":true}))))
}
async fn download_website_file(State(state):State<AppState>,Path(id):Path<String>,Query(query):Query<FilePathQuery>)->Result<Json<serde_json::Value>,ApiError>{
 let root=website_root(&state,&id)?;let relative=query.path.as_deref().unwrap_or("");let target=contained_existing(&root,relative)?;
 if !target.is_file(){return Err(ApiError(StatusCode::BAD_REQUEST,"not_a_file","The requested path is not a regular file.".into()));}
 let metadata=std::fs::metadata(&target).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_metadata_failed",e.to_string()))?;
 if metadata.len()>32*1024*1024{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"file_too_large","Downloads through the File Manager are limited to 32 MiB.".into()));}
 let bytes=std::fs::read(&target).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_read_failed",e.to_string()))?;
 Ok(Json(json!({"website_id":id,"path":relative,"size_bytes":bytes.len(),"content_base64":encode_base64(&bytes)})))
}


#[derive(Debug, Deserialize)]
struct ExtractWebsiteArchive { path: String, archive_base64: String }

fn add_archive_directory(
    writer: &mut ZipWriter<Cursor<Vec<u8>>>,
    root: &FsPath,
    directory: &FsPath,
    count: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), ApiError> {
    for entry in std::fs::read_dir(directory).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_read_failed",e.to_string()))? {
        let entry=entry.map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_read_failed",e.to_string()))?;
        let metadata=std::fs::symlink_metadata(entry.path()).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_metadata_failed",e.to_string()))?;
        if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) { continue; }
        *count+=1;
        if *count>1000 { return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"archive_entry_limit","ZIP creation is limited to 1,000 files and folders.".into())); }
        let entry_path=entry.path();
        let relative=entry_path.strip_prefix(root).map_err(|_|ApiError(StatusCode::FORBIDDEN,"archive_path_forbidden","An archive entry escaped the selected folder.".into()))?;
        let name=relative.to_string_lossy().replace('\\',"/");
        if metadata.is_dir() {
            writer.add_directory(format!("{name}/"),SimpleFileOptions::default())
                .map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_write_failed",e.to_string()))?;
            add_archive_directory(writer,root,&entry_path,count,total_bytes)?;
        } else {
            *total_bytes=total_bytes.saturating_add(metadata.len());
            if *total_bytes>256*1024*1024 { return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"archive_size_limit","ZIP creation is limited to 256 MiB of uncompressed files.".into())); }
            writer.start_file(name,SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated))
                .map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_write_failed",e.to_string()))?;
            let mut file=File::open(&entry_path).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_read_failed",e.to_string()))?;
            let mut buffer=[0u8;64*1024];
            loop {
                let read=file.read(&mut buffer).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_read_failed",e.to_string()))?;
                if read==0 { break; }
                writer.write_all(&buffer[..read]).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_write_failed",e.to_string()))?;
            }
        }
    }
    Ok(())
}

async fn download_website_archive(State(state):State<AppState>,Path(id):Path<String>,Query(query):Query<FilePathQuery>)->Result<Json<serde_json::Value>,ApiError>{
    let root=website_root(&state,&id)?;
    let relative=query.path.as_deref().unwrap_or("");
    let directory=if relative.is_empty(){root.clone()}else{contained_existing(&root,relative)?};
    if !directory.is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"not_a_directory","ZIP creation requires a folder.".into()));}
    let mut writer=ZipWriter::new(Cursor::new(Vec::new()));
    let mut count=0usize;
    let mut total_bytes=0u64;
    add_archive_directory(&mut writer,&directory,&directory,&mut count,&mut total_bytes)?;
    let bytes=writer.finish().map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_write_failed",e.to_string()))?.into_inner();
    if bytes.len()>16*1024*1024{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"archive_compressed_limit","Generated ZIP files are limited to 16 MiB.".into()));}
    Ok(Json(json!({"website_id":id,"path":relative,"size_bytes":bytes.len(),"content_base64":encode_base64(&bytes)})))
}

async fn extract_website_archive(State(state):State<AppState>,Path(id):Path<String>,Json(input):Json<ExtractWebsiteArchive>)->Result<(StatusCode,Json<serde_json::Value>),ApiError>{
    let root=website_root(&state,&id)?;
    let relative=safe_relative_path(&input.path)?;
    if relative.as_os_str().is_empty(){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_extract_path","Choose a new folder name for extraction.".into()));}
    let parent_relative=relative.parent().unwrap_or_else(||FsPath::new(""));
    let parent_text=parent_relative.to_string_lossy();
    let parent=if parent_text.is_empty(){root.clone()}else{contained_existing(&root,&parent_text)?};
    if !parent.is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"parent_not_directory","The extraction parent is not a directory.".into()));}
    let name=relative.file_name().ok_or_else(||ApiError(StatusCode::BAD_REQUEST,"invalid_extract_path","Choose a new folder name for extraction.".into()))?;
    let destination=parent.join(name);
    let bytes=decode_base64(&input.archive_base64)?;
    if bytes.len()>16*1024*1024{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"archive_compressed_limit","ZIP uploads are limited to 16 MiB.".into()));}
    match std::fs::symlink_metadata(&destination){
        Ok(_)=>return Err(ApiError(StatusCode::CONFLICT,"destination_exists","The extraction folder already exists.".into())),
        Err(e) if e.kind()!=std::io::ErrorKind::NotFound=>return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR,"directory_check_failed",e.to_string())),
        Err(_)=>{}
    }
    let result=(||->Result<(usize,u64),ApiError>{
        let mut archive=ZipArchive::new(Cursor::new(bytes)).map_err(|e|ApiError(StatusCode::BAD_REQUEST,"invalid_zip_archive",format!("The selected file is not a valid ZIP archive: {e}")))?;
        if archive.len()>1000{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"archive_entry_limit","ZIP extraction is limited to 1,000 entries.".into()));}
        let mut total=0u64;
        for index in 0..archive.len(){
            let mut file=archive.by_index(index).map_err(|e|ApiError(StatusCode::BAD_REQUEST,"invalid_zip_archive",e.to_string()))?;
            let enclosed=file.enclosed_name().ok_or_else(||ApiError(StatusCode::BAD_REQUEST,"unsafe_archive_path","The ZIP contains an absolute or traversal path.".into()))?.to_path_buf();
            if enclosed.components().any(|c|!matches!(c,std::path::Component::Normal(_))) || enclosed.to_string_lossy().contains('\\') || enclosed.to_string_lossy().contains(':') {
                return Err(ApiError(StatusCode::BAD_REQUEST,"unsafe_archive_path","The ZIP contains an unsafe path.".into()));
            }
            if file.unix_mode().map(|mode|mode&0o170000==0o120000).unwrap_or(false){
                return Err(ApiError(StatusCode::BAD_REQUEST,"archive_symlink_forbidden","ZIP symbolic links are not extracted.".into()));
            }
            total=total.saturating_add(file.size());
            if total>256*1024*1024{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"archive_uncompressed_limit","ZIP extraction is limited to 256 MiB uncompressed.".into()));}
            let target=destination.join(&enclosed);
            if file.is_dir(){
                std::fs::create_dir_all(&target).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_extract_failed",e.to_string()))?;
            }else{
                let target_parent=target.parent().ok_or_else(||ApiError(StatusCode::BAD_REQUEST,"unsafe_archive_path","The ZIP entry has no parent folder.".into()))?;
                std::fs::create_dir_all(target_parent).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_extract_failed",e.to_string()))?;
                let mut output=std::fs::OpenOptions::new().write(true).create_new(true).open(&target)
                    .map_err(|e|if e.kind()==std::io::ErrorKind::AlreadyExists{ApiError(StatusCode::CONFLICT,"archive_duplicate_path","The ZIP contains duplicate paths.".into())}else{ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_extract_failed",e.to_string())})?;
                std::io::copy(&mut file,&mut output).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"archive_extract_failed",e.to_string()))?;
            }
        }
        Ok((archive.len(),total))
    })();
    match result {
        Ok((entries,total_bytes))=>Ok((StatusCode::CREATED,Json(json!({"website_id":id,"path":input.path,"entries":entries,"uncompressed_bytes":total_bytes,"extracted":true})))),
        Err(error)=>{let _=std::fs::remove_dir_all(&destination);Err(error)}
    }
}

async fn create_website_directory(State(state):State<AppState>,Path(id):Path<String>,Json(input):Json<CreateWebsiteDirectory>)->Result<(StatusCode,Json<serde_json::Value>),ApiError>{
    let root=website_root(&state,&id)?;
    let relative=safe_relative_path(&input.path)?;
    if relative.as_os_str().is_empty(){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_directory_path","A folder name is required.".into()));}
    let parent_relative=relative.parent().unwrap_or_else(||FsPath::new(""));
    let parent_text=parent_relative.to_string_lossy();
    let parent=if parent_text.is_empty(){root.clone()}else{contained_existing(&root,&parent_text)?};
    if !parent.is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"parent_not_directory","The parent path is not a directory.".into()));}
    let name=relative.file_name().ok_or_else(||ApiError(StatusCode::BAD_REQUEST,"invalid_directory_path","A folder name is required.".into()))?;
    let target=parent.join(name);
    match std::fs::symlink_metadata(&target){
        Ok(_)=>return Err(ApiError(StatusCode::CONFLICT,"destination_exists","A file or folder already exists at this path.".into())),
        Err(e) if e.kind()!=std::io::ErrorKind::NotFound=>return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR,"directory_check_failed",e.to_string())),
        Err(_)=>{}
    }
    std::fs::create_dir(&target).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"directory_create_failed",e.to_string()))?;
    Ok((StatusCode::CREATED,Json(json!({"website_id":id,"path":input.path,"created":true}))))
}
async fn rename_website_file(State(state):State<AppState>,Path(id):Path<String>,Json(input):Json<RenameWebsiteFile>)->Result<Json<serde_json::Value>,ApiError>{
    let root=website_root(&state,&id)?;
    let source=contained_existing(&root,&input.source_path)?;
    if source==root{return Err(ApiError(StatusCode::FORBIDDEN,"cannot_rename_website_root","The website root cannot be renamed.".into()));}
    let new_relative=safe_relative_path(&input.new_path)?;
    if new_relative.as_os_str().is_empty(){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_file_path","A destination name is required.".into()));}
    let parent_relative=new_relative.parent().unwrap_or_else(||FsPath::new(""));
    let parent_text=parent_relative.to_string_lossy();
    let destination_parent=if parent_text.is_empty(){root.clone()}else{contained_existing(&root,&parent_text)?};
    if !destination_parent.is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"parent_not_directory","The destination parent is not a directory.".into()));}
    let file_name=new_relative.file_name().ok_or_else(||ApiError(StatusCode::BAD_REQUEST,"invalid_file_path","A destination name is required.".into()))?;
    let destination=destination_parent.join(file_name);
    if destination.exists()||std::fs::symlink_metadata(&destination).is_ok(){
        return Err(ApiError(StatusCode::CONFLICT,"destination_exists","A file or folder already exists at the destination.".into()));
    }
    if destination_parent==source || destination_parent.starts_with(&source){
        return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_move","A folder cannot be moved inside itself.".into()));
    }
    std::fs::rename(&source,&destination).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"file_rename_failed",e.to_string()))?;
    Ok(Json(json!({"website_id":id,"source_path":input.source_path,"path":input.new_path,"renamed":true})))
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

#[derive(Debug, Deserialize)]
struct CreatePostgresConnection { name:String, host:String, port:u16, database_name:String, username:String }
#[derive(Debug, Deserialize)]
struct TestPostgresConnection { password:String }
fn validate_pg_field(value:&str,max:usize)->bool { !value.trim().is_empty() && value.len()<=max && !value.chars().any(|c|c.is_control()) }
async fn list_postgres_connections(State(state):State<AppState>)->Result<Json<Vec<serde_json::Value>>,ApiError>{
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let mut q=db.prepare("SELECT id,name,host,port,database_name,username,created_at FROM postgres_connections ORDER BY name").map_err(database_error)?;
 let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"host":r.get::<_,String>(2)?,"port":r.get::<_,u16>(3)?,"database_name":r.get::<_,String>(4)?,"username":r.get::<_,String>(5)?,"engine":"postgresql","created_at":r.get::<_,String>(6)?}))).map_err(database_error)?;
 Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(database_error)?))
}
async fn create_postgres_connection(State(state):State<AppState>,Json(input):Json<CreatePostgresConnection>)->Result<(StatusCode,Json<serde_json::Value>),ApiError>{
 let host=input.host.trim().to_ascii_lowercase();
 if !["localhost","127.0.0.1","::1"].contains(&host.as_str()){return Err(ApiError(StatusCode::BAD_REQUEST,"postgres_host_not_allowed","For this initial release, PostgreSQL connections are restricted to this PC (localhost, 127.0.0.1, or ::1).".into()));}
 if !validate_pg_field(&input.name,48)||!input.name.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'||b==b'-')||!validate_pg_field(&input.database_name,63)||!validate_pg_field(&input.username,63){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_postgres_profile","Profile name, database name, or username is invalid.".into()));}
 if input.port==0{return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_postgres_port","PostgreSQL port must be between 1 and 65535.".into()));}
 let id=Uuid::new_v4().to_string();
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 db.execute("INSERT INTO postgres_connections(id,name,host,port,database_name,username) VALUES(?1,?2,?3,?4,?5,?6)",params![id,input.name.trim(),host,input.port,input.database_name.trim(),input.username.trim()]).map_err(|e|if matches!(e,rusqlite::Error::SqliteFailure(ref x,_) if x.code==rusqlite::ErrorCode::ConstraintViolation){ApiError(StatusCode::CONFLICT,"postgres_profile_exists","A PostgreSQL profile with that name already exists.".into())}else{database_error(e)})?;
 let created_at:String=db.query_row("SELECT created_at FROM postgres_connections WHERE id=?1",[&id],|r|r.get(0)).map_err(database_error)?;
 Ok((StatusCode::CREATED,Json(json!({"id":id,"name":input.name.trim(),"host":host,"port":input.port,"database_name":input.database_name.trim(),"username":input.username.trim(),"engine":"postgresql","created_at":created_at}))))
}
async fn delete_postgres_connection(State(state):State<AppState>,Path(id):Path<String>)->Result<StatusCode,ApiError>{
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let changed=db.execute("DELETE FROM postgres_connections WHERE id=?1",[id]).map_err(database_error)?;
 if changed==0{return Err(ApiError(StatusCode::NOT_FOUND,"postgres_profile_not_found","PostgreSQL connection profile was not found.".into()));}
 Ok(StatusCode::NO_CONTENT)
}
async fn test_postgres_connection(State(state):State<AppState>,Path(id):Path<String>,Json(input):Json<TestPostgresConnection>)->Result<Json<serde_json::Value>,ApiError>{
 if input.password.len()>1024{return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_postgres_password","Password is too long.".into()));}
 let profile={
  let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  db.query_row("SELECT host,port,database_name,username FROM postgres_connections WHERE id=?1",[&id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u16>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))
   .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"postgres_profile_not_found","PostgreSQL connection profile was not found.".into())}else{database_error(e)})?
 };
 let (host,port,database_name,username)=profile;
 let mut config=tokio_postgres::Config::new();config.host(&host).port(port).dbname(&database_name).user(&username).password(&input.password).connect_timeout(std::time::Duration::from_secs(5));
 let (client,connection)=config.connect(tokio_postgres::NoTls).await.map_err(|e|ApiError(StatusCode::BAD_GATEWAY,"postgres_connection_failed",format!("PostgreSQL connection failed: {e}")))?;
 tokio::spawn(async move{let _=connection.await;});
 let row=client.query_one("SELECT current_database(), current_user, version()", &[]).await.map_err(|e|ApiError(StatusCode::BAD_GATEWAY,"postgres_query_failed",e.to_string()))?;
 Ok(Json(json!({"id":id,"connected":true,"database":row.get::<_,String>(0),"username":row.get::<_,String>(1),"server_version":row.get::<_,String>(2),"password_persisted":false})))
}



#[derive(Debug, Deserialize)]
struct CreateMysqlConnection { name:String, host:String, port:u16, database_name:String, username:String }
#[derive(Debug, Deserialize)]
struct TestMysqlConnection { password:String }
async fn list_mysql_connections(State(state):State<AppState>)->Result<Json<Vec<serde_json::Value>>,ApiError>{
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let mut q=db.prepare("SELECT id,name,host,port,database_name,username,created_at FROM mysql_connections ORDER BY name").map_err(database_error)?;
 let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"host":r.get::<_,String>(2)?,"port":r.get::<_,u16>(3)?,"database_name":r.get::<_,String>(4)?,"username":r.get::<_,String>(5)?,"engine":"mysql","created_at":r.get::<_,String>(6)?}))).map_err(database_error)?;
 Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(database_error)?))
}
async fn create_mysql_connection(State(state):State<AppState>,Json(input):Json<CreateMysqlConnection>)->Result<(StatusCode,Json<serde_json::Value>),ApiError>{
 let host=input.host.trim().to_ascii_lowercase();
 if !["localhost","127.0.0.1","::1"].contains(&host.as_str()){return Err(ApiError(StatusCode::BAD_REQUEST,"mysql_host_not_allowed","MySQL/MariaDB profiles are restricted to this PC (localhost, 127.0.0.1, or ::1).".into()));}
 if !validate_pg_field(&input.name,48)||!input.name.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'||b==b'-')||!validate_pg_field(&input.database_name,64)||!validate_pg_field(&input.username,96){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_mysql_profile","Profile name, database name, or username is invalid.".into()));}
 if input.port==0{return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_mysql_port","MySQL/MariaDB port must be between 1 and 65535.".into()));}
 let id=Uuid::new_v4().to_string();
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 db.execute("INSERT INTO mysql_connections(id,name,host,port,database_name,username) VALUES(?1,?2,?3,?4,?5,?6)",params![id,input.name.trim(),host,input.port,input.database_name.trim(),input.username.trim()]).map_err(|e|if matches!(e,rusqlite::Error::SqliteFailure(ref x,_) if x.code==rusqlite::ErrorCode::ConstraintViolation){ApiError(StatusCode::CONFLICT,"mysql_profile_exists","A MySQL/MariaDB profile with that name already exists.".into())}else{database_error(e)})?;
 let created_at:String=db.query_row("SELECT created_at FROM mysql_connections WHERE id=?1",[&id],|r|r.get(0)).map_err(database_error)?;
 Ok((StatusCode::CREATED,Json(json!({"id":id,"name":input.name.trim(),"host":host,"port":input.port,"database_name":input.database_name.trim(),"username":input.username.trim(),"engine":"mysql","created_at":created_at}))))
}
async fn delete_mysql_connection(State(state):State<AppState>,Path(id):Path<String>)->Result<StatusCode,ApiError>{
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let changed=db.execute("DELETE FROM mysql_connections WHERE id=?1",[id]).map_err(database_error)?;
 if changed==0{return Err(ApiError(StatusCode::NOT_FOUND,"mysql_profile_not_found","MySQL/MariaDB connection profile was not found.".into()));}
 Ok(StatusCode::NO_CONTENT)
}
async fn test_mysql_connection(State(state):State<AppState>,Path(id):Path<String>,Json(input):Json<TestMysqlConnection>)->Result<Json<serde_json::Value>,ApiError>{
 if input.password.len()>1024{return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_mysql_password","Password is too long.".into()));}
 let (host,port,database_name,username)={
  let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  db.query_row("SELECT host,port,database_name,username FROM mysql_connections WHERE id=?1",[&id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u16>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))
   .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"mysql_profile_not_found","MySQL/MariaDB connection profile was not found.".into())}else{database_error(e)})?
 };
 let mut options=sqlx::mysql::MySqlConnectOptions::new().host(&host).port(port).username(&username).password(&input.password).ssl_mode(sqlx::mysql::MySqlSslMode::Disabled);
 if !database_name.is_empty(){options=options.database(&database_name);}
 let connect=sqlx::mysql::MySqlConnection::connect_with(&options);
 let mut connection=tokio::time::timeout(std::time::Duration::from_secs(5),connect).await
  .map_err(|_|ApiError(StatusCode::GATEWAY_TIMEOUT,"mysql_connection_timeout","MySQL/MariaDB connection timed out after five seconds.".into()))?
  .map_err(|e|ApiError(StatusCode::BAD_GATEWAY,"mysql_connection_failed",format!("MySQL/MariaDB connection failed: {e}")))?;
 use sqlx::{Connection,Row};
 let row=sqlx::query("SELECT DATABASE(), CURRENT_USER(), VERSION()").fetch_one(&mut connection).await
  .map_err(|e|ApiError(StatusCode::BAD_GATEWAY,"mysql_query_failed",e.to_string()))?;
 let database:Option<String>=row.try_get(0).unwrap_or(None);
 let user:String=row.try_get(1).unwrap_or_default();
 let version:String=row.try_get(2).unwrap_or_default();
 connection.close().await.ok();
 Ok(Json(json!({"id":id,"connected":true,"database":database,"username":user,"server_version":version,"password_persisted":false})))
}


#[derive(Debug, Deserialize)]
struct MysqlQuery { password: String, sql: String }

async fn query_mysql_connection(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<MysqlQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let sql = input.sql.trim();
    if sql.is_empty() || sql.len() > 64 * 1024 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_query", "Enter a SQL statement up to 64 KiB.".into()));
    }
    if input.password.len() > 1024 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_mysql_password", "Password is too long.".into()));
    }
    let (host, port, database_name, username) = {
        let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database_locked", "Database lock is unavailable.".into()))?;
        db.query_row(
            "SELECT host,port,database_name,username FROM mysql_connections WHERE id=?1",
            [&id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, u16>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
        ).map_err(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
            ApiError(StatusCode::NOT_FOUND, "mysql_profile_not_found", "MySQL/MariaDB connection profile was not found.".into())
        } else { database_error(e) })?
    };
    let mut options = sqlx::mysql::MySqlConnectOptions::new()
        .host(&host).port(port).username(&username).password(&input.password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled);
    if !database_name.is_empty() { options = options.database(&database_name); }
    let connect = sqlx::mysql::MySqlConnection::connect_with(&options);
    let mut connection = tokio::time::timeout(std::time::Duration::from_secs(5), connect).await
        .map_err(|_| ApiError(StatusCode::GATEWAY_TIMEOUT, "mysql_connection_timeout", "MySQL/MariaDB connection timed out after five seconds.".into()))?
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, "mysql_connection_failed", format!("MySQL/MariaDB connection failed: {e}")))?;
    use sqlx::{Connection, Column, Row, TypeInfo};
    use futures_util::TryStreamExt;
    sqlx::query("SET SESSION MAX_EXECUTION_TIME=5000").execute(&mut connection).await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, "mysql_timeout_setup_failed", e.to_string()))?;
    sqlx::query("SET TRANSACTION READ ONLY").execute(&mut connection).await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, "mysql_read_only_failed", e.to_string()))?;
    let mut transaction = connection.begin().await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, "mysql_read_only_failed", e.to_string()))?;
    let mut columns: Vec<String> = Vec::new();
    let mut values: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut truncated = false;
    let mut estimated_bytes = 0usize;
    {
        let stream = sqlx::query(sql).fetch(&mut *transaction);
        tokio::pin!(stream);
    while let Some(row) = stream.try_next().await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, "mysql_sql_query_failed", e.to_string()))? {
        if columns.is_empty() {
            columns = row.columns().iter().map(|c| c.name().to_string()).collect();
            if columns.len() > 100 {
                return Err(ApiError(StatusCode::BAD_REQUEST, "too_many_columns", "Queries may return at most 100 columns.".into()));
            }
        }
        if values.len() >= 200 { truncated = true; break; }
        let mut cells = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let type_name = row.column(index).type_info().name().to_ascii_uppercase();
            let value = if type_name.contains("BLOB") || type_name.contains("BINARY") || type_name=="BIT" {
                match row.try_get::<Option<Vec<u8>>, _>(index) {
                    Ok(Some(bytes)) => json!({"type":"binary","hex":bytes.iter().take(4096).map(|b|format!("{b:02x}")).collect::<String>(),"truncated":bytes.len()>4096}),
                    Ok(None) => serde_json::Value::Null,
                    Err(_) => json!({"type":type_name,"unsupported":true}),
                }
            } else {
                match row.try_get::<Option<String>, _>(index) {
                    Ok(Some(text)) => {
                        let shortened: String = text.chars().take(8192).collect();
                        if shortened.len() < text.len() { json!({"value":shortened,"truncated":true}) } else { json!(shortened) }
                    }
                    Ok(None) => serde_json::Value::Null,
                    Err(_) => json!({"type":type_name,"unsupported":true}),
                }
            };
            estimated_bytes = estimated_bytes.saturating_add(value.to_string().len());
            cells.push(value);
        }
        if estimated_bytes > 1024 * 1024 { truncated = true; break; }
        values.push(cells);
        }
    }
    // Dropping the transaction rolls back the read-only query; the connection closes on drop.
    Ok(Json(json!({"columns":columns,"rows":values,"row_count":values.len(),"truncated":truncated,"read_only":true})))
}

#[derive(Debug, Deserialize)]
struct PostgresQuery { password: String, sql: String }

async fn query_postgres_connection(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<PostgresQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let sql = input.sql.trim();
    if sql.is_empty() || sql.len() > 64 * 1024 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_query", "Enter a SQL statement up to 64 KiB.".into()));
    }
    if input.password.len() > 1024 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_postgres_password", "Password is too long.".into()));
    }
    let (host, port, database_name, username) = {
        let db = state.db.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "database_locked", "Database lock is unavailable.".into()))?;
        db.query_row(
            "SELECT host,port,database_name,username FROM postgres_connections WHERE id=?1",
            [&id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, u16>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
        ).map_err(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
            ApiError(StatusCode::NOT_FOUND, "postgres_profile_not_found", "PostgreSQL connection profile was not found.".into())
        } else { database_error(e) })?
    };
    let mut config = tokio_postgres::Config::new();
    config.host(&host).port(port).dbname(&database_name).user(&username)
        .password(&input.password).connect_timeout(std::time::Duration::from_secs(5));
    let (mut client, connection) = config.connect(tokio_postgres::NoTls).await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, "postgres_connection_failed", format!("PostgreSQL connection failed: {e}")))?;
    tokio::spawn(async move { let _ = connection.await; });
    let transaction = client.build_transaction().read_only(true).start().await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, "postgres_read_only_failed", format!("Could not start a read-only transaction: {e}")))?;
    transaction.batch_execute("SET LOCAL statement_timeout = '5s'").await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, "postgres_timeout_setup_failed", e.to_string()))?;
    let statement = transaction.prepare(sql).await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, "postgres_sql_query_failed", e.to_string()))?;
    let column_count = statement.columns().len();
    if column_count > 100 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "too_many_columns", "Queries may return at most 100 columns.".into()));
    }
    let columns: Vec<String> = statement.columns().iter().map(|c| c.name().to_string()).collect();
    let mut stream = transaction.query_raw(
        &statement,
        std::iter::empty::<&(dyn tokio_postgres::types::ToSql + Sync)>(),
    ).await.map_err(|e| ApiError(StatusCode::BAD_REQUEST, "postgres_sql_query_failed", e.to_string()))?;
    use futures_util::TryStreamExt;
    tokio::pin!(stream);
    let mut values: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut truncated = false;
    let mut estimated_bytes = 0usize;
    while let Some(row) = stream.try_next().await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, "postgres_sql_query_failed", e.to_string()))? {
        if values.len() >= 200 { truncated = true; break; }
        let mut cells = Vec::with_capacity(column_count);
        for (index, column) in statement.columns().iter().enumerate() {
            let ty = column.type_();
            let value = if *ty == tokio_postgres::types::Type::BOOL {
                row.try_get::<usize, Option<bool>>(index).ok().flatten().map_or(serde_json::Value::Null, |v| json!(v))
            } else if *ty == tokio_postgres::types::Type::INT2 {
                row.try_get::<usize, Option<i16>>(index).ok().flatten().map_or(serde_json::Value::Null, |v| json!(v))
            } else if *ty == tokio_postgres::types::Type::INT4 {
                row.try_get::<usize, Option<i32>>(index).ok().flatten().map_or(serde_json::Value::Null, |v| json!(v))
            } else if *ty == tokio_postgres::types::Type::INT8 {
                row.try_get::<usize, Option<i64>>(index).ok().flatten().map_or(serde_json::Value::Null, |v| json!(v))
            } else if *ty == tokio_postgres::types::Type::FLOAT4 {
                row.try_get::<usize, Option<f32>>(index).ok().flatten().map_or(serde_json::Value::Null, |v| json!(v))
            } else if *ty == tokio_postgres::types::Type::FLOAT8 {
                row.try_get::<usize, Option<f64>>(index).ok().flatten().map_or(serde_json::Value::Null, |v| json!(v))
            } else if *ty == tokio_postgres::types::Type::BYTEA {
                row.try_get::<usize, Option<Vec<u8>>>(index).ok().flatten().map_or(serde_json::Value::Null, |v| {
                    json!({"type":"bytea","base64":encode_base64(&v[..v.len().min(8192)]),"truncated":v.len()>8192})
                })
            } else {
                match row.try_get::<usize, Option<String>>(index) {
                    Ok(Some(v)) => {
                        let shortened: String = v.chars().take(8192).collect();
                        if shortened.len() < v.len() { json!({"value":shortened,"truncated":true}) } else { json!(shortened) }
                    }
                    Ok(None) => serde_json::Value::Null,
                    Err(_) => json!({"type":ty.name(),"unsupported":true}),
                }
            };
            estimated_bytes = estimated_bytes.saturating_add(value.to_string().len());
            cells.push(value);
        }
        values.push(cells);
        if estimated_bytes > 1024 * 1024 { truncated = true; break; }
    }
    drop(stream);
    let _ = transaction.rollback().await;
    let row_count = values.len();
    Ok(Json(json!({
        "connection_id": id, "columns": columns, "rows": values, "row_count": row_count,
        "truncated": truncated, "read_only": true, "statement_timeout_ms": 5000
    })))
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

#[derive(Debug, Deserialize)]
struct DatabaseQuery { sql: String }
async fn query_database(State(state): State<AppState>, Path(id): Path<String>, Json(input): Json<DatabaseQuery>) -> Result<Json<serde_json::Value>,ApiError> {
    let sql=input.sql.trim();
    if sql.is_empty() || sql.len()>64*1024 {
        return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_query","Enter a SQL statement up to 64 KiB.".into()));
    }
    let path:String={
        let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
        db.query_row("SELECT file_path FROM managed_databases WHERE id=?1 AND engine='sqlite'",[&id],|r|r.get(0))
            .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"database_not_found","Managed SQLite database was not found.".into())}else{database_error(e)})?
    };
    let connection=rusqlite::Connection::open_with_flags(&path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e|ApiError(StatusCode::BAD_GATEWAY,"database_connection_failed",e.to_string()))?;
    let mut statement=connection.prepare(sql).map_err(|e|ApiError(StatusCode::BAD_REQUEST,"sql_query_failed",e.to_string()))?;
    if !statement.readonly() {
        return Err(ApiError(StatusCode::FORBIDDEN,"read_only_query_required","The SQL console only permits read-only statements. Use your application's own database connection for writes.".into()));
    }
    let column_count=statement.column_count();
    if column_count>100 {
        return Err(ApiError(StatusCode::BAD_REQUEST,"too_many_columns","Queries may return at most 100 columns.".into()));
    }
    let columns:Vec<String>=(0..column_count).map(|i|statement.column_name(i).unwrap_or("column").to_string()).collect();
    let mut rows=statement.query([]).map_err(|e|ApiError(StatusCode::BAD_REQUEST,"sql_query_failed",e.to_string()))?;
    let mut values=Vec::new();
    let mut truncated=false;
    let mut estimated_bytes=0usize;
    while let Some(row)=rows.next().map_err(|e|ApiError(StatusCode::BAD_REQUEST,"sql_query_failed",e.to_string()))? {
        if values.len()>=200 {truncated=true;break;}
        let mut cells=Vec::with_capacity(column_count);
        for index in 0..column_count {
            let value=match row.get_ref(index).map_err(database_error)? {
                rusqlite::types::ValueRef::Null=>serde_json::Value::Null,
                rusqlite::types::ValueRef::Integer(v)=>json!(v),
                rusqlite::types::ValueRef::Real(v)=>json!(v),
                rusqlite::types::ValueRef::Text(v)=>json!(String::from_utf8_lossy(&v[..v.len().min(8192)]).to_string()),
                rusqlite::types::ValueRef::Blob(v)=>json!({"type":"blob","base64":encode_base64(&v[..v.len().min(8192)]),"truncated":v.len()>8192}),
            };
            estimated_bytes+=value.to_string().len();
            cells.push(value);
        }
        values.push(cells);
        if estimated_bytes>1024*1024 {truncated=true;break;}
    }
    let row_count=values.len();
    Ok(Json(json!({"database_id":id,"columns":columns,"rows":values,"row_count":row_count,"truncated":truncated,"read_only":true})))
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

async fn export_database(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>,ApiError> {
    let path:String={
        let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
        db.query_row("SELECT file_path FROM managed_databases WHERE id=?1 AND engine='sqlite'",[&id],|r|r.get(0))
            .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"database_not_found","Managed SQLite database was not found.".into())}else{database_error(e)})?
    };
    let connection=rusqlite::Connection::open_with_flags(&path,rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e|ApiError(StatusCode::BAD_GATEWAY,"database_export_failed",e.to_string()))?;
    let (busy,_,_):(i64,i64,i64)=connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(database_error)?;
    if busy!=0{return Err(ApiError(StatusCode::CONFLICT,"database_busy","The database is busy; close active connections and retry the export.".into()));}
    drop(connection);
    let metadata=std::fs::metadata(&path).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_export_failed",e.to_string()))?;
    if metadata.len()>16*1024*1024{return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"database_export_limit","SQLite exports are limited to 16 MiB.".into()));}
    let bytes=std::fs::read(&path).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_export_failed",e.to_string()))?;
    Ok(Json(json!({"database_id":id,"size_bytes":bytes.len(),"content_base64":encode_base64(&bytes)})))
}

async fn import_database(State(state): State<AppState>, Json(input): Json<serde_json::Value>) -> Result<(StatusCode,Json<serde_json::Value>),ApiError> {
    let name=input.get("name").and_then(|v|v.as_str()).unwrap_or("").trim();
    if name.is_empty() || name.len()>48 || !name.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'||b==b'-') {
        return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_database_name","Use 1–48 letters, numbers, underscores or hyphens for the database name.".into()));
    }
    let encoded=input.get("content_base64").and_then(|v|v.as_str()).unwrap_or("");
    let bytes=decode_base64(encoded)?;
    if bytes.is_empty() || bytes.len()>16*1024*1024 {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE,"database_import_limit","SQLite imports must be between 1 byte and 16 MiB.".into()));
    }
    if !bytes.starts_with(b"SQLite format 3\0") {
        return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_sqlite_file","The selected file is not a SQLite database.".into()));
    }
    {
        let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
        if db.query_row("SELECT EXISTS(SELECT 1 FROM managed_databases WHERE name=?1)",[name],|r|r.get::<_,bool>(0)).map_err(database_error)? {
            return Err(ApiError(StatusCode::CONFLICT,"database_name_in_use","A managed database with that name already exists.".into()));
        }
    }
    let directory=state.config_dir.join("databases");
    std::fs::create_dir_all(&directory).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_storage_unavailable",e.to_string()))?;
    let id=Uuid::new_v4().to_string();
    let path=directory.join(format!("{id}.sqlite3"));
    let temp=directory.join(format!("{id}.importing.sqlite3"));
    std::fs::write(&temp,&bytes).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_import_failed",e.to_string()))?;
    let validation=(||->Result<(),ApiError>{
        let connection=rusqlite::Connection::open_with_flags(&temp,rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e|ApiError(StatusCode::BAD_REQUEST,"invalid_sqlite_file",format!("SQLite could not open this file: {e}")))?;
        let result:String=connection.query_row("PRAGMA integrity_check",[],|r|r.get(0))
            .map_err(|e|ApiError(StatusCode::BAD_REQUEST,"invalid_sqlite_file",e.to_string()))?;
        if result!="ok" {return Err(ApiError(StatusCode::BAD_REQUEST,"sqlite_integrity_failed",format!("SQLite integrity check failed: {result}")));}
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;").map_err(database_error)?;
        drop(connection);
        Ok(())
    })();
    if let Err(error)=validation {let _=std::fs::remove_file(&temp);return Err(error);}
    if let Err(error)=std::fs::rename(&temp,&path) {let _=std::fs::remove_file(&temp);return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_import_failed",error.to_string()));}
    let path_text=path.to_string_lossy().to_string();
    let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
    if db.query_row("SELECT EXISTS(SELECT 1 FROM managed_databases WHERE name=?1)",[name],|r|r.get::<_,bool>(0)).map_err(database_error)? {
        drop(db);let _=std::fs::remove_file(&path);
        return Err(ApiError(StatusCode::CONFLICT,"database_name_in_use","A managed database with that name already exists.".into()));
    }
    if let Err(error)=db.execute("INSERT INTO managed_databases(id,name,engine,file_path) VALUES (?1,?2,'sqlite',?3)",params![id,name,path_text]) {
        drop(db);let _=std::fs::remove_file(&path);return Err(database_error(error));
    }
    let created_at:String=db.query_row("SELECT created_at FROM managed_databases WHERE id=?1",[&id],|r|r.get(0)).map_err(database_error)?;
    Ok((StatusCode::CREATED,Json(json!({"id":id,"name":name,"engine":"sqlite","size_bytes":bytes.len(),"created_at":created_at}))))
}

async fn system_metrics()->Result<Json<serde_json::Value>,ApiError>{
    let metrics=tokio::task::spawn_blocking(crate::monitor::snapshot).await
        .map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"metrics_collection_failed",e.to_string()))?;
    Ok(Json(metrics))
}

async fn networking_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>,ApiError> {
    let lan_ip=crate::network::lan_ipv4().map(|ip|ip.to_string());
    let public_exposure_enabled=crate::settings::setting_bool(&state,"public_exposure_enabled",false);
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
    Ok(Json(json!({"lan_ipv4":lan_ip,"websites":websites,"management_api":"loopback_only","public_exposure_enabled":public_exposure_enabled})))
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

async fn diagnose_domain(State(state): State<AppState>, Json(input): Json<DiagnoseDomain>) -> Result<Json<serde_json::Value>, ApiError> {
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
        "public_exposure_enabled":crate::settings::setting_bool(&state,"public_exposure_enabled",false)})))
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
    let _operation=runtime_operation_guard(&state,&id).await?;
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
async fn runtime_operation_guard(state:&AppState,id:&str)->Result<tokio::sync::OwnedMutexGuard<()>,ApiError>{
    let lock={
        let mut locks=state.runtime_operation_locks.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"runtime_lock_registry_unavailable","Website operation lock registry is unavailable.".into()))?;
        locks.entry(id.to_string()).or_insert_with(||Arc::new(tokio::sync::Mutex::new(()))).clone()
    };
    Ok(lock.lock_owned().await)
}
fn stop_process(state:&AppState,id:&str)->Result<(),ApiError>{
    let mut map=state.processes.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"process_registry_locked","Process registry is unavailable.".into()))?;
    if let Some(mut child)=map.remove(id){
        if let Ok(mut times)=state.process_started_at.lock(){times.remove(id);}
        let pid=child.id();
        #[cfg(windows)]
        { let _=Command::new("taskkill").args(["/PID",&pid.to_string(),"/T","/F"]).output(); }
        #[cfg(not(windows))]
        { let _=child.kill(); }
        let _=child.wait();
    }
    Ok(())
}
#[derive(Debug,Deserialize)]
struct SaveRuntimeProfile { website_id:String, program:String, args:Vec<String> }
async fn list_runtime_profiles(State(state):State<AppState>)->Result<Json<Vec<serde_json::Value>>,ApiError>{
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let mut q=db.prepare("SELECT p.id,p.website_id,w.name,p.program,p.args_json,p.updated_at FROM runtime_profiles p JOIN websites w ON w.id=p.website_id ORDER BY w.name").map_err(database_error)?;
 let rows=q.query_map([],|r|{let raw:String=r.get(4)?;let args:Vec<String>=serde_json::from_str(&raw).unwrap_or_default();Ok(json!({"id":r.get::<_,String>(0)?,"website_id":r.get::<_,String>(1)?,"website_name":r.get::<_,String>(2)?,"program":r.get::<_,String>(3)?,"args":args,"updated_at":r.get::<_,String>(5)?}))}).map_err(database_error)?;
 Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(database_error)?))
}
async fn save_runtime_profile(State(state):State<AppState>,Json(input):Json<SaveRuntimeProfile>)->Result<Json<serde_json::Value>,ApiError>{
 let program=input.program.trim();
 if program.is_empty()||program.len()>512||program.contains('\0'){return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_program","Executable must contain 1–512 valid characters.".into()));}
 if input.args.len()>64||input.args.iter().any(|a|a.len()>1024||a.contains('\0'))||input.args.iter().map(String::len).sum::<usize>()>8192{return Err(ApiError(StatusCode::BAD_REQUEST,"invalid_arguments","Use at most 64 arguments, each up to 1,024 characters and 8 KiB total.".into()));}
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let (runtime,status):(String,String)=db.query_row("SELECT runtime,status FROM websites WHERE id=?1",[&input.website_id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into())}else{database_error(e)})?;
 if runtime=="static"{return Err(ApiError(StatusCode::BAD_REQUEST,"static_profile_unsupported","Static websites do not use a launch command.".into()));}
 if status!="stopped"{return Err(ApiError(StatusCode::CONFLICT,"website_must_be_stopped","Stop the website before changing its launch profile.".into()));}
 let id=Uuid::new_v4().to_string();let args_json=serde_json::to_string(&input.args).map_err(|e|ApiError(StatusCode::BAD_REQUEST,"invalid_arguments",e.to_string()))?;
 db.execute("INSERT INTO runtime_profiles(id,website_id,program,args_json) VALUES(?1,?2,?3,?4) ON CONFLICT(website_id) DO UPDATE SET program=excluded.program,args_json=excluded.args_json,updated_at=CURRENT_TIMESTAMP",params![id,input.website_id,program,args_json]).map_err(database_error)?;
 let (profile_id,updated_at):(String,String)=db.query_row("SELECT id,updated_at FROM runtime_profiles WHERE website_id=?1",[&input.website_id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(database_error)?;
 Ok(Json(json!({"id":profile_id,"website_id":input.website_id,"program":program,"args":input.args,"updated_at":updated_at,"saved":true})))
}
async fn delete_runtime_profile(State(state):State<AppState>,Path(website_id):Path<String>)->Result<StatusCode,ApiError>{
 let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let status:String=db.query_row("SELECT status FROM websites WHERE id=?1",[&website_id],|r|r.get(0)).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){ApiError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into())}else{database_error(e)})?;
 if status!="stopped"{return Err(ApiError(StatusCode::CONFLICT,"website_must_be_stopped","Stop the website before removing its launch profile.".into()));}
 if db.execute("DELETE FROM runtime_profiles WHERE website_id=?1",[website_id]).map_err(database_error)?==0{return Err(ApiError(StatusCode::NOT_FOUND,"runtime_profile_not_found","No custom launch profile exists for this website.".into()));}
 Ok(StatusCode::NO_CONTENT)
}
// Require an actual HTTP response, not merely a TCP listener. Redirects and
// client errors still prove the server is responding; 5xx responses do not.
fn http_status_is_healthy(status: u16) -> bool {
    (200..500).contains(&status)
}

fn http_readiness_probe(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(250)) else {
        return false;
    };
    let timeout = Some(Duration::from_millis(500));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    if stream.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").is_err() {
        return false;
    }
    let mut response = [0u8; 512];
    let Ok(count) = stream.read(&mut response) else {
        return false;
    };
    let first_line = String::from_utf8_lossy(&response[..count]);
    let Some(status) = first_line.lines().next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok()) else {
            return false;
        };
    http_status_is_healthy(status)
}

const MAX_RUNTIME_LOG_BYTES:u64=10*1024*1024;
const RUNTIME_LOG_TRUNCATED_MARKER:&[u8]=b"\n[WebDynamite] Runtime log limit reached (10 MiB); further output is discarded.\n";
const MAX_RUNTIME_LOG_CONTENT_BYTES:u64=MAX_RUNTIME_LOG_BYTES-RUNTIME_LOG_TRUNCATED_MARKER.len() as u64;

fn reserve_runtime_log_bytes(counter:&AtomicU64,requested:usize)->usize{
    loop{
        let current=counter.load(Ordering::Relaxed);
        if current>=MAX_RUNTIME_LOG_CONTENT_BYTES{return 0;}
        let allowed=(MAX_RUNTIME_LOG_CONTENT_BYTES-current).min(requested as u64);
        if counter.compare_exchange_weak(current,current+allowed,Ordering::Relaxed,Ordering::Relaxed).is_ok(){return allowed as usize;}
    }
}

fn pump_runtime_log<R:Read+Send+'static>(mut reader:R,file:Arc<Mutex<File>>,written:Arc<AtomicU64>,truncated:Arc<AtomicBool>){
    std::thread::spawn(move||{
        let mut buffer=[0u8;8192];
        loop{
            let count=match reader.read(&mut buffer){Ok(0)|Err(_)=>break,Ok(count)=>count};
            let allowed=reserve_runtime_log_bytes(&written,count);
            if allowed>0{
                let Ok(mut output)=file.lock() else{break;};
                if output.write_all(&buffer[..allowed]).is_err(){break;}
            }
            if allowed<count&&!truncated.swap(true,Ordering::Relaxed){
                if let Ok(mut output)=file.lock(){let _=output.write_all(RUNTIME_LOG_TRUNCATED_MARKER);}
            }
        }
    });
}

async fn start_website(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let _operation=runtime_operation_guard(&state,&id).await?;
    start_website_inner(state.clone(),id).await
}
async fn start_website_inner(state:AppState,id:String)->Result<Json<serde_json::Value>,ApiError>{
    let (mut project,runtime)=project_details(&state,&id)?;
    if !FsPath::new(&project).is_dir(){return Err(ApiError(StatusCode::BAD_REQUEST,"project_not_found","The project directory is no longer available.".into()));}
    stop_process(&state,&id)?;
    if runtime!="static" {
        let active_release:Option<String>={
            let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
            db.query_row("SELECT release_path FROM releases WHERE website_id=?1 AND active=1",params![id],|row|row.get(0)).optional().map_err(database_error)?
        };
        if let Some(path)=active_release {
            let releases_root=state.releases_dir.canonicalize().map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"release_store_unavailable",e.to_string()))?;
            let snapshot=FsPath::new(&path).canonicalize().map_err(|e|ApiError(StatusCode::BAD_REQUEST,"active_release_missing",format!("The active runtime release is unavailable: {e}")))?;
            if !snapshot.starts_with(&releases_root)||snapshot==releases_root||!snapshot.is_dir(){return Err(ApiError(StatusCode::FORBIDDEN,"active_release_path_forbidden","The active runtime release is outside the managed release store.".into()));}
            project=snapshot.to_string_lossy().to_string();
        }
    }
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
    let log_file=Arc::new(Mutex::new(File::create(&log_path).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_file_error",e.to_string()))?));
    let log_bytes=Arc::new(AtomicU64::new(0));
    let log_truncated=Arc::new(AtomicBool::new(false));
    let custom_profile:Option<(String,Vec<String>)>={
        let db=state.db.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
        db.query_row("SELECT program,args_json FROM runtime_profiles WHERE website_id=?1",[&id],|r|{let raw:String=r.get(1)?;let args:Vec<String>=serde_json::from_str(&raw).unwrap_or_default();Ok((r.get(0)?,args))}).optional().map_err(database_error)?
    };
    let mut command=if let Some((program,args))=custom_profile {
        let mut c=Command::new(program);
        let expanded=args.into_iter().map(|arg|arg.replace("{project}",&project).replace("{port}",&port.to_string())).collect::<Vec<_>>();
        c.args(expanded);
        c
    } else if runtime=="php" {
        let php=crate::settings::setting_string(&state,"php_executable","php");
        let artisan=FsPath::new(&project).join("artisan");
        let public_index=FsPath::new(&project).join("public").join("index.php");
        let mut c=Command::new(php);
        if artisan.is_file() && public_index.is_file() {
            c.args(["artisan","serve","--host","127.0.0.1","--port",&port.to_string()]);
        } else {
            c.args(["-S",&format!("127.0.0.1:{port}"),"-t",&project]);
        }
        c
    } else if runtime=="python" {
        let python=crate::settings::setting_string(&state,"python_executable","python");
        let root=FsPath::new(&project);
        let mut c=Command::new(python);
        if root.join("manage.py").is_file() {
            c.args(["manage.py","runserver",&format!("127.0.0.1:{port}")]);
        } else {
            let candidates=["main.py","app.py","wsgi.py"];
            let detected=candidates.iter().find_map(|name|{
                let path=root.join(name);
                let source=std::fs::read_to_string(&path).ok()?;
                if source.contains("FastAPI(") { Some((*name,"fastapi")) }
                else if source.contains("Flask(") { Some((*name,"flask")) }
                else { None }
            });
            match detected {
                Some((file,"fastapi")) => {
                    let module=file.trim_end_matches(".py");
                    c.args(["-m","uvicorn",&format!("{module}:app"),"--host","127.0.0.1","--port",&port.to_string()]);
                }
                Some((file,"flask")) => {
                    c.args(["-m","flask","--app",file,"run","--host","127.0.0.1","--port",&port.to_string()]);
                }
                _ => {
                    c.args(["-m","http.server",&port.to_string(),"--bind","127.0.0.1","--directory",&project]);
                }
            }
        }
        c
    } else {
        #[cfg(windows)]
        { let mut c=Command::new("cmd");c.args(["/C","npm run start"]);c }
        #[cfg(not(windows))]
        { let mut c=Command::new("npm");c.args(["run","start"]);c }
    };
    let mut child=command.current_dir(&project).env("PORT",port.to_string()).env("HOST","127.0.0.1").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
      .map_err(|e|ApiError(StatusCode::BAD_REQUEST,"runtime_start_failed",if runtime=="php"{format!("Could not start PHP. Verify PHP is installed and available on PATH: {e}")}else if runtime=="python"{format!("Could not start Python. Verify Python is installed and available as `python` on PATH: {e}")}else{format!("Could not start npm. Verify Node.js and npm are installed: {e}")}))?;
    if let Some(stdout)=child.stdout.take(){pump_runtime_log(stdout,log_file.clone(),log_bytes.clone(),log_truncated.clone());}
    if let Some(stderr)=child.stderr.take(){pump_runtime_log(stderr,log_file,log_bytes,log_truncated);}
    let pid=child.id();
    // A successful spawn is not proof that the application is serving requests.
    // Wait for the assigned loopback port and detect early process exits before
    // reporting the website as running to the desktop client.
    let deadline=Instant::now()+Duration::from_secs(15);
    let mut ready=false;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let state_name=if status.success(){"stopped"}else{"crashed"};
                let _=set_status(&state,&id,state_name);
                return Err(ApiError(StatusCode::BAD_GATEWAY,"runtime_exited_during_startup",format!("The {runtime} process exited during startup (exit code: {}). Review the website logs.",status.code().map(|v|v.to_string()).unwrap_or_else(||"unknown".into()))));
            }
            Err(error) => {
                let _=child.kill(); let _=child.wait();
                let _=set_status(&state,&id,"failed");
                return Err(ApiError(StatusCode::BAD_GATEWAY,"runtime_startup_check_failed",format!("Could not monitor the {runtime} process during startup: {error}")));
            }
            Ok(None) => {}
        }
        if http_readiness_probe(port) {
            ready=true;
            break;
        }
        if Instant::now()>=deadline { break; }
        std::thread::sleep(Duration::from_millis(200));
    }
    if !ready {
        let _=child.kill(); let _=child.wait();
        let _=set_status(&state,&id,"failed");
        return Err(ApiError(StatusCode::GATEWAY_TIMEOUT,"runtime_startup_timeout",format!("The {runtime} process did not begin accepting connections on 127.0.0.1:{port} within 15 seconds. Review the website logs and launch profile.")));
    }
    state.processes.lock().map_err(|_|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"process_registry_locked","Process registry is unavailable.".into()))?.insert(id.clone(),child);
    if let Ok(mut times)=state.process_started_at.lock(){times.insert(id.clone(),Instant::now());}
    set_status(&state,&id,"running")?;
    Ok(Json(json!({"id":id,"status":"running","health":"accepting_connections","pid":pid,"port":port,"url":format!("http://127.0.0.1:{port}"),"log_path":log_path})))
}
pub(crate) async fn restart_website_for_deployment(state:AppState,id:String)->Result<(),String>{
    let _operation=runtime_operation_guard(&state,&id).await.map_err(|error|format!("{}: {}",error.1,error.2))?;
    start_website_inner(state,id).await.map(|_|()).map_err(|error|format!("{}: {}",error.1,error.2))
}
async fn stop_website(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let _operation=runtime_operation_guard(&state,&id).await?;
    let _=project_details(&state,&id)?; stop_process(&state,&id)?; set_status(&state,&id,"stopped")?;
    Ok(Json(json!({"id":id,"status":"stopped"})))
}
async fn restart_website(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let _operation=runtime_operation_guard(&state,&id).await?;
    let _=project_details(&state,&id)?; stop_process(&state,&id)?; start_website_inner(state.clone(),id).await
}
async fn website_logs(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<serde_json::Value>,ApiError>{
    let _=project_details(&state,&id)?;
    let path=state.logs_dir.join(format!("{id}.log"));
    if !path.exists(){return Ok(Json(json!({"id":id,"logs":"","message":"No process log has been created yet."})));}
    let mut file=File::open(path).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_read_error",e.to_string()))?;
    let length=file.metadata().map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_read_error",e.to_string()))?.len();
    let start=length.saturating_sub(256*1024);
    file.seek(SeekFrom::Start(start)).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_read_error",e.to_string()))?;
    let mut bytes=Vec::with_capacity(length.saturating_sub(start) as usize);
    file.read_to_end(&mut bytes).map_err(|e|ApiError(StatusCode::INTERNAL_SERVER_ERROR,"log_read_error",e.to_string()))?;
    let mut tail=String::from_utf8_lossy(&bytes).to_string();
    if start>0{tail=format!("[Earlier runtime log output omitted; showing the last 256 KiB.]\n{tail}");}
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
    use super::{valid_local_hostname, reserve_runtime_log_bytes, MAX_RUNTIME_LOG_BYTES, MAX_RUNTIME_LOG_CONTENT_BYTES, RUNTIME_LOG_TRUNCATED_MARKER};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn runtime_log_byte_reservation_never_exceeds_the_limit(){
        let counter=AtomicU64::new(MAX_RUNTIME_LOG_CONTENT_BYTES-3);
        assert_eq!(reserve_runtime_log_bytes(&counter,8),3);
        assert_eq!(reserve_runtime_log_bytes(&counter,8),0);
        assert_eq!(counter.load(Ordering::Relaxed),MAX_RUNTIME_LOG_CONTENT_BYTES);
        assert_eq!(counter.load(Ordering::Relaxed)+RUNTIME_LOG_TRUNCATED_MARKER.len() as u64,MAX_RUNTIME_LOG_BYTES);
    }

    #[test]
    fn runtime_log_limit_is_ten_mibibytes(){
        assert_eq!(MAX_RUNTIME_LOG_BYTES,10*1024*1024);
        assert!(RUNTIME_LOG_TRUNCATED_MARKER.windows(6).any(|part|part==b"10 MiB"));
    }

    #[test]
    fn accepts_http_success_redirect_and_client_error_responses() {
        for status in [200, 204, 301, 302, 400, 404, 499] {
            assert!(super::http_status_is_healthy(status));
        }
    }

    #[test]
    fn rejects_server_errors_and_invalid_status_codes() {
        for status in [0, 100, 500, 502, 503, 599, 600] {
            assert!(!super::http_status_is_healthy(status));
        }
    }

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


pub(crate) async fn restart_after_crash(state: AppState, id: String) -> bool {
    let Ok(_operation)=runtime_operation_guard(&state,&id).await else{return false;};
    if !crate::settings::setting_bool(&state,"runtime_auto_restart",false){return false;}
    let still_crashed=state.db.lock().ok().and_then(|db|db.query_row("SELECT status FROM websites WHERE id=?1",[&id],|row|row.get::<_,String>(0)).ok()).is_some_and(|status|status=="error");
    if !still_crashed{return false;}
    start_website_inner(state,id).await.is_ok()
}
