use axum::{extract::{Path, Query, State}, routing::get, Json, Router, http::StatusCode, response::IntoResponse};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs, path::{Path as FsPath, PathBuf}, process::Command, time::Instant};
use uuid::Uuid;
use crate::AppState;

#[derive(Debug)]
struct DeployError(StatusCode, &'static str, String);
impl axum::response::IntoResponse for DeployError {
    fn into_response(self)->axum::response::Response {
        (self.0,Json(json!({"error":{"code":self.1,"message":self.2}}))).into_response()
    }
}
fn db_error(e:rusqlite::Error)->DeployError{DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_error",e.to_string())}
#[derive(Deserialize)]
struct SaveProfile{website_id:String,program:String,args:Vec<String>,output_dir:String}
#[derive(Deserialize)]
struct RunsQuery{website_id:Option<String>}
#[derive(Serialize)]
struct Profile{id:String,website_id:String,website_name:String,program:String,args:Vec<String>,output_dir:String,updated_at:String}
#[derive(Serialize)]
struct BuildRun{id:String,website_id:String,website_name:String,program:String,status:String,exit_code:Option<i32>,duration_ms:i64,logs:String,created_at:String}\n#[derive(Serialize)]\nstruct Release{id:String,website_id:String,website_name:String,source:String,commit_hash:Option<String>,active:bool,file_count:u64,size_bytes:u64,created_at:String}
fn allowed_program(program:&str)->bool{matches!(program,"node"|"python"|"php"|"cargo"|"dotnet")}
fn safe_output_dir(value:&str)->bool{
 let p=FsPath::new(value);
 !value.trim().is_empty()&&!p.is_absolute()&&p.components().all(|c|matches!(c,std::path::Component::Normal(_)))
}
pub fn router()->Router<AppState>{
 Router::new()
  .route("/api/v1/deployments/profiles",get(list_profiles).put(save_profile))
  .route("/api/v1/deployments/runs",get(list_runs))\n  .route("/api/v1/deployments/releases",get(list_releases))\n  .route("/api/v1/deployments/:website_id/release",axum::routing::post(create_release))\n  .route("/api/v1/deployments/:website_id/rollback/:release_id",axum::routing::post(rollback_release))
  .route("/api/v1/deployments/:website_id/run",axum::routing::post(run_build))
}
async fn list_profiles(State(state):State<AppState>)->Result<Json<Vec<Profile>>,DeployError>{
 let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let mut q=db.prepare("SELECT p.id,p.website_id,w.name,p.program,p.args_json,p.output_dir,p.updated_at FROM build_profiles p JOIN websites w ON w.id=p.website_id ORDER BY w.name").map_err(db_error)?;
 let rows=q.query_map([],|r|{let args:String=r.get(4)?;Ok(Profile{id:r.get(0)?,website_id:r.get(1)?,website_name:r.get(2)?,program:r.get(3)?,args:serde_json::from_str(&args).unwrap_or_default(),output_dir:r.get(5)?,updated_at:r.get(6)?})}).map_err(db_error)?;
 Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(db_error)?))
}
async fn save_profile(State(state):State<AppState>,Json(input):Json<SaveProfile>)->Result<Json<Profile>,DeployError>{
 let program=input.program.trim();
 if !allowed_program(program){return Err(DeployError(StatusCode::BAD_REQUEST,"unsupported_build_program","Choose a supported executable: node, python, php, cargo or dotnet. Shell scripts and command strings are not accepted." .into()));}
 if input.args.len()>64||input.args.iter().any(|a|a.len()>2048||a.contains('\0')){return Err(DeployError(StatusCode::BAD_REQUEST,"invalid_build_args","Use at most 64 arguments, each no longer than 2048 characters.".into()));}
 if !safe_output_dir(&input.output_dir){return Err(DeployError(StatusCode::BAD_REQUEST,"invalid_output_dir","Build output must be a relative folder path without traversal components.".into()));}
 let args_json=serde_json::to_string(&input.args).map_err(|e|DeployError(StatusCode::BAD_REQUEST,"invalid_build_args",e.to_string()))?;
 let id=Uuid::new_v4().to_string();
 let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let website_name:String=db.query_row("SELECT name FROM websites WHERE id=?1",params![input.website_id],|r|r.get(0)).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){DeployError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into())}else{db_error(e)})?;
 db.execute("INSERT INTO build_profiles(id,website_id,program,args_json,output_dir,updated_at) VALUES(?1,?2,?3,?4,?5,CURRENT_TIMESTAMP) ON CONFLICT(website_id) DO UPDATE SET program=excluded.program,args_json=excluded.args_json,output_dir=excluded.output_dir,updated_at=CURRENT_TIMESTAMP",params![id,input.website_id,program,args_json,input.output_dir]).map_err(db_error)?;
 let profile=db.query_row("SELECT id,website_id,program,args_json,output_dir,updated_at FROM build_profiles WHERE website_id=?1",params![input.website_id],|r|{let args:String=r.get(3)?;Ok(Profile{id:r.get(0)?,website_id:r.get(1)?,website_name:website_name.clone(),program:r.get(2)?,args:serde_json::from_str(&args).unwrap_or_default(),output_dir:r.get(4)?,updated_at:r.get(5)?})}).map_err(db_error)?;
 Ok(Json(profile))
}
async fn list_runs(State(state):State<AppState>,Query(query):Query<RunsQuery>)->Result<Json<Vec<BuildRun>>,DeployError>{
 let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let sql=if query.website_id.is_some(){"SELECT r.id,r.website_id,w.name,r.program,r.status,r.exit_code,r.duration_ms,r.logs,r.created_at FROM build_runs r JOIN websites w ON w.id=r.website_id WHERE r.website_id=?1 ORDER BY r.created_at DESC LIMIT 50"}else{"SELECT r.id,r.website_id,w.name,r.program,r.status,r.exit_code,r.duration_ms,r.logs,r.created_at FROM build_runs r JOIN websites w ON w.id=r.website_id ORDER BY r.created_at DESC LIMIT 50"};
 let mut q=db.prepare(sql).map_err(db_error)?;
 let map=|r:&rusqlite::Row<'_>|Ok(BuildRun{id:r.get(0)?,website_id:r.get(1)?,website_name:r.get(2)?,program:r.get(3)?,status:r.get(4)?,exit_code:r.get(5)?,duration_ms:r.get(6)?,logs:r.get(7)?,created_at:r.get(8)?});
 let rows=if let Some(id)=query.website_id{q.query_map(params![id],map).map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?}else{q.query_map([],map).map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?};
 Ok(Json(rows))
}
async fn run_build(State(state):State<AppState>,Path(website_id):Path<String>)->Result<(StatusCode,Json<BuildRun>),DeployError>{
 let (root,profile):(PathBuf,(String,Vec<String>,String,String))={
  let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  let root:String=db.query_row("SELECT project_path FROM websites WHERE id=?1",params![website_id],|r|r.get(0)).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){DeployError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into())}else{db_error(e)})?;
  let p=db.query_row("SELECT program,args_json,output_dir,id FROM build_profiles WHERE website_id=?1",params![website_id],|r|{let args:String=r.get(1)?;Ok((r.get(0)?,serde_json::from_str(&args).unwrap_or_default(),r.get(2)?,r.get(3)?))}).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){DeployError(StatusCode::NOT_FOUND,"build_profile_missing","Save a build profile before running a build.".into())}else{db_error(e)})?;
  (PathBuf::from(root),p)
 };
 let canonical=root.canonicalize().map_err(|e|DeployError(StatusCode::BAD_REQUEST,"website_folder_unavailable",e.to_string()))?;
 if !safe_output_dir(&profile.2){return Err(DeployError(StatusCode::BAD_REQUEST,"invalid_output_dir","Build output path is invalid.".into()));}
 let started=Instant::now();
 let result=Command::new(&profile.0).args(&profile.1).current_dir(&canonical).output()
  .map_err(|e|DeployError(StatusCode::BAD_GATEWAY,"build_start_failed",format!("Could not start {}: {e}",profile.0)))?;
 let elapsed=started.elapsed().as_millis().min(i64::MAX as u128) as i64;
 let mut logs=String::from_utf8_lossy(&result.stdout).to_string();
 logs.push_str(&String::from_utf8_lossy(&result.stderr));
 if logs.len()>262_144{logs=format!("{}\n… build log truncated to 256 KiB",String::from_utf8_lossy(&logs.as_bytes()[logs.len()-262_144..]));}
 let mut status=if result.status.success(){"success"}else{"failed"};
 let mut exit_code=result.status.code();
 if result.status.success(){
  let output=canonical.join(&profile.2);
  match output.canonicalize(){
   Ok(path) if path.starts_with(&canonical)&&path.is_dir()=>{},
   _=>{status="failed";exit_code=Some(1);logs.push_str("\nBuild command exited successfully, but the configured output folder was not found inside the website project.");}
  }
 }
 let id=Uuid::new_v4().to_string();
 let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 db.execute("INSERT INTO build_runs(id,website_id,program,status,exit_code,duration_ms,logs) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,website_id,profile.0,status,exit_code,elapsed,logs]).map_err(db_error)?;
 let website_name:String=db.query_row("SELECT name FROM websites WHERE id=?1",params![website_id],|r|r.get(0)).map_err(db_error)?;
 let created_at:String=db.query_row("SELECT created_at FROM build_runs WHERE id=?1",params![id],|r|r.get(0)).map_err(db_error)?;
Ok((if status=="success"{StatusCode::OK}else{StatusCode::UNPROCESSABLE_ENTITY},Json(BuildRun{id,website_id,website_name,program:profile.0,status:status.into(),exit_code,duration_ms:elapsed,logs,created_at})))
}

fn copy_release_tree(source:&FsPath,destination:&FsPath,counts:&mut (u64,u64))->Result<(),DeployError>{
 for entry in fs::read_dir(source).map_err(|e|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"release_read_failed",e.to_string()))?{
  let entry=entry.map_err(|e|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"release_read_failed",e.to_string()))?;
  let kind=entry.file_type().map_err(|e|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"release_metadata_failed",e.to_string()))?;
  let name=entry.file_name().to_string_lossy().to_ascii_lowercase();
  if name==".git"||name==".env"||name.starts_with(".env.")||name.ends_with(".pem")||name.ends_with(".key")||name.ends_with(".p12")||name.ends_with(".pfx")||name=="id_rsa"||name=="id_ed25519"{
   return Err(DeployError(StatusCode::FORBIDDEN,"release_sensitive_file","Build output contains a sensitive file or directory; remove it before creating a release.".into()));
  }
  if kind.is_symlink(){return Err(DeployError(StatusCode::FORBIDDEN,"release_symlink_forbidden","Build output contains a symbolic link; release snapshot was cancelled.".into()));}
  let target=destination.join(entry.file_name());
  if kind.is_dir(){fs::create_dir_all(&target).map_err(|e|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"release_create_failed",e.to_string()))?;copy_release_tree(&entry.path(),&target,counts)?;}
  else if kind.is_file(){
   let size=entry.metadata().map_err(|e|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"release_metadata_failed",e.to_string()))?.len();
   counts.0+=1;counts.1=counts.1.saturating_add(size);
   if counts.0>50000||counts.1>2*1024*1024*1024{return Err(DeployError(StatusCode::PAYLOAD_TOO_LARGE,"release_too_large","Release snapshot is limited to 50,000 files and 2 GiB.".into()));}
   fs::copy(entry.path(),&target).map_err(|e|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"release_copy_failed",e.to_string()))?;
  }else{return Err(DeployError(StatusCode::FORBIDDEN,"release_special_file_forbidden","Build output contains a special file; release snapshot was cancelled.".into()));}
 }
 Ok(())
}
async fn list_releases(State(state):State<AppState>,Query(query):Query<RunsQuery>)->Result<Json<Vec<Release>>,DeployError>{
 let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let sql=if query.website_id.is_some(){"SELECT r.id,r.website_id,w.name,r.source,r.commit_hash,r.active,r.file_count,r.size_bytes,r.created_at FROM releases r JOIN websites w ON w.id=r.website_id WHERE r.website_id=?1 ORDER BY r.created_at DESC LIMIT 50"}else{"SELECT r.id,r.website_id,w.name,r.source,r.commit_hash,r.active,r.file_count,r.size_bytes,r.created_at FROM releases r JOIN websites w ON w.id=r.website_id ORDER BY r.created_at DESC LIMIT 100"};
 let mut q=db.prepare(sql).map_err(db_error)?;
 let map=|r:&rusqlite::Row<'_>|Ok(Release{id:r.get(0)?,website_id:r.get(1)?,website_name:r.get(2)?,source:r.get(3)?,commit_hash:r.get(4)?,active:r.get::<_,i64>(5)?==1,file_count:r.get::<_,i64>(6)? as u64,size_bytes:r.get::<_,i64>(7)? as u64,created_at:r.get(8)?});
 let rows=if let Some(id)=query.website_id{q.query_map(params![id],map).map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?}else{q.query_map([],map).map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?};
 Ok(Json(rows))
}
async fn create_release(State(state):State<AppState>,Path(website_id):Path<String>)->Result<(StatusCode,Json<Release>),DeployError>{
 let (root,output_dir,runtime,website_name):(PathBuf,String,String,String)={
  let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  let (root,runtime,name):(String,String,String)=db.query_row("SELECT project_path,runtime,name FROM websites WHERE id=?1",params![website_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){DeployError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into())}else{db_error(e)})?;
  let output:String=db.query_row("SELECT output_dir FROM build_profiles WHERE website_id=?1",params![website_id],|r|r.get(0)).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){DeployError(StatusCode::NOT_FOUND,"build_profile_missing","Save a build profile before creating a release.".into())}else{db_error(e)})?;
  (PathBuf::from(root),output,runtime,name)
 };
 if runtime!="static"{return Err(DeployError(StatusCode::BAD_REQUEST,"release_runtime_unsupported","Release activation currently supports static websites only. Node.js, PHP and Python processes continue to use their project folders.".into()));}
 if !safe_output_dir(&output_dir){return Err(DeployError(StatusCode::BAD_REQUEST,"invalid_output_dir","Configured build output path is invalid.".into()));}
 let root=root.canonicalize().map_err(|e|DeployError(StatusCode::BAD_REQUEST,"website_folder_unavailable",e.to_string()))?;
 let source=root.join(&output_dir).canonicalize().map_err(|e|DeployError(StatusCode::NOT_FOUND,"build_output_missing",format!("Build output folder is unavailable: {e}")))?;
 if !source.starts_with(&root)||!source.is_dir(){return Err(DeployError(StatusCode::FORBIDDEN,"build_output_forbidden","Build output must resolve to a directory inside the website project.".into()));}
 let id=Uuid::new_v4().to_string();
 let destination=state.releases_dir.join(&id);
 fs::create_dir_all(&destination).map_err(|e|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"release_create_failed",e.to_string()))?;
 let mut counts=(0,0);
 if let Err(error)=copy_release_tree(&source,&destination,&mut counts){let _=fs::remove_dir_all(&destination);return Err(error);}
 if counts.0==0{let _=fs::remove_dir_all(&destination);return Err(DeployError(StatusCode::BAD_REQUEST,"empty_release","Build output folder is empty.".into()));}
 let clean_git=Command::new("git").arg("-C").arg(&root).args(["status","--short"]).output().ok().filter(|o|o.status.success()).map(|o|o.stdout.is_empty()).unwrap_or(false);
 let commit=if clean_git{Command::new("git").arg("-C").arg(&root).args(["rev-parse","HEAD"]).output().ok().filter(|o|o.status.success()).map(|o|String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s|!s.is_empty())}else{None};
 let release_path=destination.to_string_lossy().to_string();
 let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let tx=db.unchecked_transaction().map_err(db_error)?;
 tx.execute("UPDATE releases SET active=0 WHERE website_id=?1",params![website_id]).map_err(db_error)?;
 tx.execute("INSERT INTO releases(id,website_id,release_path,source,commit_hash,active,file_count,size_bytes) VALUES(?1,?2,?3,'build',?4,1,?5,?6)",params![id,website_id,release_path,commit,counts.0 as i64,counts.1 as i64]).map_err(db_error)?;
 let created_at:String=tx.query_row("SELECT created_at FROM releases WHERE id=?1",params![id],|r|r.get(0)).map_err(db_error)?;
 tx.commit().map_err(db_error)?;
 Ok((StatusCode::CREATED,Json(Release{id,website_id,website_name,source:"build".into(),commit_hash:commit,active:true,file_count:counts.0,size_bytes:counts.1,created_at})))
}
async fn rollback_release(State(state):State<AppState>,Path((website_id,release_id)):Path<(String,String)>)->Result<Json<serde_json::Value>,DeployError>{
 let path:String={
  let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  db.query_row("SELECT release_path FROM releases WHERE id=?1 AND website_id=?2",params![release_id,website_id],|r|r.get(0)).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){DeployError(StatusCode::NOT_FOUND,"release_not_found","Release was not found for this website.".into())}else{db_error(e)})?
 };
 let root=state.releases_dir.canonicalize().map_err(|e|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"release_store_unavailable",e.to_string()))?;
 let target=FsPath::new(&path).canonicalize().map_err(|e|DeployError(StatusCode::NOT_FOUND,"release_files_missing",e.to_string()))?;
 if !target.starts_with(&root)||!target.is_dir(){return Err(DeployError(StatusCode::FORBIDDEN,"release_path_forbidden","Release path is outside the managed release store.".into()));}
 let db=state.db.lock().map_err(|_|DeployError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let tx=db.unchecked_transaction().map_err(db_error)?;
 tx.execute("UPDATE releases SET active=0 WHERE website_id=?1",params![website_id]).map_err(db_error)?;
 tx.execute("UPDATE releases SET active=1 WHERE id=?1 AND website_id=?2",params![release_id,website_id]).map_err(db_error)?;
 tx.commit().map_err(db_error)?;
 Ok(Json(json!({"website_id":website_id,"active_release_id":release_id,"message":"Previous release activated. Static requests now serve this snapshot."})))
}
