use axum::{extract::{Path,State},routing::get,Json,Router,http::StatusCode};
use serde::Serialize;
use std::{io::{BufRead,BufReader},process::{Command,Stdio},thread,time::{Duration,SystemTime,UNIX_EPOCH}};
use crate::AppState;

#[derive(Serialize)]
pub struct TunnelInfo{website_id:String,website_name:String,runtime:String,running:bool,url:Option<String>,started_at:Option<u64>}
fn now()->u64{SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()}
fn error(status:StatusCode,message:impl Into<String>)->(StatusCode,Json<serde_json::Value>){(status,Json(serde_json::json!({"error":{"code":"tunnel_error","message":message.into()}})))}

pub async fn list(State(state):State<AppState>)->Result<Json<Vec<TunnelInfo>>,(StatusCode,Json<serde_json::Value>)>{
 let sites={let db=state.db.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"Database lock unavailable"))?;
  let mut q=db.prepare("SELECT id,name,runtime,status,port FROM websites ORDER BY name").map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
  let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<u16>>(4)?))).map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
  rows.collect::<Result<Vec<_>,_>>().map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?};
 let mut out=Vec::new();let mut children=state.tunnel_processes.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"Tunnel process registry unavailable"))?;
 let urls=state.tunnel_urls.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"Tunnel URL registry unavailable"))?;
 for(id,name,runtime,status,port)in sites{
  let running=if let Some(child)=children.get_mut(&id){match child.try_wait(){Ok(None)=>true,Ok(Some(_))|Err(_)=>{children.remove(&id);false}}}else{false};
  let url=if running{urls.get(&id).cloned()}else{None};
  let started_at=if running{Some(now())}else{None};
  let _=(status,port);
  out.push(TunnelInfo{website_id:id,website_name:name,runtime,running,url,started_at});
 }
 Ok(Json(out))
}

pub async fn start(State(state):State<AppState>,Path(id):Path<String>)->Result<Json<TunnelInfo>,(StatusCode,Json<serde_json::Value>)>{
 let (name,runtime,status,port)={let db=state.db.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"Database lock unavailable"))?;
  db.query_row("SELECT name,runtime,status,port FROM websites WHERE id=?1",[&id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<u16>>(3)?))).map_err(|_|error(StatusCode::NOT_FOUND,"Website not found"))?};
 if runtime!="static"&&status!="running"{return Err(error(StatusCode::CONFLICT,"Start the website before publishing it through a tunnel."));}
 let origin=if runtime=="static"{format!("http://127.0.0.1:47821/hosted/{id}")}else{format!("http://127.0.0.1:{}",port.ok_or_else(||error(StatusCode::CONFLICT,"Website has no allocated runtime port."))?)};
 let mut children=state.tunnel_processes.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"Tunnel process registry unavailable"))?;
 if let Some(child)=children.get_mut(&id){if matches!(child.try_wait(),Ok(None)){return Err(error(StatusCode::CONFLICT,"A tunnel is already running for this website."));}children.remove(&id);}
 let mut child=Command::new(&state.cloudflared_bin).args(["tunnel","--url",&origin,"--no-autoupdate"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e|error(StatusCode::SERVICE_UNAVAILABLE,format!("cloudflared could not start. Install cloudflared or set CLOUDFLARED_BIN. {e}")))?;
 let stdout=child.stdout.take().ok_or_else(||error(StatusCode::INTERNAL_SERVER_ERROR,"Could not capture tunnel output"))?;
 let urls=state.tunnel_urls.clone();let key=id.clone();
 thread::spawn(move||{for line in BufReader::new(stdout).lines().flatten(){if let Some(start)=line.find("https://"){let candidate=&line[start..];let end=candidate.find(char::is_whitespace).unwrap_or(candidate.len());let url=candidate[..end].trim_end_matches(|c:char|matches!(c,')'|','|'.'));if url.ends_with(".trycloudflare.com"){if let Ok(mut map)=urls.lock(){map.insert(key.clone(),url.to_string());}break;}}}});
 children.insert(id.clone(),child);
 Ok(Json(TunnelInfo{website_id:id,website_name:name,runtime,running:true,url:None,started_at:Some(now())}))
}

pub async fn stop(State(state):State<AppState>,Path(id):Path<String>)->Result<StatusCode,(StatusCode,Json<serde_json::Value>)>{
 if let Ok(mut children)=state.tunnel_processes.lock(){if let Some(mut child)=children.remove(&id){let _=child.kill();let _=child.wait();}}
 if let Ok(mut urls)=state.tunnel_urls.lock(){urls.remove(&id);}
 Ok(StatusCode::NO_CONTENT)
}

pub fn stop_all(state:&AppState){if let Ok(mut children)=state.tunnel_processes.lock(){for(_,mut child)in children.drain(){let _=child.kill();let _=child.wait();}}if let Ok(mut urls)=state.tunnel_urls.lock(){urls.clear();}}
