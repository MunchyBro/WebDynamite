pub mod ipc_protocol;
pub mod named_pipe_client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use keyring::Entry;
use std::{io::{self, BufRead, BufReader, Write}, net::{SocketAddr, TcpStream}, process::{Command, Stdio}, thread, time::{Duration, Instant}};
use tauri::Manager;
use zeroize::{Zeroize, Zeroizing};
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
fn core_service_token_entry() -> Result<Entry, String> {
    Entry::new("WebDynamite.CoreService", "api-token")
        .map_err(|e| api_error("Could not access the WebDynamite Core Service credential", e))
}

fn load_or_create_legacy_core_service_token() -> Result<String, String> {
    let entry = core_service_token_entry()?;
    if let Ok(token) = entry.get_password() {
        if token.len() >= 32 { return Ok(token); }
    }
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    entry.set_password(&token)
        .map_err(|e| api_error("Could not securely store the Core Service API token", e))?;
    Ok(token)
}

#[tauri::command]
fn core_service_token(app: tauri::AppHandle) -> Result<String, String> {
    load_or_create_core_service_token(&app)
}

fn bundled_core_service_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let resource_path = app.path().resource_dir()
        .map_err(|e| api_error("Could not locate application resources", e))?
        .join("webdynamite-core.exe");
    if resource_path.is_file() { return Ok(resource_path); }

    let development_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/release/webdynamite-core.exe");
    if development_path.is_file() { return Ok(development_path); }
    Err(format!("Bundled Core Service was not found. Expected {}. Build core-service in release mode before running the desktop app.", resource_path.display()))
}

fn existing_core_service_matches_token(address: &SocketAddr, token: &str) -> io::Result<bool> {
    let mut stream = match TcpStream::connect_timeout(address, Duration::from_millis(250)) {
        Ok(stream) => stream,
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => return Ok(false),
        Err(error) => return Err(error),
    };
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    write!(
        stream,
        "GET /api/v1/websites HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    )?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    Ok(is_success_status_line(&status_line))
}

fn start_core_service(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let trusted_ipc = trusted_ipc_configuration_present().map_err(io::Error::other)?;
    let token = Zeroizing::new(load_or_create_core_service_token(app).map_err(io::Error::other)?);
    let address: SocketAddr = "127.0.0.1:47821".parse().map_err(io::Error::other)?;

    // Reuse the background service only when it proves possession of our protected token.
    // A different process on the management port must never be treated as WebDynamite.
    if trusted_ipc {
        // The named-pipe worker can accept the bootstrap request slightly before
        // the HTTP listener finishes binding during automatic service startup.
        // Retry transient connection/read failures, but never launch an interactive
        // duplicate in a trusted build.
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            if matches!(existing_core_service_matches_token(&address, token.as_str()), Ok(true)) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(150));
        }
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "The authenticated Windows Core Service did not expose its loopback API within 12 seconds.",
        ).into());
    }
    if existing_core_service_matches_token(&address, token.as_str())? {
        return Ok(());
    }
    if TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "Another process is already listening on the WebDynamite Core Service port 47821, and it did not authenticate as this installation.",
        ).into());
    }

    let executable = bundled_core_service_path(app).map_err(io::Error::other)?;
    let mut command = Command::new(executable);
    command.env("WEBDYNAMITE_API_TOKEN", token.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!("WebDynamite Core Service exited during startup: {status}")).into());
        }
        if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
            // Dropping std::process::Child does not terminate the process. The Core Service
            // intentionally outlives this desktop window and is reattached on the next launch.
            drop(child);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(150));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(io::Error::new(io::ErrorKind::TimedOut, "WebDynamite Core Service did not become ready within 12 seconds").into())
}

fn database_credential_entry(engine:&str,profile_id:&str)->Result<Entry,String>{
 let service=match engine{"mysql"=>"WebDynamite.Database.MySQL","postgres"=>"WebDynamite.Database.PostgreSQL",_=>return Err("Unsupported database credential type.".into())};
 let id=profile_id.trim();
 if id.is_empty()||id.len()>128||!id.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-'){return Err("Invalid database profile identifier.".into());}
 Entry::new(service,id).map_err(|e|api_error("Could not access Windows Credential Manager",e))
}
#[tauri::command]
fn save_database_credential(engine:String,profileId:String,password:String)->Result<(),String>{
 if password.is_empty()||password.len()>4096{return Err("Password must contain between 1 and 4096 bytes.".into());}
 database_credential_entry(&engine,&profileId)?.set_password(&password).map_err(|e|api_error("Could not save database credential in Windows Credential Manager",e))
}
#[tauri::command]
fn get_database_credential(engine:String,profileId:String)->Result<String,String>{
 database_credential_entry(&engine,&profileId)?.get_password().map_err(|e|api_error("No saved password is available in Windows Credential Manager",e))
}
#[tauri::command]
fn delete_database_credential(engine:String,profileId:String)->Result<(),String>{
 database_credential_entry(&engine,&profileId)?.delete_credential().map_err(|e|api_error("Could not delete database credential from Windows Credential Manager",e))
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
async fn runtime_preflight(token:String,id:String)->Result<Value,String>{
 let path=format!("/api/v1/websites/{id}/runtime-preflight");
 authorized_request(&token,reqwest::Method::GET,&path,None).await?.json().await.map_err(|e|api_error("Could not check website runtime prerequisites",e))
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
async fn export_database(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/databases/{id}/export"),None).await?.json().await.map_err(|e|api_error("Could not export SQLite database",e))}
#[tauri::command]
async fn import_database(token:String,name:String,content_base64:String)->Result<Value,String>{
 let body=serde_json::json!({"name":name,"content_base64":content_base64});
 authorized_request(&token,reqwest::Method::POST,"/api/v1/databases/import",Some(body)).await?.json().await.map_err(|e|api_error("Could not import SQLite database",e))
}
#[tauri::command]
async fn delete_database(token:String,id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/databases/{id}"),None).await?;Ok(())}
#[tauri::command]
async fn test_database(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/databases/{id}/test"),None).await?.json().await.map_err(|e|api_error("Could not test database",e))}
#[tauri::command]
async fn sqlite_database_connection_url(token:String,id:String)->Result<String,String>{let value:Value=authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/databases/{id}/connection"),None).await?.json().await.map_err(|e|api_error("Could not get SQLite connection URL",e))?;value.get("database_url").and_then(Value::as_str).map(str::to_string).ok_or_else(||"Core Service returned no SQLite connection URL.".into())}
#[tauri::command]
async fn query_database(token:String,id:String,sql:String)->Result<Value,String>{
 let body=serde_json::json!({"sql":sql});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/databases/{id}/query"),Some(body)).await?.json().await.map_err(|e|api_error("SQLite query failed",e))
}
#[tauri::command]
async fn list_mysql_connections(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/mysql-connections",None).await?.json().await.map_err(|e|api_error("Could not list MySQL/MariaDB profiles",e))}
#[tauri::command]
async fn create_mysql_connection(token:String,name:String,host:String,port:u16,databaseName:String,username:String)->Result<Value,String>{
 let body=serde_json::json!({"name":name,"host":host,"port":port,"database_name":databaseName,"username":username});
 authorized_request(&token,reqwest::Method::POST,"/api/v1/mysql-connections",Some(body)).await?.json().await.map_err(|e|api_error("Could not create MySQL/MariaDB profile",e))
}
#[tauri::command]
async fn delete_mysql_connection(token:String,id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/mysql-connections/{id}"),None).await?;Ok(())}
#[tauri::command]
async fn test_mysql_connection(token:String,id:String,password:String)->Result<Value,String>{
 let body=serde_json::json!({"password":password});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/mysql-connections/{id}/test"),Some(body)).await?.json().await.map_err(|e|api_error("MySQL/MariaDB connection test failed",e))
}
#[tauri::command]
async fn list_postgres_connections(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/postgres-connections",None).await?.json().await.map_err(|e|api_error("Could not list PostgreSQL profiles",e))}
#[tauri::command]
async fn create_postgres_connection(token:String,name:String,host:String,port:u16,databaseName:String,username:String)->Result<Value,String>{
 let body=serde_json::json!({"name":name,"host":host,"port":port,"database_name":databaseName,"username":username});
 authorized_request(&token,reqwest::Method::POST,"/api/v1/postgres-connections",Some(body)).await?.json().await.map_err(|e|api_error("Could not create PostgreSQL profile",e))
}
#[tauri::command]
async fn delete_postgres_connection(token:String,id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/postgres-connections/{id}"),None).await?;Ok(())}
#[tauri::command]
async fn test_postgres_connection(token:String,id:String,password:String)->Result<Value,String>{
 let body=serde_json::json!({"password":password});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/postgres-connections/{id}/test"),Some(body)).await?.json().await.map_err(|e|api_error("PostgreSQL connection test failed",e))
}
#[tauri::command]
async fn provision_postgres_database(token:String,id:String,password:String,name:String,owner:Option<String>)->Result<Value,String>{
 let body=serde_json::json!({"password":password,"name":name,"owner":owner});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/postgres-connections/{id}/provision/database"),Some(body)).await?.json().await.map_err(|e|api_error("PostgreSQL database provisioning failed",e))
}
#[tauri::command]
async fn provision_postgres_role(token:String,id:String,password:String,name:String,rolePassword:String,databaseName:Option<String>)->Result<Value,String>{
 let body=serde_json::json!({"password":password,"name":name,"role_password":rolePassword,"database_name":databaseName});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/postgres-connections/{id}/provision/role"),Some(body)).await?.json().await.map_err(|e|api_error("PostgreSQL role provisioning failed",e))
}
#[tauri::command]
async fn provision_mysql_database(token:String,id:String,password:String,name:String)->Result<Value,String>{
 let body=serde_json::json!({"password":password,"name":name});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/mysql-connections/{id}/provision/database"),Some(body)).await?.json().await.map_err(|e|api_error("MySQL/MariaDB database provisioning failed",e))
}
#[tauri::command]
async fn provision_mysql_user(token:String,id:String,password:String,name:String,userPassword:String,databaseName:Option<String>)->Result<Value,String>{
 let body=serde_json::json!({"password":password,"name":name,"user_password":userPassword,"database_name":databaseName});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/mysql-connections/{id}/provision/user"),Some(body)).await?.json().await.map_err(|e|api_error("MySQL/MariaDB user provisioning failed",e))
}
#[tauri::command]
async fn query_mysql_connection(token:String,id:String,password:String,sql:String)->Result<Value,String>{
 let body=serde_json::json!({"password":password,"sql":sql});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/mysql-connections/{id}/query"),Some(body)).await?.json().await.map_err(|e|api_error("MySQL/MariaDB query failed",e))
}
#[tauri::command]
async fn query_postgres_connection(token:String,id:String,password:String,sql:String)->Result<Value,String>{
 let body=serde_json::json!({"password":password,"sql":sql});
 authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/postgres-connections/{id}/query"),Some(body)).await?.json().await.map_err(|e|api_error("PostgreSQL query failed",e))
}
#[tauri::command]
async fn monitoring_metrics(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/monitoring/metrics",None).await?.json().await.map_err(|e|api_error("Could not read system metrics",e))}
#[tauri::command]
async fn website_health_check(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/websites/{id}/health-check"),None).await?.json().await.map_err(|e|api_error("Website health check failed",e))}
#[tauri::command]
async fn website_health_history(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/websites/{id}/health-history"),None).await?.json().await.map_err(|e|api_error("Could not read website health history",e))}
#[tauri::command]
async fn health_report(token:String,days:u32,website_id:Option<String>)->Result<Value,String>{
 let mut path=format!("/api/v1/health-reports?days={days}");
 if let Some(id)=website_id.filter(|value|!value.is_empty()){path.push_str("&website_id=");path.push_str(&encode_query_value(&id));}
 authorized_request(&token,reqwest::Method::GET,&path,None).await?.json().await.map_err(|e|api_error("Could not read website uptime report",e))
}
#[tauri::command]
async fn website_health_alerts(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/health-alerts",None).await?.json().await.map_err(|e|api_error("Could not read website health alerts",e))}
#[tauri::command]
async fn acknowledge_website_health_alert(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/health-alerts/{id}/acknowledge"),None).await?.json().await.map_err(|e|api_error("Could not acknowledge website health alert",e))}
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
async fn proxy_tls_details(token:String,hostname:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/proxy/tls-details/{}",encode_query_value(&hostname)),None).await?.json().await.map_err(|e|api_error("Certificate inspection failed",e))}
#[tauri::command]
async fn proxy_tls_history(token:String,hostname:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/proxy/tls-history/{}",encode_query_value(&hostname)),None).await?.json().await.map_err(|e|api_error("Could not load certificate history",e))}
#[tauri::command]
async fn proxy_tls_monitoring(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/proxy/tls-monitoring",None).await?.json().await.map_err(|e|api_error("Could not load TLS monitoring settings",e))}
#[tauri::command]
async fn save_proxy_tls_monitoring(token:String,enabled:bool,interval_hours:i64,warning_days:i64)->Result<Value,String>{let body=serde_json::json!({"enabled":enabled,"interval_hours":interval_hours,"warning_days":warning_days});authorized_request(&token,reqwest::Method::PUT,"/api/v1/proxy/tls-monitoring",Some(body)).await?.json().await.map_err(|e|api_error("Could not save TLS monitoring settings",e))}
#[tauri::command]
async fn proxy_tls_alerts(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/proxy/tls-alerts",None).await?.json().await.map_err(|e|api_error("Could not load TLS alerts",e))}
#[tauri::command]
async fn acknowledge_proxy_tls_alert(token:String,id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/proxy/tls-alerts/{id}/acknowledge"),None).await?.json().await.map_err(|e|api_error("Could not acknowledge TLS alert",e))}
#[tauri::command]
async fn hosting_recovery_events(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/hosting/recovery-events",None).await?.json().await.map_err(|e|api_error("Could not load hosting recovery history",e))}

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
async fn preview_website_folder_delete(token:String,id:String,path:String)->Result<Value,String>{
 let url_path=format!("/api/v1/websites/{id}/files/delete-preview?path={}",encode_query_value(&path));
 authorized_request(&token,reqwest::Method::GET,&url_path,None).await?.json().await.map_err(|e|api_error("Could not preview folder deletion",e))
}
#[tauri::command]
async fn delete_website_file(token:String,id:String,path:String,recursive:Option<bool>)->Result<(),String>{
 let mut url_path=format!("/api/v1/websites/{id}/file?path={}",encode_query_value(&path));
 if recursive.unwrap_or(false){url_path.push_str("&recursive=true");}
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
fn encode_query_component(value:&str)->String{
 let mut encoded=String::new();
 for byte in value.bytes(){if byte.is_ascii_alphanumeric()||matches!(byte,b'-'|b'_'|b'.'|b'~'){encoded.push(byte as char);}else{encoded.push_str(&format!("%{byte:02X}"));}}
 encoded
}
#[tauri::command]
async fn list_build_runs(token:String,website_id:String,limit:u32,offset:u32,status:String,search:String)->Result<Vec<Value>,String>{
 let mut parts=vec![format!("limit={limit}"),format!("offset={offset}"),format!("status={}",encode_query_component(&status)),format!("search={}",encode_query_component(&search))];
 if !website_id.is_empty(){parts.push(format!("website_id={}",encode_query_component(&website_id)));}
 let suffix=format!("?{}",parts.join("&"));
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
async fn delete_deployment_release(token:String,website_id:String,release_id:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/deployments/{website_id}/releases/{release_id}"),None).await?.json().await.map_err(|e|api_error("Could not delete release snapshot",e))}


#[tauri::command]
async fn list_tunnels(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/tunnels",None).await?.json().await.map_err(|e|api_error("Could not read tunnel status",e))}
#[tauri::command]
async fn tunnel_action(token:String,website_id:String,action:String)->Result<Value,String>{
 if action=="start"{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/tunnels/{website_id}/start"),None).await?.json().await.map_err(|e|api_error("Could not start tunnel",e))}
 else if action=="stop"{authorized_request(&token,reqwest::Method::POST,&format!("/api/v1/tunnels/{website_id}/stop"),None).await?;Ok(serde_json::json!({"stopped":true}))}
 else{Err("Unsupported tunnel action.".into())}
}
#[tauri::command]
async fn backup_storage_summary(token:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/backups/storage",None).await?.json().await.map_err(|e|api_error("Could not read backup storage summary",e))}
#[tauri::command]
async fn preview_backup_retention(token:String,retention_count:i64)->Result<Value,String>{authorized_request(&token,reqwest::Method::GET,&format!("/api/v1/backups/retention-preview?count={retention_count}"),None).await?.json().await.map_err(|e|api_error("Could not preview backup retention",e))}
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
async fn list_website_environment(token:String,id:String)->Result<Vec<Value>,String>{
 let path=format!("/api/v1/websites/{id}/environment");
 authorized_request(&token,reqwest::Method::GET,&path,None).await?.json().await.map_err(|e|api_error("Could not read website environment variables",e))
}
#[tauri::command]
async fn save_website_environment(token:String,id:String,name:String,value:String,is_secret:bool)->Result<Value,String>{
 let path=format!("/api/v1/websites/{id}/environment");
 let body=serde_json::json!({"name":name,"value":value,"is_secret":is_secret});
 authorized_request(&token,reqwest::Method::PUT,&path,Some(body)).await?.json().await.map_err(|e|api_error("Could not save website environment variable",e))
}
#[tauri::command]
async fn save_website_environment_batch(token:String,id:String,variables:Vec<Value>)->Result<Vec<Value>,String>{
 let path=format!("/api/v1/websites/{id}/environment/batch");
 let body=serde_json::json!({"variables":variables});
 authorized_request(&token,reqwest::Method::PUT,&path,Some(body)).await?.json().await.map_err(|e|api_error("Could not save website environment batch",e))
}

#[tauri::command]
async fn delete_website_environment(token:String,id:String,name:String)->Result<(),String>{
 let path=format!("/api/v1/websites/{id}/environment/{name}");
 authorized_request(&token,reqwest::Method::DELETE,&path,None).await?;Ok(())
}

#[tauri::command]
async fn delete_website_environment_batch(token:String,id:String,names:Vec<String>)->Result<Value,String>{
 let path=format!("/api/v1/websites/{id}/environment/batch");
 let body=serde_json::json!({"names":names});
 authorized_request(&token,reqwest::Method::DELETE,&path,Some(body)).await?.json().await.map_err(|e|api_error("Could not remove website environment batch",e))
}

#[tauri::command]
async fn list_runtime_profiles(token:String)->Result<Vec<Value>,String>{authorized_request(&token,reqwest::Method::GET,"/api/v1/runtime-profiles",None).await?.json().await.map_err(|e|api_error("Could not read runtime profiles",e))}
#[tauri::command]
async fn save_runtime_profile(token:String,website_id:String,program:String,args:Vec<String>,health_path:String)->Result<Value,String>{authorized_request(&token,reqwest::Method::PUT,"/api/v1/runtime-profiles",Some(serde_json::json!({"website_id":website_id,"program":program,"args":args,"health_path":health_path}))).await?.json().await.map_err(|e|api_error("Could not save runtime profile",e))}
#[tauri::command]
async fn delete_runtime_profile(token:String,website_id:String)->Result<(),String>{authorized_request(&token,reqwest::Method::DELETE,&format!("/api/v1/runtime-profiles/{website_id}"),None).await?;Ok(())}


/// Resolve the publisher pin injected by the trusted release build. It is deliberately
/// not accepted from the webview, user settings, environment variables, or CLI.
fn trusted_core_publisher_pin() -> Result<&'static str, String> {
    option_env!("WEBDYNAMITE_PUBLISHER_SHA256")
        .filter(|value| value.len() == 64
            && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            && !value.bytes().all(|byte| byte == b'0'))
        .ok_or_else(|| "Authenticated IPC is disabled: this build has no valid trusted Core Service publisher pin.".into())
}

/// Resolve the service SID injected by the trusted Windows release build.
/// It is never accepted from the webview, environment variables, or user settings.
fn trusted_core_service_sid() -> Result<&'static str, String> {
    option_env!("WEBDYNAMITE_SERVICE_SID")
        .filter(|value| {
            let parts = value.trim().split('-').collect::<Vec<_>>();
            parts.len() == 9 && parts[0] == "S" && parts[1] == "1"
                && parts[2] == "5" && parts[3] == "80"
                && parts[4..].iter().all(|part| !part.is_empty() && part.parse::<u32>().is_ok())
        })
        .ok_or_else(|| "Authenticated IPC is disabled: this build has no trusted Core Service service SID.".into())
}

fn authenticated_secret_ipc(app: &tauri::AppHandle, operation: &str, data: Value) -> Result<Value, String> {
    let executable = bundled_core_service_path(app)?;
    let pin = trusted_core_publisher_pin()?;
    let service_sid = trusted_core_service_sid()?;
    let response = named_pipe_client::exchange(operation, data, &executable, pin, service_sid)?;
    if !response.ok {
        return Err(format!(
            "Authenticated Core Service operation failed: {}",
            response.error_code.as_deref().unwrap_or("unknown_error")
        ));
    }
    response.data.ok_or_else(|| "Authenticated Core Service returned no operation data.".into())
}

fn trusted_ipc_configuration_state(pin: Option<&str>, service_sid: Option<&str>) -> Result<bool, String> {
    match (pin, service_sid) {
        (None, None) => Ok(false),
        (Some(pin), Some(service_sid)) => {
            if pin.len() != 64 || !pin.bytes().all(|byte| byte.is_ascii_hexdigit())
                || pin.bytes().all(|byte| byte == b'0')
            {
                return Err("The trusted Core Service publisher pin must be a nonzero 64-character SHA-256 hex value.".into());
            }
            let parts = service_sid.trim().split('-').collect::<Vec<_>>();
            if parts.len() != 9 || parts[0] != "S" || parts[1] != "1"
                || parts[2] != "5" || parts[3] != "80"
                || parts[4..].iter().any(|part| part.is_empty() || part.parse::<u32>().is_err())
            {
                return Err("The trusted Core Service service SID is invalid.".into());
            }
            Ok(true)
        }
        _ => Err("Trusted Core Service IPC requires both WEBDYNAMITE_PUBLISHER_SHA256 and WEBDYNAMITE_SERVICE_SID at build time.".into()),
    }
}

/// A release build is explicitly configured for both trusted IPC pins, or is
/// an engineering build with neither. Partial/invalid configuration fails closed.
fn trusted_ipc_configuration_present() -> Result<bool, String> {
    let present = trusted_ipc_configuration_state(
        option_env!("WEBDYNAMITE_PUBLISHER_SHA256"),
        option_env!("WEBDYNAMITE_SERVICE_SID"),
    )?;
    if present {
        trusted_core_publisher_pin()?;
        trusted_core_service_sid()?;
    }
    Ok(present)
}

fn extract_service_token(data: &mut Value) -> Result<String, String> {
    let Some(Value::String(stored_token)) = data.get_mut("token") else {
        return Err("Authenticated Core Service returned no API token.".into());
    };
    let mut token = stored_token.clone();
    stored_token.zeroize();
    if token.trim().len() < 32 {
        token.zeroize();
        return Err("Authenticated Core Service returned an invalid API token.".into());
    }
    Ok(token)
}

fn load_or_create_core_service_token(app: &tauri::AppHandle) -> Result<String, String> {
    if !trusted_ipc_configuration_present()? {
        return load_or_create_legacy_core_service_token();
    }

    let mut response = authenticated_secret_ipc(app, "service.auth-token", serde_json::json!({}))?;
    let token = extract_service_token(&mut response)?;
    // Refresh the existing desktop credential cache only after the service
    // proves its identity and returns the provisioned machine-DPAPI token.
    core_service_token_entry()?.set_password(&token)
        .map_err(|error| api_error("Could not cache the authenticated Core Service API token", error))?;
    Ok(token)
}

#[tauri::command]
fn ipc_secret_status(app: tauri::AppHandle) -> Result<Value, String> {
    authenticated_secret_ipc(&app, "secret.status", serde_json::json!({}))
}

#[tauri::command]
fn ipc_secret_put(app: tauri::AppHandle, website_id: String, name: String, value: String) -> Result<Value, String> {
    if value.is_empty() || value.len() > 8192 || value.contains('\0') {
        return Err("Secret value must contain between 1 and 8192 bytes and no NUL characters.".into());
    }
    authenticated_secret_ipc(
        &app,
        "secret.put",
        serde_json::json!({"website_id": website_id, "name": name, "value": value}),
    )
}

#[tauri::command]
fn ipc_secret_delete(app: tauri::AppHandle, website_id: String, name: String) -> Result<Value, String> {
    authenticated_secret_ipc(
        &app,
        "secret.delete",
        serde_json::json!({"website_id": website_id, "name": name}),
    )
}


#[tauri::command]
async fn migrate_website_environment_secrets(app: tauri::AppHandle, token: String, id: String) -> Result<Value, String> {
    if !trusted_ipc_configuration_present()? {
        return Ok(serde_json::json!({"supported": false, "migrated": 0, "already_migrated": 0}));
    }
    let path = format!("/api/v1/websites/{id}/environment");
    let rows: Vec<Value> = authorized_request(&token, reqwest::Method::GET, &path, None)
        .await?.json().await.map_err(|error| api_error("Could not inspect website environment metadata", error))?;
    let mut migrated = 0usize;
    let mut already_migrated = 0usize;
    for row in rows.into_iter().filter(|row| row.get("is_secret").and_then(Value::as_bool) == Some(true)) {
        let name = row.get("name").and_then(Value::as_str)
            .ok_or_else(|| "Website environment metadata contained an invalid secret name.".to_string())?.to_string();
        let status = authenticated_secret_ipc(&app, "secret.verify", serde_json::json!({"website_id": id, "name": name}))?;
        let entry = Entry::new("WebDynamite", &format!("website-env:{id}:{name}"))
            .map_err(|error| api_error("Could not open the legacy website credential", error))?;
        if status.get("exists").and_then(Value::as_bool) == Some(true) {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => already_migrated += 1,
                Err(error) => return Err(api_error("The protected secret already exists, but the stale legacy credential could not be removed", error)),
            }
            continue;
        }
        let mut value = entry.get_password()
            .map_err(|error| match error {
                keyring::Error::NoEntry => format!("Secret {name} is missing from both the legacy credential store and the protected service store; no source was deleted."),
                other => api_error("Could not read the legacy website credential", other),
            })?;
        let mut payload = serde_json::json!({"website_id": id, "name": name, "value": value});
        let stored = authenticated_secret_ipc(&app, "secret.put", payload.clone());
        if let Some(Value::String(secret)) = payload.get_mut("value") { secret.zeroize(); }
        value.zeroize();
        let stored = stored?;
        if stored.get("stored").and_then(Value::as_bool) != Some(true)
            || stored.get("verified").and_then(Value::as_bool) != Some(true) {
            return Err(format!("Protected-store verification did not succeed for {name}; the legacy credential was retained."));
        }
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => migrated += 1,
            Err(error) => return Err(api_error("The protected copy was verified, but the legacy credential could not be removed", error)),
        }
    }
    Ok(serde_json::json!({"supported": true, "migrated": migrated, "already_migrated": already_migrated}))
}

#[cfg_attr(mobile,tauri::mobile_entry_point)]
pub fn run(){
 let app = tauri::Builder::default()
 .setup(|app| {
  start_core_service(app.handle())?;
  Ok(())
 })
 .invoke_handler(tauri::generate_handler![core_service_token,save_database_credential,get_database_credential,delete_database_credential,core_health,list_websites,create_website,website_action,runtime_preflight,website_logs,delete_website,list_domains,create_domain,delete_domain,diagnose_domain,list_public_domains,create_public_domain,delete_public_domain,export_caddyfile,proxy_status,proxy_tls_check,proxy_tls_details,proxy_tls_history,proxy_tls_monitoring,save_proxy_tls_monitoring,proxy_tls_alerts,acknowledge_proxy_tls_alert,hosting_recovery_events,proxy_action,networking_status,monitoring_metrics,website_health_check,website_health_history,health_report,website_health_alerts,acknowledge_website_health_alert,set_lan_access,list_databases,create_database,delete_database,test_database,sqlite_database_connection_url,query_database,list_mysql_connections,create_mysql_connection,delete_mysql_connection,test_mysql_connection,provision_mysql_database,provision_mysql_user,query_mysql_connection,list_postgres_connections,create_postgres_connection,delete_postgres_connection,test_postgres_connection,provision_postgres_database,provision_postgres_role,query_postgres_connection,export_database,import_database,website_storage_usage,list_website_files,read_website_file,write_website_file,upload_website_file,download_website_file,download_website_archive,extract_website_archive,create_website_directory,rename_website_file,preview_website_folder_delete,delete_website_file,list_runtime_profiles,save_runtime_profile,delete_runtime_profile,list_website_environment,save_website_environment,save_website_environment_batch,delete_website_environment,delete_website_environment_batch,list_build_profiles,save_build_profile,run_build,deploy_website,list_build_runs,git_repository_status,git_fetch,git_pull,list_deployment_releases,create_deployment_release,rollback_deployment_release,delete_deployment_release,list_tunnels,tunnel_action,backup_storage_summary,preview_backup_retention,get_backup_schedule,save_backup_schedule,list_backups,create_backup,verify_backup,preview_backup_restore,restore_backup,delete_backup,get_app_settings,save_app_settings,diagnostics_report,recover_website_states,ipc_secret_status,ipc_secret_put,ipc_secret_delete,migrate_website_environment_secrets])
 .build(tauri::generate_context!())
 .expect("error while building WebDynamite");
 app.run(|_, _| {});
}


#[cfg(test)]
mod core_service_lifecycle_tests {
    use super::*;

    #[test]
    fn authenticated_reuse_requires_success_status() {
        assert!(is_success_status_line("HTTP/1.1 200 OK"));
        assert!(is_success_status_line("HTTP/1.0 200 OK"));
        assert!(!is_success_status_line("HTTP/1.1 401 Unauthorized"));
        assert!(!is_success_status_line("HTTP/1.1 404 Not Found"));
    }

    #[test]
    fn trusted_ipc_requires_both_valid_pins_or_neither() {
        let pin = "ab".repeat(32);
        let sid = "S-1-5-80-1-2-3-4-5";
        assert!(!trusted_ipc_configuration_state(None, None).unwrap());
        assert!(trusted_ipc_configuration_state(Some(&pin), Some(sid)).unwrap());
        assert!(trusted_ipc_configuration_state(Some(&pin), None).is_err());
        assert!(trusted_ipc_configuration_state(None, Some(sid)).is_err());
        assert!(trusted_ipc_configuration_state(Some(&"0".repeat(64)), Some(sid)).is_err());
        assert!(trusted_ipc_configuration_state(Some("bad-pin"), Some(sid)).is_err());
        assert!(trusted_ipc_configuration_state(Some(&pin), Some("S-1-5-21-1-2-3-4")).is_err());
    }

    #[test]
    fn service_token_extraction_wipes_response_copy_and_rejects_short_tokens() {
        let mut response = serde_json::json!({"token": "a".repeat(64)});
        assert_eq!(extract_service_token(&mut response).unwrap(), "a".repeat(64));
        assert_eq!(response["token"], "");
        let mut invalid = serde_json::json!({"token": "short"});
        assert!(extract_service_token(&mut invalid).is_err());
        assert_eq!(invalid["token"], "");
        let mut missing = serde_json::json!({"other": true});
        assert!(extract_service_token(&mut missing).is_err());
    }
}

fn is_success_status_line(status_line: &str) -> bool {
    status_line.starts_with("HTTP/1.1 200 ") || status_line.starts_with("HTTP/1.0 200 ")
}
