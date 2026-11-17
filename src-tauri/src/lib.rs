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
 let client=reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(2)).timeout(std::time::Duration::from_secs(600)).build().map_err(|e|api_error("Could not initialize HTTP client",e))?;
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
async fn list_databases(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/databases",None).await?.json().await.map_err(|e|api_error("Could not read managed databases",e))}
#[tauri::command]
async fn create_database(token:String,name:String)->Result<Value,String>{
 let body=serde_json::json!({"name":name});
 authorized_request(&token,reqwest::Method::POST,"/api/v1/databases",Some(body)).await?.json().await.map_err(|e|api_error("Could not create SQLite database",e))
}
#[tauri::command]
async fn delete_database(token:String,id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/databases/{id}"),None).await?;Ok(())}
#[tauri::command]
async fn test_database(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/databases/{id}/test"),None).await?.json().await.map_err(|e|api_error("Could not test database",e))}
#[tauri::command]
async fn monitoring_metrics(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/monitoring/metrics",None).await?.json().await.map_err(|e|api_error("Could not read system metrics",e))}
#[tauri::command]
async fn networking_status(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/networking/status",None).await?.json().await.map_err(|e|api_error("Could not read network status",e))}
#[tauri::command]
async fn set_lan_access(token:String,id:String,enabled:bool)->Result<Value,String>{
 let body=serde_json::json!({"enabled":enabled});
 authorized_request(&token,reqwest::Method::PUT,&format!("/api/v1/websites/{id}/lan"),Some(body)).await?.json().await.map_err(|e|api_error("Could not change LAN access",e))
}
#[tauri::command]
async fn proxy_tls_check(token:String,hostname:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/proxy/tls-check/{}",encode_query_value(&hostname)),None).await?.json().await.map_err(|e|api_error("HTTPS verification failed",e))}
#[tauri::command]
async fn proxy_status(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/proxy/status",None).await?.json().await.map_err(|e|api_error("Could not read Caddy status",e))}
#[tauri::command]
async fn proxy_action(token:String,action:String)->Result<Value,String>{if !["start","stop","reload","validate"].contains(&action.as_str()){return Err("Unsupported Caddy action.".into());} authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/proxy/{action}"),None).await?.json().await.map_err(|e|api_error("Caddy operation failed",e))}

fn encode_query_value(value:&str)->String{
    let mut encoded=String::new();
    for byte in value.bytes(){
        if byte.is_ascii_alphanumeric()||b"-_.~".contains(&byte){encoded.push(byte as char);}
        else{encoded.push_str(&format!("%{byte:02X}"));}
    }
    encoded
}
#[tauri::command]
async fn website_storage_usage(token:String,id:String)->Result<Value,String>{
 authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/websites/{id}/storage"),None).await?.json().await.map_err(|e|api_error("Could not calculate website storage usage",e))
}
#[tauri::command]
async fn list_website_files(token:String,id:String,path:String)->Result<Value,String>{
 let url_path=format!("/api/v1/websites/{id}/files?path={}",encode_query_value(&path));
 authorized_request(&token,reqwest::Method::GET,&url_path,None).await?.json().await.map_err(|e|api_error("Could not list website files",e))
}
#[tauri::command]
async fn read_website_file(token:String,id:String,path:String)->Result<Value,String>{
 let url_path=format!("/api/v1/websites/{id}/file?path={}",encode_query_value(&path));
 authorized_request(&token,reqwest::Method::GET,&url_path,None).await?.json().await.map_err(|e|api_error("Could not read website file",e))
}
#[tauri::command]
async fn write_website_file(token:String,id:String,path:String,content:String)->Result<Value,String>{
 let body=serde_json::json!({"path":path,"content":content});
 authorized_request(&token,reqwest::Method::PUT,&format!("/api/v1/websites/{id}/file"),Some(body)).await?.json().await.map_err(|e|api_error("Could not save website file",e))
}
#[tauri::command]
async fn upload_website_file(token:String,id:String,path:String,content_base64:String)->Result<Value,String>{
 let body=serde_json::json!({"path":path,"content_base64":content_base64});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/websites/{id}/files/upload"),Some(body)).await?.json().await.map_err(|e|api_error("Could not upload website file",e))
}
#[tauri::command]
async fn download_website_file(token:String,id:String,path:String)->Result<Value,String>{
 let url_path=format!("/api/v1/websites/{id}/files/download?path={}",encode_query_value(&path));
 authorized_request(&token,reqwest::Method::GET,&url_path,None).await?.json().await.map_err(|e|api_error("Could not download website file",e))
}
#[tauri::command]
async fn download_website_archive(token:String,id:String,path:String)->Result<Value,String>{
 let url_path=format!("/api/v1/websites/{id}/files/archive?path={}",encode_query_value(&path));
 authorized_request(&token,reqwest::Method::GET,&url_path,None).await?.json().await.map_err(|e|api_error("Could not create website ZIP archive",e))
}
#[tauri::command]
async fn extract_website_archive(token:String,id:String,path:String,archive_base64:String)->Result<Value,String>{
 let body=serde_json::json!({"path":path,"archive_base64":archive_base64});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/websites/{id}/files/extract"),Some(body)).await?.json().await.map_err(|e|api_error("Could not extract website ZIP archive",e))
}
#[tauri::command]
async fn create_website_directory(token:String,id:String,path:String)->Result<Value,String>{
 let body=serde_json::json!({"path":path});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/websites/{id}/files/directory"),Some(body)).await?.json().await.map_err(|e|api_error("Could not create website folder",e))
}
#[tauri::command]
async fn rename_website_file(token:String,id:String,source_path:String,new_path:String)->Result<Value,String>{
 let body=serde_json::json!({"source_path":source_path,"new_path":new_path});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/websites/{id}/files/rename"),Some(body)).await?.json().await.map_err(|e|api_error("Could not rename website file",e))
}
#[tauri::command]
async fn delete_website_file(token:String,id:String,path:String)->Result<(),String>{
 let url_path=format!("/api/v1/websites/{id}/file?path={}",encode_query_value(&path));
 authorized_request(&token,reqwest::Method::DELETE,&url_path,None).await?;Ok(())
}


#[tauri::command]
async fn list_build_profiles(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/deployments/profiles",None).await?.json().await.map_err(|e|api_error("Could not read build profiles",e))}
#[tauri::command]
async fn save_build_profile(token:String,website_id:String,program:String,args:Vec<String>,output_dir:String)->Result<Value,String>{
 let body=serde_json::json!({"website_id":website_id,"program":program,"args":args,"output_dir":output_dir});
 authorized_request(&token,reqwest::Method::PUT,"/api/v1/deployments/profiles",Some(body)).await?.json().await.map_err(|e|api_error("Could not save build profile",e))
}
#[tauri::command]
async fn run_build(token:String,website_id:String)->Result<Value,String>{
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/deployments/{website_id}/run"),None).await?.json().await.map_err(|e|api_error("Build failed",e))
}
#[tauri::command]
async fn deploy_website(token:String,website_id:String)->Result<Value,String>{
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/deployments/{website_id}/deploy"),None).await?.json().await.map_err(|e|api_error("Automated deployment failed",e))
}
#[tauri::command]
async fn list_build_runs(token:String,website_id:String)->Result<Vec<Value>,String>{
 let suffix=if website_id.is_empty(){String::new()}else{format!("?website_id={website_id}")};
 authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/deployments/runs{suffix}"),None).await?.json().await.map_err(|e|api_error("Could not read build history",e))
}


#[tauri::command]
async fn git_repository_status(token:String,website_id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/git/{website_id}/status"),None).await?.json().await.map_err(|e|api_error("Could not read Git status",e))}
#[tauri::command]
async fn git_fetch(token:String,website_id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/git/{website_id}/fetch"),None).await?.json().await.map_err(|e|api_error("Git fetch failed",e))}
#[tauri::command]
async fn git_pull(token:String,website_id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/git/{website_id}/pull"),None).await?.json().await.map_err(|e|api_error("Git pull failed",e))}
#[tauri::command]
async fn list_deployment_releases(token:String,website_id:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/deployments/releases?website_id={website_id}"),None).await?.json().await.map_err(|e|api_error("Could not read release history",e))}
#[tauri::command]
async fn create_deployment_release(token:String,website_id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/deployments/{website_id}/release"),None).await?.json().await.map_err(|e|api_error("Could not create release snapshot",e))}
#[tauri::command]
async fn rollback_deployment_release(token:String,website_id:String,release_id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/deployments/{website_id}/rollback/{release_id}"),None).await?.json().await.map_err(|e|api_error("Could not activate previous release",e))}


#[tauri::command]
async fn list_tunnels(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/tunnels",None).await?.json().await.map_err(|e|api_error("Could not read tunnel status",e))}
#[tauri::command]
async fn tunnel_action(token:String,website_id:String,action:String)->Result<Value,String>{
 if action=="start"{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/tunnels/{website_id}/start"),None).await?.json().await.map_err(|e|api_error("Could not start tunnel",e))}
 else if action=="stop"{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/tunnels/{website_id}/stop"),None).await?;Ok(serde_json::json!({"stopped":true}))}
 else{Err("Unsupported tunnel action.".into())}
}
#[tauri::command]
async fn get_backup_schedule(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/backups/schedule",None).await?.json().await.map_err(|e|api_error("Could not read backup schedule",e))}
#[tauri::command]
async fn save_backup_schedule(token:String,enabled:bool,interval_hours:i64,retention_count:i64)->Result<Value,String>{authorized_request(&token,reqwest::Method::PUT,"/api/v1/backups/schedule",Some(serde_json::json!({"enabled":enabled,"interval_hours":interval_hours,"retention_count":retention_count}))).await?.json().await.map_err(|e|api_error("Could not save backup schedule",e))}
#[tauri::command]
async fn list_backups(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/backups",None).await?.json().await.map_err(|e|api_error("Could not read backup history",e))}
#[tauri::command]
async fn create_backup(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,"/api/v1/backups",Some(serde_json::json!({}))).await?.json().await.map_err(|e|api_error("Could not create backup",e))}
#[tauri::command]
async fn verify_backup(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/backups/{id}/verify"),None).await?.json().await.map_err(|e|api_error("Backup verification failed",e))}
#[tauri::command]
async fn preview_backup_restore(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/backups/{id}/preview"),None).await?.json().await.map_err(|e|api_error("Could not preview backup restore",e))}
#[tauri::command]
async fn restore_backup(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/backups/{id}/restore"),Some(serde_json::json!({"confirm":true}))).await?.json().await.map_err(|e|api_error("Backup restore failed",e))}
#[tauri::command]
async fn delete_backup(token:String,id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/backups/{id}"),None).await?;Ok(())}


#[tauri::command]
async fn get_app_settings(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/settings",None).await?.json().await.map_err(|e|api_error("Could not load settings",e))}
#[tauri::command]
async fn save_app_settings(token:String,settings:Value)->Result<Value,String>{authorized_request(&token,reqwest::Method::PUT,"/api/v1/settings",Some(settings)).await?.json().await.map_err(|e|api_error("Could not save settings",e))}
#[tauri::command]
async fn diagnostics_report(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/diagnostics",None).await?.json().await.map_err(|e|api_error("Could not run diagnostics",e))}
#[tauri::command]
async fn recover_website_states(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,"/api/v1/recovery/reconcile",Some(serde_json::json!({}))).await?.json().await.map_err(|e|api_error("Recovery reconciliation failed",e))}

#[tauri::command]
async fn list_runtime_profiles(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/runtime-profiles",None).await?.json().await.map_err(|e|api_error("Could not read runtime profiles",e))}
#[tauri::command]
async fn save_runtime_profile(token:String,website_id:String,program:String,args:Vec<String>)->Result<Value,String>{authorized_request(&token,reqwest::Method::PUT,"/api/v1/runtime-profiles",Some(serde_json::json!({"website_id":website_id,"program":program,"args":args}))).await?.json().await.map_err(|e|api_error("Could not save runtime profile",e))}
#[tauri::command]
async fn delete_runtime_profile(token:String,website_id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/runtime-profiles/{website_id}"),None).await?;Ok(())}

#[cfg_attr(mobile,tauri::mobile_entry_point)]
pub fn run(){tauri::Builder::default().invoke_handler(tauri::generate_handler![core_health,list_websites,create_website,website_action,website_logs,delete_website,list_domains,create_domain,delete_domain,diagnose_domain,list_public_domains,create_public_domain,delete_public_domain,export_caddyfile,proxy_status,proxy_tls_check,proxy_action,networking_status,monitoring_metrics,set_lan_access,list_databases,create_database,delete_database,test_database,website_storage_usage,list_website_files,read_website_file,write_website_file,upload_website_file,download_website_file,download_website_archive,extract_website_archive,create_website_directory,rename_website_file,delete_website_file,list_runtime_profiles,save_runtime_profile,delete_runtime_profile,list_build_profiles,save_build_profile,run_build,deploy_website,list_build_runs,git_repository_status,git_fetch,git_pull,list_deployment_releases,create_deployment_release,rollback_deployment_release,list_tunnels,tunnel_action,get_backup_schedule,save_backup_schedule,list_backups,create_backup,verify_backup,preview_backup_restore,restore_backup,delete_backup,get_app_settings,save_app_settings,diagnostics_report,recover_website_states]).run(tauri::generate_context!()).expect("error while running WebDynamite");}
