use axum::{extract::{Path, State}, routing::get, Json, Router, http::StatusCode};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use crate::AppState;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

fn secret_storage_key(id: &str, name: &str) -> String {
 let mut digest = Sha256::new();
 digest.update(b"WebDynamite.SecretStore.Key.v1\\0");
 digest.update(id.as_bytes());
 digest.update([0]);
 digest.update(name.as_bytes());
 format!("{:x}", digest.finalize())
}
fn read_secret_value(state: &AppState, id: &str, name: &str) -> Result<String, String> {
 if let Some(root) = &state.secret_store_root {
  let mut bytes = crate::secret_store::read_secret(root, &secret_storage_key(id, name))?;
  let value = String::from_utf8(bytes.clone()).map_err(|_| "Protected environment secret is not valid UTF-8.".to_string());
  bytes.zeroize();
  value
 } else {
  credential_entry(id, name).map_err(|error| error.2)?.get_password().map_err(|error| format!("Secret environment variable {name} is unavailable in Windows Credential Manager: {error}"))
 }
}
fn write_secret_value(state: &AppState, id: &str, name: &str, value: &str) -> Result<(), String> {
 if let Some(root) = &state.secret_store_root {
  crate::secret_store::write_secret_verified(root, &secret_storage_key(id, name), value.as_bytes())
 } else {
  credential_entry(id, name).map_err(|error| error.2)?.set_password(value).map_err(|error| error.to_string())
 }
}
fn delete_secret_value(state: &AppState, id: &str, name: &str) -> Result<(), String> {
 if let Some(root) = &state.secret_store_root {
  crate::secret_store::delete_secret(root, &secret_storage_key(id, name))
 } else {
  let entry = credential_entry(id, name).map_err(|error| error.2)?;
  match entry.delete_credential() { Ok(()) | Err(keyring::Error::NoEntry) => Ok(()), Err(error) => Err(error.to_string()) }
 }
}
#[derive(Debug)]
struct EnvironmentError(StatusCode, &'static str, String);
impl axum::response::IntoResponse for EnvironmentError {
 fn into_response(self)->axum::response::Response{(self.0,Json(json!({"error":{"code":self.1,"message":self.2}}))).into_response()}
}
fn db_error(e:rusqlite::Error)->EnvironmentError{EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_error",e.to_string())}
fn credential_entry(id:&str,name:&str)->Result<keyring::Entry,EnvironmentError>{
 keyring::Entry::new("WebDynamite",&format!("website-env:{id}:{name}")).map_err(|e|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"credential_store_unavailable",format!("Windows Credential Manager is unavailable: {e}")))
}
fn valid_name(v:&str)->bool{let b=v.as_bytes();!b.is_empty()&&b.len()<=128&&(b[0].is_ascii_uppercase()||b[0]==b'_')&&b.iter().all(|x|x.is_ascii_uppercase()||x.is_ascii_digit()||*x==b'_')&&!matches!(v,"PORT"|"HOST"|"WEBDYNAMITE_API_TOKEN")}
#[derive(Serialize)] struct EnvironmentVariable{name:String,value:Option<String>,is_secret:bool}
#[derive(Deserialize)] struct SaveEnvironmentVariable{name:String,value:String,is_secret:bool}
#[derive(Deserialize)] struct SaveEnvironmentBatch{variables:Vec<SaveEnvironmentVariable>}
#[derive(Deserialize)] struct DeleteEnvironmentBatch{names:Vec<String>}
fn validate_variable(input:&SaveEnvironmentVariable)->Result<(),EnvironmentError>{
 if !valid_name(&input.name){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"invalid_environment_name","Use uppercase A-Z, digits and underscores; PORT, HOST and service credentials are reserved.".into()));}
 if input.value.len()>8192||input.value.contains('\0'){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"invalid_environment_value","Values must be at most 8192 bytes and cannot contain NUL.".into()));}
 if input.is_secret&&input.value.is_empty(){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"empty_secret","Secret values cannot be empty.".into()));}
 Ok(())
}
fn validate_batch(variables:&[SaveEnvironmentVariable])->Result<(),EnvironmentError>{
 if variables.is_empty()||variables.len()>64{return Err(EnvironmentError(StatusCode::BAD_REQUEST,"invalid_environment_batch","A batch must contain between 1 and 64 variables.".into()));}
 let mut names=HashSet::with_capacity(variables.len());
 for variable in variables{validate_variable(variable)?;if !names.insert(variable.name.as_str()){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"duplicate_environment_name","A batch cannot contain the same variable name more than once.".into()));}}
 Ok(())
}
fn validate_delete_batch(names:&[String])->Result<(),EnvironmentError>{
 if names.is_empty()||names.len()>64{return Err(EnvironmentError(StatusCode::BAD_REQUEST,"invalid_environment_batch","A delete batch must contain between 1 and 64 variable names.".into()));}
 let mut unique=HashSet::with_capacity(names.len());
 for name in names{if !valid_name(name){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"invalid_environment_name","Environment variable name is invalid.".into()));}if !unique.insert(name.as_str()){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"duplicate_environment_name","A delete batch cannot contain the same variable name more than once.".into()));}}
 Ok(())
}
fn restore_credentials(id:&str,snapshots:&[(String,Option<String>)])->bool{
 let mut restored=true;
 for(name,previous)in snapshots.iter().rev(){
  match credential_entry(id,name){
   Ok(entry)=>if let Some(value)=previous{if entry.set_password(value).is_err(){restored=false;}}else{if let Err(error)=entry.delete_credential(){if !matches!(error,keyring::Error::NoEntry){restored=false;}}},
   Err(_)=>restored=false,
  }
 }
 restored
}
pub fn router()->Router<AppState>{Router::new().route("/api/v1/websites/:id/environment",get(list).put(save)).route("/api/v1/websites/:id/environment/batch",axum::routing::put(save_batch).delete(remove_batch)).route("/api/v1/websites/:id/environment/:name",axum::routing::delete(remove))}
async fn list(State(s):State<AppState>,Path(id):Path<String>)->Result<Json<Vec<EnvironmentVariable>>,EnvironmentError>{
 let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM websites WHERE id=?1)",[&id],|r|r.get(0)).map_err(db_error)?;
 if !exists{return Err(EnvironmentError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into()));}
 let mut q=db.prepare("SELECT name,value,is_secret FROM website_environment WHERE website_id=?1 ORDER BY name").map_err(db_error)?;
 let rows=q.query_map([&id],|r|{let secret:i64=r.get(2)?;Ok(EnvironmentVariable{name:r.get(0)?,value:if secret==1{None}else{Some(r.get(1)?)},is_secret:secret==1})}).map_err(db_error)?;
 Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(db_error)?))
}
async fn save(State(s):State<AppState>,Path(id):Path<String>,Json(input):Json<SaveEnvironmentVariable>)->Result<Json<EnvironmentVariable>,EnvironmentError>{
 validate_variable(&input)?;
 {let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM websites WHERE id=?1)",[&id],|r|r.get(0)).map_err(db_error)?;if !exists{return Err(EnvironmentError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into()));}}
 let entry=credential_entry(&id,&input.name)?;
 if input.is_secret{entry.set_password(&input.value).map_err(|e|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"credential_save_failed",format!("Could not protect secret in Windows Credential Manager: {e}")))?;}
 let result={let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;db.execute("INSERT INTO website_environment(website_id,name,value,is_secret,updated_at) VALUES(?1,?2,?3,?4,CURRENT_TIMESTAMP) ON CONFLICT(website_id,name) DO UPDATE SET value=excluded.value,is_secret=excluded.is_secret,updated_at=CURRENT_TIMESTAMP",params![id,input.name,if input.is_secret{String::new()}else{input.value.clone()},if input.is_secret{1}else{0}]).map_err(db_error)};
 if let Err(e)=result{if input.is_secret{let _=entry.delete_credential();}return Err(e);}
 if !input.is_secret{let _=entry.delete_credential();}
 Ok(Json(EnvironmentVariable{name:input.name,value:if input.is_secret{None}else{Some(input.value)},is_secret:input.is_secret}))
}
async fn save_batch(State(s):State<AppState>,Path(id):Path<String>,Json(input):Json<SaveEnvironmentBatch>)->Result<Json<Vec<EnvironmentVariable>>,EnvironmentError>{
 validate_batch(&input.variables)?;
 {let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM websites WHERE id=?1)",[&id],|r|r.get(0)).map_err(db_error)?;if !exists{return Err(EnvironmentError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into()));}}
 // Stage protected values first and remember the previous credential contents so a
 // failed batch can compensate without leaving the old registry and secrets mismatched.
 let mut snapshots:Vec<(String,Option<String>)>=Vec::new();
 for variable in input.variables.iter().filter(|v|v.is_secret){
  let entry=match credential_entry(&id,&variable.name){Ok(entry)=>entry,Err(error)=>{
   let restored=restore_credentials(&id,&snapshots);
   return Err(EnvironmentError(error.0,error.1,if restored{error.2}else{format!("{} Credential rollback was incomplete; review this website's environment secrets.",error.2)}));
  }};
  let previous=match entry.get_password(){
   Ok(value)=>Some(value),
   Err(keyring::Error::NoEntry)=>None,
   Err(error)=>{
    let restored=restore_credentials(&id,&snapshots);
    let message=format!("Could not inspect the previous protected value in Windows Credential Manager: {error}");
    return Err(EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"credential_read_failed",if restored{message}else{format!("{message} Credential rollback was incomplete; review this website's environment secrets.")}));
   }
  };
  snapshots.push((variable.name.clone(),previous));
  if let Err(error)=entry.set_password(&variable.value){
   let restored=restore_credentials(&id,&snapshots);
   let message=format!("Could not protect secret in Windows Credential Manager: {error}");
   return Err(EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"credential_save_failed",if restored{message}else{format!("{message} Credential rollback was incomplete; review this website's environment secrets.")}));
  }
 }
 let database_result:Result<(),EnvironmentError>=(||{
  let mut db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  let tx=db.transaction().map_err(db_error)?;
  for variable in &input.variables{
   tx.execute("INSERT INTO website_environment(website_id,name,value,is_secret,updated_at) VALUES(?1,?2,?3,?4,CURRENT_TIMESTAMP) ON CONFLICT(website_id,name) DO UPDATE SET value=excluded.value,is_secret=excluded.is_secret,updated_at=CURRENT_TIMESTAMP",params![id,variable.name,if variable.is_secret{String::new()}else{variable.value.clone()},if variable.is_secret{1}else{0}]).map_err(db_error)?;
  }
  tx.commit().map_err(db_error)
 })();
 if let Err(error)=database_result{
  let restored=restore_credentials(&id,&snapshots);
  return Err(EnvironmentError(error.0,error.1,if restored{error.2}else{format!("{} Credential rollback was incomplete; review this website's environment secrets.",error.2)}));
 }
 // Non-secret replacements no longer need a credential entry after the DB commit.
 for variable in input.variables.iter().filter(|v|!v.is_secret){if let Ok(entry)=credential_entry(&id,&variable.name){let _=entry.delete_credential();}}
 Ok(Json(input.variables.into_iter().map(|variable|EnvironmentVariable{name:variable.name,value:if variable.is_secret{None}else{Some(variable.value)},is_secret:variable.is_secret}).collect()))
}


async fn remove_batch(State(s):State<AppState>,Path(id):Path<String>,Json(input):Json<DeleteEnvironmentBatch>)->Result<Json<serde_json::Value>,EnvironmentError>{
 validate_delete_batch(&input.names)?;
 {
  let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM websites WHERE id=?1)",[&id],|r|r.get(0)).map_err(db_error)?;
  if !exists{return Err(EnvironmentError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into()));}
 }
 // Capture all prior credential values before mutation so a failed registry commit
 // can restore the protected store to its previous state.
 let mut snapshots:Vec<(String,Option<String>)>=Vec::with_capacity(input.names.len());
 for name in &input.names{
  let entry=credential_entry(&id,name)?;
  let previous=match entry.get_password(){
   Ok(value)=>Some(value),
   Err(keyring::Error::NoEntry)=>None,
   Err(error)=>return Err(EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"credential_read_failed",format!("Could not inspect the existing protected value for {name}: {error}"))),
  };
  snapshots.push((name.clone(),previous));
 }
 for name in &input.names{
  let entry=match credential_entry(&id,name){
   Ok(entry)=>entry,
   Err(error)=>{
    let restored=restore_credentials(&id,&snapshots);
    return Err(EnvironmentError(error.0,error.1,if restored{error.2}else{format!("{} Credential rollback was incomplete; review this website's environment secrets.",error.2)}));
   }
  };
  if let Err(error)=entry.delete_credential(){
   if !matches!(error,keyring::Error::NoEntry){
    let restored=restore_credentials(&id,&snapshots);
    let message=format!("Could not remove protected environment value {name}: {error}");
    return Err(EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"credential_delete_failed",if restored{message}else{format!("{message} Credential rollback was incomplete; review this website's environment secrets.")}));
   }
  }
 }
 let database_result:Result<usize,EnvironmentError>=(||{
  let mut db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
  let tx=db.transaction().map_err(db_error)?;
  let mut removed=0usize;
  for name in &input.names{removed+=tx.execute("DELETE FROM website_environment WHERE website_id=?1 AND name=?2",params![id,name]).map_err(db_error)?;}
  tx.commit().map_err(db_error)?;
  Ok(removed)
 })();
 match database_result{
  Ok(removed)=>Ok(Json(json!({"removed":removed,"requested":input.names.len()}))),
  Err(error)=>{
   let restored=restore_credentials(&id,&snapshots);
   Err(EnvironmentError(error.0,error.1,if restored{error.2}else{format!("{} Credential rollback was incomplete; review this website's environment secrets.",error.2)}))
  }
 }
}

async fn remove(State(s):State<AppState>,Path((id,name)):Path<(String,String)>)->Result<StatusCode,EnvironmentError>{
 if !valid_name(&name){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"invalid_environment_name","Environment variable name is invalid.".into()));}
 let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let n=db.execute("DELETE FROM website_environment WHERE website_id=?1 AND name=?2",params![id,name]).map_err(db_error)?;
 if n==0{return Err(EnvironmentError(StatusCode::NOT_FOUND,"environment_variable_not_found","Environment variable was not found.".into()));}drop(db);
 if let Ok(entry)=credential_entry(&id,&name){let _=entry.delete_credential();}Ok(StatusCode::NO_CONTENT)
}
pub fn apply_to_command(s:&AppState,id:&str,c:&mut std::process::Command)->Result<(),String>{
 let rows={let db=s.db.lock().map_err(|_|"Database lock is unavailable.".to_string())?;let mut q=db.prepare("SELECT name,value,is_secret FROM website_environment WHERE website_id=?1 ORDER BY name").map_err(|e|e.to_string())?;let mapped=q.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?))).map_err(|e|e.to_string())?;mapped.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?};
 for(name,value,secret)in rows{let value=if secret==1{read_secret_value(s,id,&name)?}else{value};c.env(name,value);}Ok(())
}
pub fn delete_all_for_website(state:&AppState,id:&str){
 let names=state.db.lock().ok().and_then(|db|{
  let mut q=db.prepare("SELECT name FROM website_environment WHERE website_id=?1").ok()?;
  let rows=q.query_map([id],|r|r.get::<_,String>(0)).ok()?;
  rows.collect::<Result<Vec<_>,_>>().ok()
 }).unwrap_or_default();
 for name in names{if let Ok(entry)=credential_entry(id,&name){let _=entry.delete_credential();}}
}
#[cfg(test)]
mod tests{
 use super::{validate_batch,validate_delete_batch,valid_name,SaveEnvironmentVariable};
 fn variable(name:&str,value:&str,is_secret:bool)->SaveEnvironmentVariable{
  SaveEnvironmentVariable{name:name.into(),value:value.into(),is_secret}
 }
 #[test]
 fn environment_names_are_safe_and_reserved_names_are_rejected(){
  assert!(valid_name("DATABASE_URL"));
  assert!(valid_name("_FEATURE_FLAG2"));
  assert!(!valid_name("database_url"));
  assert!(!valid_name("2BAD"));
  assert!(!valid_name("BAD-NAME"));
  assert!(!valid_name("PORT"));
  assert!(!valid_name("HOST"));
  assert!(!valid_name("WEBDYNAMITE_API_TOKEN"));
 }
 #[test]
 fn environment_batch_accepts_valid_plain_and_secret_values(){
  let values=vec![variable("DB_HOST","localhost",false),variable("DB_PASSWORD","secret",true)];
  assert!(validate_batch(&values).is_ok());
 }
 #[test]
 fn environment_batch_rejects_empty_oversized_and_duplicate_batches(){
  assert!(validate_batch(&[]).is_err());
  let mut oversized=Vec::new();
  for index in 0..65{oversized.push(variable(&format!("VALUE_{index}"),"x",false));}
  assert!(validate_batch(&oversized).is_err());
  assert!(validate_batch(&[variable("DB_HOST","localhost",false),variable("DB_HOST","other",false)]).is_err());
 }
 #[test]
 fn environment_delete_batch_accepts_unique_valid_names(){
  assert!(validate_delete_batch(&vec!["DATABASE_URL".into(),"DB_PASSWORD".into()]).is_ok());
 }
 #[test]
 fn environment_delete_batch_rejects_empty_oversized_duplicate_and_invalid_names(){
  assert!(validate_delete_batch(&[]).is_err());
  let oversized=(0..65).map(|index|format!("VALUE_{index}")).collect::<Vec<_>>();
  assert!(validate_delete_batch(&oversized).is_err());
  assert!(validate_delete_batch(&vec!["DB_HOST".into(),"DB_HOST".into()]).is_err());
  assert!(validate_delete_batch(&vec!["bad-name".into()]).is_err());
  assert!(validate_delete_batch(&vec!["WEBDYNAMITE_API_TOKEN".into()]).is_err());
 }
 #[test]
 fn environment_batch_rejects_invalid_values_before_persistence(){
  assert!(validate_batch(&[variable("bad_name","x",false)]).is_err());
  assert!(validate_batch(&[variable("PORT","8080",false)]).is_err());
  assert!(validate_batch(&[variable("DB_PASSWORD","",true)]).is_err());
  assert!(validate_batch(&[variable("DB_VALUE","bad\0value",false)]).is_err());
  assert!(validate_batch(&[variable("DB_VALUE",&"x".repeat(8193),false)]).is_err());
 }
}
