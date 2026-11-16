#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![health_check])
        .run(tauri::generate_context!())
        .expect("error while running WebDynamite");
}

#[tauri::command]
fn health_check() -> &'static str {
    "desktop-shell-ready"
}
