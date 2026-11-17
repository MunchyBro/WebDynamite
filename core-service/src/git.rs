use axum::{extract::{Path,State},routing::get,Json,Router,http::StatusCode,response::IntoResponse};
use rusqlite::params;
use serde_json::{json,Value};
use std::{path::PathBuf,process::Command};
use crate::AppState;

#[derive(Debug)]
struct GitError(StatusCode,&'static str,String);
impl IntoResponse for GitError{
 fn into_response(self)->axum::response::Response{(self.0,Json(json!({"error":{"code":self.1,"message":self.2}}))).into_response()}
}
fn db_error(e:rusqlite::Error)->GitError{GitError(StatusCode::INTERNAL_SERVER_ERROR,"database_error",e.to_string())}
fn git_output(root:&PathBuf,args:&[&str])->Result<String,GitError>{
 let output=Command::new("git").arg("-C").arg(root).args(args)
  .env("GIT_TERMINAL_PROMPT","0").env("GCM_INTERACTIVE","Never")
  .output().map_err(|_|GitError(StatusCode::BAD_GATEWAY,"git_unavailable","Git could not be started. Install Git for Windows and ensure git.exe is on the Core Service PATH.".into()))?;
 if !output.status.success(){
  return Err(GitError(StatusCode::CONFLICT,"git_operation_failed","Git rejected the operation. Check repository state, remote configuration and credentials in your Git client; sensitive command output is intentionally not returned.".into()));
 }
 Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
fn project_root(state:&AppState,id:&str)->Result<PathBuf,GitError>{
 let path:String={let db=state.db.lock().map_err(|_|GitError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  db.query_row("SELECT project_path FROM websites WHERE id=?1",params![id],|r|r.get(0))
   .map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){GitError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into())}else{db_error(e)})?};
 let root=PathBuf::from(path).canonicalize().map_err(|e|GitError(StatusCode::NOT_FOUND,"project_folder_unavailable",e.to_string()))?;
 if !root.is_dir(){return Err(GitError(StatusCode::BAD_REQUEST,"project_not_directory","Website project path is not a directory.".into()));}
 git_output(&root,&["rev-parse","--show-toplevel"])?;
 Ok(root)
}
fn status_json(root:&PathBuf)->Result<Value,GitError>{
 let branch=git_output(root,&["branch","--show-current"])?;
 let commit=git_output(root,&["rev-parse","HEAD"])?;
 let changes=git_output(root,&["status","--short"])?;
 let top=git_output(root,&["rev-parse","--show-toplevel"])?;
 Ok(json!({"repository_root":top,"branch":branch,"commit":commit,"changes":changes,"clean":changes.is_empty()}))
}
pub fn router()->Router<AppState>{
 Router::new()
  .route("/api/v1/git/:website_id/status",get(git_status))
  .route("/api/v1/git/:website_id/fetch",axum::routing::post(git_fetch))
  .route("/api/v1/git/:website_id/pull",axum::routing::post(git_pull))
}
async fn git_status(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<Value>,GitError>{
 let root=project_root(&state,&id)?;
 let value=tokio::task::spawn_blocking(move||status_json(&root)).await.map_err(|e|GitError(StatusCode::INTERNAL_SERVER_ERROR,"git_task_failed",e.to_string()))??;
 Ok(Json(value))
}
async fn git_fetch(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<Value>,GitError>{
 let root=project_root(&state,&id)?;
 tokio::task::spawn_blocking(move||git_output(&root,&["fetch","--prune","--no-tags"])).await.map_err(|e|GitError(StatusCode::INTERNAL_SERVER_ERROR,"git_task_failed",e.to_string()))??;
 let root=project_root(&state,&id)?;
 let value=status_json(&root)?;
 Ok(Json(json!({"operation":"fetch","message":"Remote references fetched successfully.","status":value})))
}
async fn git_pull(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<Value>,GitError>{
 let root=project_root(&state,&id)?;
 tokio::task::spawn_blocking(move||git_output(&root,&["pull","--ff-only"])).await.map_err(|e|GitError(StatusCode::INTERNAL_SERVER_ERROR,"git_task_failed",e.to_string()))??;
 let root=project_root(&state,&id)?;
 let value=status_json(&root)?;
 Ok(Json(json!({"operation":"pull","message":"Fast-forward update completed successfully.","status":value})))
}
