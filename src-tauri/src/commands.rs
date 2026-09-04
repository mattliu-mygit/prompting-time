use prompting_time_core::providers::{ProviderId, ProviderInstallation, discover_provider};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapSnapshot {
    pub providers: Vec<ProviderInstallation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub message: String,
}

#[tauri::command]
pub async fn bootstrap() -> Result<BootstrapSnapshot, CommandError> {
    Ok(BootstrapSnapshot {
        providers: vec![
            provider_installation("codex", ProviderId::Codex).await,
            provider_installation("claude", ProviderId::Claude).await,
        ],
    })
}

async fn provider_installation(binary: &str, id: ProviderId) -> ProviderInstallation {
    discover_provider(binary, id)
        .await
        .unwrap_or_else(|error| error.into_installation(id))
}
