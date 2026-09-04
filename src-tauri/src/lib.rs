pub mod commands;
pub mod state;

use std::sync::Arc;

use state::{AppState, StartupDiagnostic};
use tauri::Manager;

pub fn run() {
    let command_builder = commands::binding_builder();
    let app = tauri::Builder::default()
        .invoke_handler(command_builder.invoke_handler())
        .setup(|app| {
            let state = match app.path().app_data_dir() {
                Ok(app_data_dir) => {
                    tauri::async_runtime::block_on(AppState::initialize(app_data_dir))
                }
                Err(_) => AppState::failed(StartupDiagnostic {
                    code: "storage-error",
                    message: "Prompting Time could not resolve its Application Support directory."
                        .to_owned(),
                    action: Some(
                        "Restart Prompting Time and verify macOS storage access.".to_owned(),
                    ),
                }),
            };
            state.start_event_forwarding(app.handle().clone());
            app.manage(state);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Prompting Time");

    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let state = Arc::clone(app.state::<Arc<AppState>>().inner());
            tauri::async_runtime::block_on(state.shutdown());
        }
    });
}
