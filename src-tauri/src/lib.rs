pub mod commands;
pub mod state;

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![commands::bootstrap])
        .run(tauri::generate_context!())
        .expect("error while running Prompting Time");
}
