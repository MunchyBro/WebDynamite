use axum::{extract::{Path, State}, routing::get, Json, Router, http::StatusCode};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::AppState;
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
pub fn router()->Router<AppState>{Router::new().route("/api/v1/websites/:id/environment",get(list).put(save)).route("/api/v1/websites/:id/environment/:name",axum::routing::delete(remove))}
async fn list(State(s):State<AppState>,Path(id):Path<String>)->Result<Json<Vec<EnvironmentVariable>>,EnvironmentError>{
 let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;
 let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM websites WHERE id=?1)",[&id],|r|r.get(0)).map_err(db_error)?;
 if !exists{return Err(EnvironmentError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into()));}
 let mut q=db.prepare("SELECT name,value,is_secret FROM website_environment WHERE website_id=?1 ORDER BY name").map_err(db_error)?;
 let rows=q.query_map([&id],|r|{let secret:i64=r.get(2)?;Ok(EnvironmentVariable{name:r.get(0)?,value:if secret==1{None}else{Some(r.get(1)?)},is_secret:secret==1})}).map_err(db_error)?;
 Ok(Json(rows.collect::<Result<Vec<_>,_>>().map_err(db_error)?))
}
async fn save(State(s):State<AppState>,Path(id):Path<String>,Json(input):Json<SaveEnvironmentVariable>)->Result<Json<EnvironmentVariable>,EnvironmentError>{
 if !valid_name(&input.name){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"invalid_environment_name","Use uppercase A-Z, digits and underscores; PORT, HOST and service credentials are reserved.".into()));}
 if input.value.len()>8192||input.value.contains('\0'){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"invalid_environment_value","Values must be at most 8192 bytes and cannot contain NUL.".into()));}
 if input.is_secret&&input.value.is_empty(){return Err(EnvironmentError(StatusCode::BAD_REQUEST,"empty_secret","Secret values cannot be empty.".into()));}
 {let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM websites WHERE id=?1)",[&id],|r|r.get(0)).map_err(db_error)?;if !exists{return Err(EnvironmentError(StatusCode::NOT_FOUND,"website_not_found","Website was not found.".into()));}}
 let entry=credential_entry(&id,&input.name)?;
 if input.is_secret{entry.set_password(&input.value).map_err(|e|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"credential_save_failed",format!("Could not protect secret in Windows Credential Manager: {e}")))?;}
 let result={let db=s.db.lock().map_err(|_|EnvironmentError(StatusCode::INTERNAL_SERVER_ERROR,"database_locked","Database lock is unavailable.".into()))?;db.execute("INSERT INTO website_environment(website_id,name,value,is_secret,updated_at) VALUES(?1,?2,?3,?4,CURRENT_TIMESTAMP) ON CONFLICT(website_id,name) DO UPDATE SET value=excluded.value,is_secret=excluded.is_secret,updated_at=CURRENT_TIMESTAMP",params![id,input.name,if input.is_secret{String::new()}else{input.value.clone()},if input.is_secret{1}else{0}]).map_err(db_error)};
 if let Err(e)=result{if input.is_secret{let _=entry.delete_credential();}return Err(e);}
 if !input.is_secret{let _=entry.delete_credential();}
 Ok(Json(EnvironmentVariable{name:input.name,value:if input.is_secret{None}else{Some(input.value)},is_secret:input.is_secret}))
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
 for(name,value,secret)in rows{let value=if secret==1{credential_entry(id,&name).map_err(|e|e.2)?.get_password().map_err(|e|format!("Secret environment variable {name} is unavailable in Windows Credential Manager: {e}"))?}else{value};c.env(name,value);}Ok(())
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
 use super::valid_name;
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
}
