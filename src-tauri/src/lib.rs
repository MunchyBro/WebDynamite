use serde::{Deserialize, Serialize};
use serde_json::Value;
const CORE_URL: &str = "http://127.0.0.1:47821";
#[derive(Debug, Serialize, Deserialize)]
struct Website { id:String, name:String, project_path:String, runtime:String, status:String, created_at:String, updated_at:String }
#[derive(Debug, Serialize, Deserialize)]
struct Domain { id:String, website_id:String, website_name:String, hostname:String, access_mode:String, created_at:String }
fn api_error(context:&str,error:impl std::fmt::Display)->String{format!("{context}: {error}")}
async fn authorized_request(token:&str,method:reqwest::Method,path:&str,body:Option<Value>)->Result<reqwest::Response,String>{
 if token.trim().len()<32{return Err("API token must contain at least 32 characters.".into());}
 let client=reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(2)).timeout(std::time::Duration::from_secs(15)).build().map_err(|e|api_error("Could not initialize HTTP client",e))?;
 let mut request=client.request(method,format!("{CORE_URL}{path}")).bearer_auth(token.trim());
 if let Some(json)=body{request=request.json(&json);}
 let response=request.send().await.map_err(|e|api_error("Could not reach WebDynamite Core Service at 127.0.0.1:47821",e))?;
 if !response.status().is_success(){let status=response.status();let detail=response.text().await.unwrap_or_default();return Err(format!("Core Service returned {status}: {detail}"));}
 Ok(response)
}
#[tauri::command]
async fn core_health()->Result<Value,String>{
 let client=reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(2)).timeout(std::time::Duration::from_secs(5)).build().map_err(|e|api_error("HTTP client error",e))?;
 let response=client.get(format!("{CORE_URL}/api/v1/health")).send().await.map_err(|e|api_error("Core Service is not reachable. Start it and try again",e))?;
 if !response.status().is_success(){return Err(format!("Core Service health check failed: {}",response.status()));}
 response.json().await.map_err(|e|api_error("Invalid health response",e))
}
#[tauri::command]
async fn list_websites(token:String)->Result<Vec<Website>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/websites",None).await?.json().await.map_err(|e|api_error("Could not read website registry",e))}
#[tauri::command]
async fn create_website(token:String,name:String,project_path:String,runtime:String)->Result<Website,String>{
 let body=serde_json::json!({"name":name,"project_path":project_path,"runtime":runtime});
 authorized_request(&token,reqwest::Method::POST,"/api/v1/websites",Some(body)).await?.json().await.map_err(|e|api_error("Could not read created website",e))
}
#[tauri::command]
async fn website_action(token:String,id:String,action:String)->Result<Value,String>{
 if !["start","stop","restart"].contains(&action.as_str()){return Err("Unsupported website action.".into());}
 let path=format!("/api/v1/websites/{id}/{action}");
 authorized_request(&token,reqwest::Method::POST,&path,None).await?.json().await.map_err(|e|api_error("Could not read website operation result",e))
}
#[tauri::command]
async fn website_logs(token:String,id:String)->Result<Value,String>{
 let path=format!("/api/v1/websites/{id}/logs");
 authorized_request(&token,reqwest::Method::GET,&path,None).await?.json().await.map_err(|e|api_error("Could not read website logs",e))
}
#[tauri::command]
async fn delete_website(token:String,id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/websites/{id}"),None).await?;Ok(())}

#[tauri::command]
async fn diagnose_domain(token:String,hostname:String,expected_ipv4:String)->Result<Value,String>{
 let body=serde_json::json!({"hostname":hostname,"expected_ipv4":expected_ipv4});
 authorized_request(&token,reqwest::Method::POST,"/api/v1/domains/diagnose",Some(body)).await?.json().await.map_err(|e|api_error("Could not read DNS diagnostic",e))
}
#[tauri::command]
async fn list_public_domains(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/public-domains",None).await?.json().await.map_err(|e|api_error("Could not read public domain registry",e))}
#[tauri::command]
async fn create_public_domain(token:String,website_id:String,hostname:String)->Result<Value,String>{
 let body=serde_json::json!({"website_id":website_id,"hostname":hostname});
 authorized_request(&token,reqwest::Method::POST,"/api/v1/public-domains",Some(body)).await?.json().await.map_err(|e|api_error("Could not assign public domain",e))
}
#[tauri::command]
async fn delete_public_domain(token:String,id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/public-domains/{id}"),None).await?;Ok(())}
#[tauri::command]
async fn list_domains(token:String)->Result<Vec<Domain>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/domains",None).await?.json().await.map_err(|e|api_error("Could not read domain registry",e))}
#[tauri::command]
async fn create_domain(token:String,website_id:String,hostname:String)->Result<Domain,String>{
 let body=serde_json::json!({"website_id":website_id,"hostname":hostname});
 authorized_request(&token,reqwest::Method::POST,"/api/v1/domains",Some(body)).await?.json().await.map_err(|e|api_error("Could not read created domain",e))
}
#[tauri::command]
async fn delete_domain(token:String,id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/domains/{id}"),None).await?;Ok(())}
#[tauri::command]
async fn export_caddyfile(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/domains/caddyfile",None).await?.json().await.map_err(|e|api_error("Could not export Caddyfile",e))}
#[tauri::command]
async fn networking_status(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/networking/status",None).await?.json().await.map_err(|e|api_error("Could not read network status",e))}
#[tauri::command]
async fn set_lan_access(token:String,id:String,enabled:bool)->Result<Value,String>{
 let body=serde_json::json!({"enabled":enabled});
 authorized_request(&token,reqwest::Method::PUT,&format!("/api/v1/websites/{id}/lan"),Some(body)).await?.json().await.map_err(|e|api_error("Could not change LAN access",e))
}
#[tauri::command]
async fn proxy_status(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/proxy/status",None).await?.json().await.map_err(|e|api_error("Could not read Caddy status",e))}
#[tauri::command]
async fn proxy_action(token:String,action:String)->Result<Value,String>{if !["start","stop","reload","validate"].contains(&action.as_str()){return Err("Unsupported Caddy action.".into());} authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/proxy/{action}"),None).await?.json().await.map_err(|e|api_error("Caddy operation failed",e))}
#[cfg_attr(mobile,tauri::mobile_entry_point)]
pub fn run(){tauri::Builder::default().invoke_handler(tauri::generate_handler![core_health,list_websites,create_website,website_action,website_logs,delete_website,list_domains,create_domain,delete_domain,diagnose_domain,list_public_domains,create_public_domain,delete_public_domain,export_caddyfile,proxy_status,proxy_action,networking_status,set_lan_access]).run(tauri::generate_context!()).expect("error while running WebDynamite");}
