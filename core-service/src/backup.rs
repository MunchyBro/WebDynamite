use axum::{extract::{Path,State},routing::get,Json,Router,http::StatusCode,response::IntoResponse};
use rusqlite::{params,Connection,types::ValueRef};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{fs,path::{Path as FsPath,PathBuf},time::SystemTime};
use uuid::Uuid;
use sysinfo::Disks;
use crate::AppState;

#[derive(Debug)]
struct BackupError(StatusCode,&'static str,String);
fn backup_error(status:StatusCode,code:&'static str,message:impl Into<String>)->BackupError{BackupError(status,code,message.into())}
impl IntoResponse for BackupError{
 fn into_response(self)->axum::response::Response{(self.0,Json(json!({"error":{"code":self.1,"message":self.2}}))).into_response()}
}
fn db_error(e:rusqlite::Error)->BackupError{backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_error",e.to_string())}
fn io_error(code:&'static str,e:std::io::Error)->BackupError{backup_error(StatusCode::INTERNAL_SERVER_ERROR,code,e.to_string())}
fn valid_backup_entry_id(value:&str)->bool {
 !value.is_empty()&&value.len()<=64&&value.bytes().all(|b|b.is_ascii_hexdigit()||b==b'-')
}
fn legacy_checksum_algorithm()->String{"fnv1a-64".into()}
#[derive(Serialize,Deserialize)]
struct Manifest{version:u32,#[serde(default="legacy_checksum_algorithm")]checksum_algorithm:String,checksum:String,file_count:u64,size_bytes:u64,website_count:u64,database_count:u64,created_at:String}
#[derive(Serialize)]
struct BackupRecord{id:String,checksum:String,file_count:u64,size_bytes:u64,website_count:u64,database_count:u64,verified:bool,created_at:String}
#[derive(Deserialize)]
struct RestoreConfirm{confirm:bool}

fn rows_json(db:&Connection,sql:&str,columns:usize)->Result<Vec<Value>,BackupError>{
 let mut statement=db.prepare(sql).map_err(db_error)?;
 let rows=statement.query_map([],|row|{
  let mut values=Vec::with_capacity(columns);
  for index in 0..columns{values.push(match row.get_ref(index)?{ValueRef::Null=>Value::Null,ValueRef::Integer(v)=>json!(v),ValueRef::Real(v)=>json!(v),ValueRef::Text(v)=>json!(String::from_utf8_lossy(v).to_string()),ValueRef::Blob(v)=>json!(v)});}
  Ok(Value::Array(values))
 }).map_err(db_error)?;
 rows.collect::<Result<Vec<_>,_>>().map_err(db_error)
}
fn snapshot_metadata(db:&Connection)->Result<Value,BackupError>{
 Ok(json!({
  "websites":rows_json(db,"SELECT id,name,project_path,runtime,start_command,port,lan_enabled,lan_port,created_at,updated_at FROM websites ORDER BY id",10)?,
  "domains":rows_json(db,"SELECT id,website_id,hostname,access_mode,created_at FROM domains ORDER BY id",5)?,
  "public_domains":rows_json(db,"SELECT id,website_id,hostname,created_at FROM public_domains ORDER BY id",4)?,
  "managed_databases":rows_json(db,"SELECT id,name,engine,created_at FROM managed_databases ORDER BY id",4)?,
  "runtime_profiles":rows_json(db,"SELECT id,website_id,program,args_json,updated_at FROM runtime_profiles ORDER BY id",5)?,
  "build_profiles":rows_json(db,"SELECT id,website_id,program,args_json,output_dir,updated_at FROM build_profiles ORDER BY id",6)?,
  "build_runs":rows_json(db,"SELECT id,website_id,program,status,exit_code,duration_ms,logs,created_at FROM build_runs ORDER BY id",8)?
 }))
}
fn copy_tree(source:&FsPath,destination:&FsPath,counts:&mut(u64,u64))->Result<(),BackupError>{
 for entry in fs::read_dir(source).map_err(|e|io_error("backup_read_failed",e))?{
  let entry=entry.map_err(|e|io_error("backup_read_failed",e))?;
  let kind=entry.file_type().map_err(|e|io_error("backup_metadata_failed",e))?;
  if kind.is_symlink(){return Err(backup_error(StatusCode::FORBIDDEN,"backup_symlink_forbidden",format!("Backup cancelled: symbolic link found at {}",entry.path().display())));}
  let target=destination.join(entry.file_name());
  if kind.is_dir(){fs::create_dir_all(&target).map_err(|e|io_error("backup_create_failed",e))?;copy_tree(&entry.path(),&target,counts)?;}
  else if kind.is_file(){
   let size=entry.metadata().map_err(|e|io_error("backup_metadata_failed",e))?.len();
   counts.0+=1;counts.1=counts.1.saturating_add(size);
   if counts.0>100000||counts.1>10*1024*1024*1024{return Err(backup_error(StatusCode::PAYLOAD_TOO_LARGE,"backup_too_large","Backup limit is 100,000 files and 10 GiB."));}
   fs::copy(entry.path(),&target).map_err(|e|io_error("backup_copy_failed",e))?;
  }else{return Err(backup_error(StatusCode::FORBIDDEN,"backup_special_file_forbidden","Backup cancelled: special files are not supported."));}
 }
 Ok(())
}
fn fnv_hash(bytes:&[u8],mut hash:u64)->u64{for byte in bytes{hash^=*byte as u64;hash=hash.wrapping_mul(1099511628211);}hash}
fn tree_checksum(root:&FsPath)->Result<(String,u64,u64),BackupError>{
 fn walk(root:&FsPath,current:&FsPath,hash:&mut u64,files:&mut u64,size:&mut u64)->Result<(),BackupError>{
  for entry in fs::read_dir(current).map_err(|e|io_error("backup_read_failed",e))?{
   let entry=entry.map_err(|e|io_error("backup_read_failed",e))?;
   let kind=entry.file_type().map_err(|e|io_error("backup_metadata_failed",e))?;
   if kind.is_symlink(){return Err(backup_error(StatusCode::FORBIDDEN,"backup_symlink_forbidden","Backup contains a symbolic link."));}
   if kind.is_dir(){walk(root,&entry.path(),hash,files,size)?;}
   else if kind.is_file(){
    let file_path=entry.path();
    let relative=file_path.strip_prefix(root).unwrap_or(FsPath::new("")).to_string_lossy();
    if relative=="manifest.json"{continue;}
    *hash=fnv_hash(relative.as_bytes(),*hash);
    let bytes=fs::read(&file_path).map_err(|e|io_error("backup_read_failed",e))?;
    *files+=1;*size=size.saturating_add(bytes.len() as u64);*hash=fnv_hash(&bytes,*hash);
   }
  }
  Ok(())
 }
 let mut hash=14695981039346656037u64;let mut files=0;let mut size=0;
 walk(root,root,&mut hash,&mut files,&mut size)?;
 Ok((format!("{hash:016x}"),files,size))
}
fn tree_checksum_sha256(root:&FsPath)->Result<(String,u64,u64),BackupError>{
 use sha2::{Digest,Sha256};
 fn collect(root:&FsPath,current:&FsPath,files:&mut Vec<PathBuf>)->Result<(),BackupError>{
  for entry in fs::read_dir(current).map_err(|e|io_error("backup_read_failed",e))?{
   let entry=entry.map_err(|e|io_error("backup_read_failed",e))?;
   let kind=entry.file_type().map_err(|e|io_error("backup_metadata_failed",e))?;
   if kind.is_symlink(){return Err(backup_error(StatusCode::FORBIDDEN,"backup_symlink_forbidden","Backup contains a symbolic link."));}
   if kind.is_dir(){collect(root,&entry.path(),files)?;}
   else if kind.is_file(){
    let path=entry.path();
    let relative=path.strip_prefix(root).unwrap_or(&path);
    if relative!=FsPath::new("manifest.json"){files.push(path);}
   }
  }
  Ok(())
 }
 let mut paths=Vec::new();collect(root,root,&mut paths)?;
 paths.sort_by(|a,b|a.strip_prefix(root).unwrap_or(a).cmp(b.strip_prefix(root).unwrap_or(b)));
 let mut hasher=Sha256::new();let mut size=0u64;
 for path in &paths{
  let relative=path.strip_prefix(root).unwrap_or(path).to_string_lossy();
  let bytes=fs::read(path).map_err(|e|io_error("backup_read_failed",e))?;
  hasher.update((relative.len() as u64).to_le_bytes());hasher.update(relative.as_bytes());
  hasher.update((bytes.len() as u64).to_le_bytes());hasher.update(&bytes);
  size=size.saturating_add(bytes.len() as u64);
 }
 Ok((format!("{:x}",hasher.finalize()),paths.len() as u64,size))
}
fn vacuum_database(source:&str,destination:&FsPath)->Result<(),BackupError>{
 let connection=Connection::open(source).map_err(db_error)?;
 let destination_text=destination.to_string_lossy().replace("'","''");
 connection.execute_batch(&format!("VACUUM INTO '{destination_text}'")).map_err(db_error)?;
 let integrity:String=Connection::open(destination).map_err(db_error)?.query_row("PRAGMA integrity_check",[],|r|r.get(0)).map_err(db_error)?;
 if integrity!="ok"{return Err(backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_database_invalid",format!("SQLite backup failed integrity check: {integrity}")));}
 Ok(())
}
fn create_backup_sync(state:&AppState)->Result<BackupRecord,BackupError>{
 let active=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?.query_row("SELECT COUNT(*) FROM websites WHERE status!='stopped'",[],|r|r.get::<_,i64>(0)).map_err(db_error)?;
 if active>0||!state.processes.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"process_registry_locked","Website process registry is unavailable."))?.is_empty(){return Err(backup_error(StatusCode::CONFLICT,"websites_must_be_stopped","Stop every website before creating a consistent full-system backup."));}
 let (metadata,websites,databases):(Value,Vec<(String,PathBuf)>,Vec<(String,String,String,String)>)={
  let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
  let metadata=snapshot_metadata(&db)?;
  let websites=metadata["websites"].as_array().unwrap_or(&Vec::new()).iter().map(|v|{
   (v[0].as_str().unwrap_or("").to_string(),PathBuf::from(v[2].as_str().unwrap_or("")))
  }).collect();
  let databases=metadata["managed_databases"].as_array().unwrap_or(&Vec::new()).iter().map(|v|{
   let id=v[0].as_str().unwrap_or("").to_string();
   let name=v[1].as_str().unwrap_or("").to_string();
   let engine=v[2].as_str().unwrap_or("sqlite").to_string();
   let path=state.config_dir.join("databases").join(format!("{id}.sqlite3")).to_string_lossy().to_string();
   (id,name,engine,path)
  }).collect();
  (metadata,websites,databases)
 };
 let id=Uuid::new_v4().to_string();let destination=state.backups_dir.join(&id);
 fs::create_dir_all(destination.join("websites")).map_err(|e|io_error("backup_create_failed",e))?;
 fs::create_dir_all(destination.join("databases")).map_err(|e|io_error("backup_create_failed",e))?;
 fs::write(destination.join("metadata.json"),serde_json::to_vec_pretty(&metadata).map_err(|e|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"backup_metadata_failed",e.to_string()))?).map_err(|e|io_error("backup_write_failed",e))?;
 let mut counts=(0u64,0u64);
 for (website_id,project) in &websites{
  let root=project.canonicalize().map_err(|e|backup_error(StatusCode::NOT_FOUND,"website_folder_unavailable",format!("A registered website folder is unavailable: {e}")))?;
  if !root.is_dir(){return Err(backup_error(StatusCode::BAD_REQUEST,"website_folder_invalid","A registered website path is not a directory."));}
  let target=destination.join("websites").join(website_id);fs::create_dir_all(&target).map_err(|e|io_error("backup_create_failed",e))?;
  copy_tree(&root,&target,&mut counts)?;
 }
 for (db_id,_,_,source) in &databases{
  let target=destination.join("databases").join(format!("{db_id}.sqlite3"));
  if !FsPath::new(source).is_file(){return Err(backup_error(StatusCode::NOT_FOUND,"managed_database_missing",format!("Managed database file is missing: {db_id}")));}
  vacuum_database(source,&target)?;
  let size=fs::metadata(&target).map_err(|e|io_error("backup_metadata_failed",e))?.len();
  counts.0+=1;counts.1=counts.1.saturating_add(size);
 }
 let metadata_size=fs::metadata(destination.join("metadata.json")).map_err(|e|io_error("backup_metadata_failed",e))?.len();
 counts.0+=1;counts.1=counts.1.saturating_add(metadata_size);
 if counts.0>100000||counts.1>10*1024*1024*1024{let _=fs::remove_dir_all(&destination);return Err(backup_error(StatusCode::PAYLOAD_TOO_LARGE,"backup_too_large","Backup limit is 100,000 files and 10 GiB."));}
 let (checksum,file_count,size_bytes)=tree_checksum_sha256(&destination)?;
 let created_at=chrono_free_timestamp();
 let manifest=Manifest{version:2,checksum_algorithm:"sha256".into(),checksum:checksum.clone(),file_count,size_bytes,website_count:websites.len() as u64,database_count:databases.len() as u64,created_at:created_at.clone()};
 fs::write(destination.join("manifest.json"),serde_json::to_vec_pretty(&manifest).map_err(|e|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"backup_manifest_failed",e.to_string()))?).map_err(|e|io_error("backup_write_failed",e))?;
 let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
 db.execute("INSERT INTO backups(id,backup_path,checksum,file_count,size_bytes,website_count,database_count,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,destination.to_string_lossy(),checksum,file_count as i64,size_bytes as i64,websites.len() as i64,databases.len() as i64,created_at]).map_err(db_error)?;
 Ok(BackupRecord{id,checksum,file_count,size_bytes,website_count:websites.len() as u64,database_count:databases.len() as u64,verified:true,created_at})
}
fn chrono_free_timestamp()->String{
 let seconds=SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
 format!("unix:{seconds}")
}
fn backup_path(state:&AppState,id:&str)->Result<PathBuf,BackupError>{
 if id.is_empty()||!id.bytes().all(|b|b.is_ascii_hexdigit()||b==b'-'){return Err(backup_error(StatusCode::BAD_REQUEST,"invalid_backup_id","Invalid backup identifier."));}
 let path={let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;db.query_row("SELECT backup_path FROM backups WHERE id=?1",[id],|r|r.get::<_,String>(0)).map_err(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){backup_error(StatusCode::NOT_FOUND,"backup_not_found","Backup was not found.")}else{db_error(e)})?};
 let root=state.backups_dir.canonicalize().map_err(|e|io_error("backup_store_unavailable",e))?;
 let target=PathBuf::from(path).canonicalize().map_err(|e|backup_error(StatusCode::NOT_FOUND,"backup_files_missing",e.to_string()))?;
 if !target.starts_with(&root)||!target.is_dir(){return Err(backup_error(StatusCode::FORBIDDEN,"backup_path_forbidden","Backup path is outside the managed backup directory."));}
 Ok(target)
}
fn load_manifest(path:&FsPath)->Result<Manifest,BackupError>{
 let bytes=fs::read(path.join("manifest.json")).map_err(|e|io_error("backup_manifest_missing",e))?;
 serde_json::from_slice(&bytes).map_err(|e|backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_manifest_invalid",e.to_string()))
}
fn verify_sync(path:&FsPath)->Result<Manifest,BackupError>{
 let manifest=load_manifest(path)?;
 if manifest.version>2{return Err(backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_version_unsupported","This backup was created by a newer WebDynamite version."));}
 let (checksum,count,size)=match (manifest.version,manifest.checksum_algorithm.as_str()){(1,"fnv1a-64")=>tree_checksum(path)?,(2,"sha256")=>tree_checksum_sha256(path)?,_=>return Err(backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_checksum_unsupported","Backup checksum algorithm is not supported."))};
 if checksum!=manifest.checksum||count!=manifest.file_count||size!=manifest.size_bytes{return Err(backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_verification_failed","Backup checksum or file inventory does not match. Do not restore this backup."));}
 Ok(manifest)
}
fn list_sync(state:&AppState)->Result<Vec<BackupRecord>,BackupError>{
 let rows={let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;let mut q=db.prepare("SELECT id,backup_path,checksum,file_count,size_bytes,website_count,database_count,created_at FROM backups ORDER BY created_at DESC").map_err(db_error)?;let mapped=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,u64>(3)?,r.get::<_,u64>(4)?,r.get::<_,u64>(5)?,r.get::<_,u64>(6)?,r.get::<_,String>(7)?))).map_err(db_error)?;let collected=mapped.collect::<Result<Vec<_>,_>>().map_err(db_error)?;collected};
 Ok(rows.into_iter().map(|(id,path,checksum,file_count,size_bytes,website_count,database_count,created_at)|{
  let verified=verify_sync(&PathBuf::from(path)).is_ok();
  BackupRecord{id,checksum,file_count,size_bytes,website_count,database_count,verified,created_at}
 }).collect())
}
fn preview_sync(state:&AppState,id:&str)->Result<Value,BackupError>{
 let path=backup_path(state,id)?;let manifest=verify_sync(&path)?;
 let metadata:Value=serde_json::from_slice(&fs::read(path.join("metadata.json")).map_err(|e|io_error("backup_metadata_missing",e))?).map_err(|e|backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_metadata_invalid",e.to_string()))?;
 let sites=metadata["websites"].as_array().cloned().unwrap_or_default().into_iter().map(|v|json!({"id":v[0],"name":v[1],"project_path":v[2]})).collect::<Vec<_>>();
 let databases=metadata["managed_databases"].as_array().cloned().unwrap_or_default().into_iter().map(|v|json!({"id":v[0],"name":v[1],"engine":v[2]})).collect::<Vec<_>>();
 Ok(json!({"backup_id":id,"verified":true,"checksum":manifest.checksum,"file_count":manifest.file_count,"size_bytes":manifest.size_bytes,"website_count":manifest.website_count,"database_count":manifest.database_count,"websites":sites,"databases":databases,"restore_scope":"All registered websites, managed SQLite databases, local/public domain mappings and build metadata will be restored. Website folders and managed database contents will be overwritten."}))
}
fn replace_tree(source:&FsPath,target:&FsPath)->Result<(),BackupError>{
 let parent=target.parent().ok_or_else(||backup_error(StatusCode::BAD_REQUEST,"invalid_project_path","Website folder has no parent directory."))?;
 let name=target.file_name().ok_or_else(||backup_error(StatusCode::BAD_REQUEST,"invalid_project_path","Website folder path is invalid."))?.to_string_lossy();
 let staging=parent.join(format!(".{name}.webdynamite-restore"));
 let previous=parent.join(format!(".{name}.webdynamite-previous"));
 if staging.exists(){fs::remove_dir_all(&staging).map_err(|e|io_error("restore_stage_failed",e))?;}
 if previous.exists(){fs::remove_dir_all(&previous).map_err(|e|io_error("restore_cleanup_failed",e))?;}
 fs::create_dir_all(&staging).map_err(|e|io_error("restore_stage_failed",e))?;
 let mut counts=(0,0);copy_tree(source,&staging,&mut counts)?;
 if target.exists(){fs::rename(target,&previous).map_err(|e|io_error("restore_stage_failed",e))?;}
 if let Err(error)=fs::rename(&staging,target){if previous.exists(){let _=fs::rename(&previous,target);}return Err(io_error("restore_activate_failed",error));}
 if previous.exists(){fs::remove_dir_all(previous).map_err(|e|io_error("restore_cleanup_failed",e))?;}
 Ok(())
}
fn restore_sync(state:&AppState,id:&str)->Result<Value,BackupError>{
 let path=backup_path(state,id)?;let manifest=verify_sync(&path)?;
 let metadata:Value=serde_json::from_slice(&fs::read(path.join("metadata.json")).map_err(|e|io_error("backup_metadata_missing",e))?).map_err(|e|backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_metadata_invalid",e.to_string()))?;
 {
  let active:i64={let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;db.query_row("SELECT COUNT(*) FROM websites WHERE status!='stopped'",[],|r|r.get(0)).map_err(db_error)?};
  let has_processes=!state.processes.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"process_registry_locked","Website process registry is unavailable."))?.is_empty();
  if active>0||has_processes{return Err(backup_error(StatusCode::CONFLICT,"websites_must_be_stopped","Stop every website before restoring a full-system backup."));}
 }
 let sites=metadata["websites"].as_array().cloned().ok_or_else(||backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_metadata_invalid","Backup metadata is missing the website inventory."))?;
 let databases=metadata["managed_databases"].as_array().cloned().ok_or_else(||backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_metadata_invalid","Backup metadata is missing the database inventory."))?;
 if sites.len() as u64!=manifest.website_count||databases.len() as u64!=manifest.database_count{
  return Err(backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_inventory_mismatch","Backup metadata inventory does not match its manifest."));
 }
 // Validate every identifier and destination before creating the safety backup or
 // changing any website/database files. A valid checksum alone does not make paths safe.
 let websites_root=path.join("websites").canonicalize().map_err(|e|io_error("backup_website_store_missing",e))?;
 let mut restore_sites=Vec::with_capacity(sites.len());
 for website in &sites{
  let website_id=website.get(0).and_then(Value::as_str).filter(|value|valid_backup_entry_id(value))
   .ok_or_else(||backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_website_id_invalid","Backup contains an invalid website identifier."))?;
  let project=website.get(2).and_then(Value::as_str).filter(|value|!value.trim().is_empty())
   .ok_or_else(||backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_project_path_invalid","Backup contains an invalid website destination."))?;
  let root=PathBuf::from(project).canonicalize().map_err(|e|backup_error(StatusCode::NOT_FOUND,"restore_project_missing",format!("Restore destination must already exist: {e}")))?;
  if !root.is_dir(){return Err(backup_error(StatusCode::BAD_REQUEST,"restore_project_invalid","A website restore destination is not a directory."));}
  let source=path.join("websites").join(website_id).canonicalize()
   .map_err(|_|backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_website_missing","Backup is missing a website folder."))?;
  if !source.starts_with(&websites_root)||!source.is_dir(){return Err(backup_error(StatusCode::FORBIDDEN,"backup_website_path_forbidden","A website backup folder resolves outside the managed backup."));}
  restore_sites.push((source,root));
 }
 let database_root=path.join("databases").canonicalize().map_err(|e|io_error("backup_database_store_missing",e))?;
 let db_dir=state.config_dir.join("databases");fs::create_dir_all(&db_dir).map_err(|e|io_error("restore_database_dir_failed",e))?;
 let mut restore_databases=Vec::with_capacity(databases.len());
 for database in &databases{
  let database_id=database.get(0).and_then(Value::as_str).filter(|value|valid_backup_entry_id(value))
   .ok_or_else(||backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_database_id_invalid","Backup contains an invalid database identifier."))?;
  if database.get(2).and_then(Value::as_str)!=Some("sqlite"){
   return Err(backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_database_engine_invalid","Backup contains an unsupported managed database engine."));
  }
  let source=path.join("databases").join(format!("{database_id}.sqlite3")).canonicalize()
   .map_err(|_|backup_error(StatusCode::UNPROCESSABLE_ENTITY,"backup_database_missing","Backup is missing a managed database file."))?;
  if !source.starts_with(&database_root)||!source.is_file(){return Err(backup_error(StatusCode::FORBIDDEN,"backup_database_path_forbidden","A database backup file resolves outside the managed backup."));}
  restore_databases.push((database_id.to_string(),source,db_dir.join(format!("{database_id}.sqlite3"))));
 }
 let safety=create_backup_sync(state)?;
 for (source,root) in restore_sites{replace_tree(&source,&root)?;}
 for (id,source,target) in restore_databases{
  let temporary=db_dir.join(format!("{id}.restore.tmp"));
  fs::copy(&source,&temporary).map_err(|e|io_error("restore_database_copy_failed",e))?;
  let _=fs::remove_file(db_dir.join(format!("{id}.sqlite3-wal")));let _=fs::remove_file(db_dir.join(format!("{id}.sqlite3-shm")));
  if target.exists(){fs::remove_file(&target).map_err(|e|io_error("restore_database_replace_failed",e))?;}
  fs::rename(&temporary,&target).map_err(|e|io_error("restore_database_replace_failed",e))?;
 }
 let mut db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
 let tx=db.transaction().map_err(db_error)?;
 tx.execute("DELETE FROM websites",[]).map_err(db_error)?;
 tx.execute("DELETE FROM managed_databases",[]).map_err(db_error)?;
 for row in sites{
  tx.execute("INSERT INTO websites(id,name,project_path,runtime,status,start_command,port,lan_enabled,lan_port,created_at,updated_at) VALUES(?1,?2,?3,?4,'stopped',?5,?6,?7,?8,?9,?10)",params![row[0].as_str(),row[1].as_str(),row[2].as_str(),row[3].as_str(),row[4].as_str(),row[5].as_i64(),row[6].as_i64().unwrap_or(0),row[7].as_i64(),row[8].as_str(),row[9].as_str()]).map_err(db_error)?;
 }
 for row in metadata["domains"].as_array().cloned().unwrap_or_default(){tx.execute("INSERT INTO domains(id,website_id,hostname,access_mode,created_at) VALUES(?1,?2,?3,?4,?5)",params![row[0].as_str(),row[1].as_str(),row[2].as_str(),row[3].as_str(),row[4].as_str()]).map_err(db_error)?;}
 for row in metadata["public_domains"].as_array().cloned().unwrap_or_default(){tx.execute("INSERT INTO public_domains(id,website_id,hostname,created_at) VALUES(?1,?2,?3,?4)",params![row[0].as_str(),row[1].as_str(),row[2].as_str(),row[3].as_str()]).map_err(db_error)?;}
 for row in databases{let id=row[0].as_str().unwrap_or("");let name=row[1].as_str().unwrap_or("");let engine=row[2].as_str().unwrap_or("sqlite");let file=db_dir.join(format!("{id}.sqlite3")).to_string_lossy().to_string();tx.execute("INSERT INTO managed_databases(id,name,engine,file_path,created_at) VALUES(?1,?2,?3,?4,?5)",params![id,name,engine,file,row[3].as_str()]).map_err(db_error)?;}
 for row in metadata["runtime_profiles"].as_array().cloned().unwrap_or_default(){tx.execute("INSERT INTO runtime_profiles(id,website_id,program,args_json,updated_at) VALUES(?1,?2,?3,?4,?5)",params![row[0].as_str(),row[1].as_str(),row[2].as_str(),row[3].as_str(),row[4].as_str()]).map_err(db_error)?;}
 for row in metadata["build_profiles"].as_array().cloned().unwrap_or_default(){tx.execute("INSERT INTO build_profiles(id,website_id,program,args_json,output_dir,updated_at) VALUES(?1,?2,?3,?4,?5,?6)",params![row[0].as_str(),row[1].as_str(),row[2].as_str(),row[3].as_str(),row[4].as_str(),row[5].as_str()]).map_err(db_error)?;}
 for row in metadata["build_runs"].as_array().cloned().unwrap_or_default(){tx.execute("INSERT INTO build_runs(id,website_id,program,status,exit_code,duration_ms,logs,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![row[0].as_str(),row[1].as_str(),row[2].as_str(),row[3].as_str(),row[4].as_i64(),row[5].as_i64(),row[6].as_str(),row[7].as_str()]).map_err(db_error)?;}
 tx.commit().map_err(db_error)?;
 Ok(json!({"restored":true,"backup_id":id,"safety_backup_id":safety.id,"website_count":manifest.website_count,"database_count":manifest.database_count,"message":"Backup restored. A pre-restore safety backup was created automatically."}))
}
async fn create_backup(State(state):State<AppState>)->Result<(StatusCode,Json<BackupRecord>),BackupError>{
 let result=tokio::task::spawn_blocking(move||create_backup_sync(&state)).await.map_err(|e|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"backup_task_failed",e.to_string()))??;
 Ok((StatusCode::CREATED,Json(result)))
}
async fn list_backups(State(state):State<AppState>)->Result<Json<Vec<BackupRecord>>,BackupError>{
 let result=tokio::task::spawn_blocking(move||list_sync(&state)).await.map_err(|e|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"backup_task_failed",e.to_string()))??;Ok(Json(result))
}
async fn verify_backup(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<Value>,BackupError>{
 let result=tokio::task::spawn_blocking(move||{let path=backup_path(&state,&id)?;let m=verify_sync(&path)?;Ok(json!({"verified":true,"checksum":m.checksum,"file_count":m.file_count,"size_bytes":m.size_bytes}))}).await.map_err(|e|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"backup_task_failed",e.to_string()))??;Ok(Json(result))
}
async fn preview_restore(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<Value>,BackupError>{
 let result=tokio::task::spawn_blocking(move||preview_sync(&state,&id)).await.map_err(|e|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"backup_task_failed",e.to_string()))??;Ok(Json(result))
}
async fn restore_backup(State(state):State<AppState>,Path(id):Path<String>,Json(input):Json<RestoreConfirm>)->Result<Json<Value>,BackupError>{
 if !input.confirm{return Err(backup_error(StatusCode::BAD_REQUEST,"restore_confirmation_required","Set confirm=true after reviewing the restore preview."));}
 let result=tokio::task::spawn_blocking(move||restore_sync(&state,&id)).await.map_err(|e|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"restore_task_failed",e.to_string()))??;Ok(Json(result))
}
async fn delete_backup(State(state):State<AppState>,Path(id):Path<String>)->Result<StatusCode,BackupError>{
 let path=backup_path(&state,&id)?;
 fs::remove_dir_all(path).map_err(|e|io_error("backup_delete_failed",e))?;
 let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
 db.execute("DELETE FROM backups WHERE id=?1",[id]).map_err(db_error)?;
 Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug,Serialize)]
struct BackupSchedule{enabled:bool,interval_hours:i64,retention_count:i64,next_run_at:Option<i64>,last_run_at:Option<i64>,last_status:Option<String>,last_error:Option<String>}
#[derive(Deserialize)]
struct SaveBackupSchedule{enabled:bool,interval_hours:i64,retention_count:i64}
fn unix_now()->i64{SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs() as i64}
fn schedule_sync(state:&AppState)->Result<BackupSchedule,BackupError>{
 let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
 db.query_row("SELECT enabled,interval_hours,retention_count,next_run_at,last_run_at,last_status,last_error FROM backup_schedule WHERE id=1",[],|r|Ok(BackupSchedule{enabled:r.get::<_,i64>(0)?!=0,interval_hours:r.get(1)?,retention_count:r.get(2)?,next_run_at:r.get(3)?,last_run_at:r.get(4)?,last_status:r.get(5)?,last_error:r.get(6)?})).map_err(db_error)
}

#[derive(Serialize)]
struct BackupStorageSummary{backup_count:u64,verified_count:u64,unverified_count:u64,total_backup_bytes:u64,volume_total_bytes:u64,volume_available_bytes:u64,volume_mount_point:String}
fn storage_summary_sync(state:&AppState)->Result<BackupStorageSummary,BackupError>{
 let rows={let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
  let mut q=db.prepare("SELECT backup_path FROM backups").map_err(db_error)?;
  let mapped=q.query_map([],|r|r.get::<_,String>(0)).map_err(db_error)?;
  mapped.collect::<Result<Vec<_>,_>>().map_err(db_error)?};
 let mut verified=0u64;let mut total_bytes=0u64;
 for path in &rows{
  let candidate=PathBuf::from(path);
  if verify_sync(&candidate).is_ok(){verified+=1;}
  if let Ok(size)=directory_size(&candidate){total_bytes=total_bytes.saturating_add(size);}
 }
 let disks=Disks::new_with_refreshed_list();
 let selected=disks.iter().filter(|disk|state.backups_dir.starts_with(disk.mount_point()))
  .max_by_key(|disk|disk.mount_point().as_os_str().len());
 let (volume_total_bytes,volume_available_bytes,volume_mount_point)=selected.map(|disk|(
  disk.total_space(),disk.available_space(),disk.mount_point().to_string_lossy().to_string()
 )).unwrap_or((0,0,String::new()));
 Ok(BackupStorageSummary{backup_count:rows.len() as u64,verified_count:verified,unverified_count:rows.len() as u64-verified,total_backup_bytes:total_bytes,volume_total_bytes,volume_available_bytes,volume_mount_point})
}
fn directory_size(root:&FsPath)->Result<u64,BackupError>{
 let metadata=fs::symlink_metadata(root).map_err(|e|io_error("backup_metadata_failed",e))?;
 if metadata.file_type().is_symlink(){return Err(backup_error(StatusCode::FORBIDDEN,"backup_symlink_forbidden","Backup storage contains a symbolic link."));}
 if metadata.is_file(){return Ok(metadata.len());}
 if !metadata.is_dir(){return Ok(0);}
 let mut total=0u64;
 for entry in fs::read_dir(root).map_err(|e|io_error("backup_read_failed",e))?{
  let entry=entry.map_err(|e|io_error("backup_read_failed",e))?;
  let path=entry.path();let kind=entry.file_type().map_err(|e|io_error("backup_metadata_failed",e))?;
  if kind.is_symlink(){continue;}
  if kind.is_dir(){total=total.saturating_add(directory_size(&path)?);}
  else if kind.is_file(){total=total.saturating_add(entry.metadata().map_err(|e|io_error("backup_metadata_failed",e))?.len());}
 }
 Ok(total)
}
async fn get_storage_summary(State(state):State<AppState>)->Result<Json<BackupStorageSummary>,BackupError>{
 let result=tokio::task::spawn_blocking(move||storage_summary_sync(&state)).await.map_err(|e|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"backup_task_failed",e.to_string()))??;
 Ok(Json(result))
}

async fn get_schedule(State(state):State<AppState>)->Result<Json<BackupSchedule>,BackupError>{Ok(Json(schedule_sync(&state)?))}
async fn save_schedule(State(state):State<AppState>,Json(input):Json<SaveBackupSchedule>)->Result<Json<BackupSchedule>,BackupError>{
 if !(1..=720).contains(&input.interval_hours)||!(1..=100).contains(&input.retention_count){return Err(backup_error(StatusCode::BAD_REQUEST,"invalid_backup_schedule","Interval must be 1–720 hours and retention must be 1–100 backups."));}
 let now=unix_now();let next=if input.enabled{Some(now+input.interval_hours*3600)}else{None};
 {let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
 db.execute("UPDATE backup_schedule SET enabled=?1,interval_hours=?2,retention_count=?3,next_run_at=?4,last_error=NULL WHERE id=1",params![input.enabled as i64,input.interval_hours,input.retention_count,next]).map_err(db_error)?;}
 Ok(Json(schedule_sync(&state)?))
}
fn prune_sync(state:&AppState,retention:i64)->Result<u64,BackupError>{
 let rows={let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
 let mut q=db.prepare("SELECT id,backup_path FROM backups ORDER BY created_at DESC").map_err(db_error)?;
 let mapped=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(db_error)?;
 mapped.collect::<Result<Vec<_>,_>>().map_err(db_error)?};
 let mut kept=0i64;let mut removed=0u64;
 for (id,path) in rows{
  let candidate=PathBuf::from(&path);
  if !candidate.is_dir()||verify_sync(&candidate).is_err(){continue;}
  kept+=1;if kept<=retention.max(1){continue;}
  let root=state.backups_dir.canonicalize().map_err(|e|io_error("backup_store_unavailable",e))?;
  let target=candidate.canonicalize().map_err(|e|io_error("backup_files_missing",e))?;
  if !target.starts_with(&root)||target==root{return Err(backup_error(StatusCode::FORBIDDEN,"backup_path_forbidden","Refusing to prune a path outside the managed backup directory."));}
  fs::remove_dir_all(&target).map_err(|e|io_error("backup_prune_failed",e))?;
  let db=state.db.lock().map_err(|_|backup_error(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable."))?;
  db.execute("DELETE FROM backups WHERE id=?1",[id]).map_err(db_error)?;removed+=1;
 }
 Ok(removed)
}
pub fn start_scheduler(state:AppState){
 tokio::spawn(async move{
  let mut tick=tokio::time::interval(std::time::Duration::from_secs(15));
  loop{
   tick.tick().await;
   let due={let db=match state.db.lock(){Ok(v)=>v,Err(_)=>continue};
    db.query_row("SELECT enabled,interval_hours,retention_count,next_run_at FROM backup_schedule WHERE id=1",[],|r|Ok((r.get::<_,i64>(0)?!=0,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<i64>>(3)?))).ok()};
   let Some((true,interval,retention,Some(next)))=due else{continue;};
   let now=unix_now();if now<next{continue;}
   {let db=match state.db.lock(){Ok(v)=>v,Err(_)=>continue};
    let _=db.execute("UPDATE backup_schedule SET next_run_at=?1 WHERE id=1",params![now+interval*3600]);}
   let backup_state=state.clone();
   let result=tokio::task::spawn_blocking(move||create_backup_sync(&backup_state)).await;
   let (status,error)=match result{Ok(Ok(_))=>("success".to_string(),None),Ok(Err(e))=>("failed".to_string(),Some(e.2)),Err(e)=>("failed".to_string(),Some(e.to_string()))};
   if status=="success"{let prune_state=state.clone();let _=tokio::task::spawn_blocking(move||prune_sync(&prune_state,retention)).await;}
   if let Ok(db)=state.db.lock(){let _=db.execute("UPDATE backup_schedule SET last_run_at=?1,last_status=?2,last_error=?3 WHERE id=1",params![now,status,error]);}
  }
 });
}

pub fn router()->Router<AppState>{
 Router::new().route("/api/v1/backups",get(list_backups).post(create_backup))
   .route("/api/v1/backups/storage",get(get_storage_summary))
  .route("/api/v1/backups/schedule",get(get_schedule).put(save_schedule))
  .route("/api/v1/backups/:id/verify",axum::routing::post(verify_backup))
  .route("/api/v1/backups/:id/preview",get(preview_restore))
  .route("/api/v1/backups/:id/restore",axum::routing::post(restore_backup))
  .route("/api/v1/backups/:id",axum::routing::delete(delete_backup))
}
#[cfg(test)]
mod tests{
 use super::*;
 #[test]fn fnv_hash_is_deterministic(){assert_eq!(format!("{:016x}",fnv_hash(b"webdynamite",14695981039346656037)),"6920feb25131f99a");}
 #[test]fn sha256_tree_checksum_detects_content_changes(){
  let root=std::env::temp_dir().join(format!("webdynamite-backup-test-{}",Uuid::new_v4()));
  fs::create_dir_all(root.join("nested")).unwrap();
  fs::write(root.join("nested/site.txt"),b"first").unwrap();
  fs::write(root.join("nested/manifest.json"),b"nested manifest is part of the payload").unwrap();
  let first=tree_checksum_sha256(&root).unwrap();
  let repeated=tree_checksum_sha256(&root).unwrap();
  assert_eq!(first,repeated);
  fs::write(root.join("nested/site.txt"),b"changed").unwrap();
  fs::write(root.join("nested/manifest.json"),b"modified nested manifest").unwrap();
  let changed=tree_checksum_sha256(&root).unwrap();
  assert_ne!(first.0,changed.0);
  let _=fs::remove_dir_all(root);
 }
 #[test]fn sha256_manifest_versions_are_explicit(){
  assert_eq!(legacy_checksum_algorithm(),"fnv1a-64");
 }
 #[test]fn backup_entry_ids_reject_path_components_and_empty_values(){
  assert!(!valid_backup_entry_id("../outside"));
  assert!(!valid_backup_entry_id(""));
  assert!(!valid_backup_entry_id("site/name"));
  assert!(valid_backup_entry_id("550e8400-e29b-41d4-a716-446655440000"));
 }
}
