use std::process::Output;
use std::time::Duration;

use thiserror::Error;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Codex,
    Claude,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallation {
    pub id: ProviderId,
    pub installed: bool,
    pub version: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("{binary} is not installed")]
    NotInstalled { binary: String, diagnostic: String },
    #[error("{binary} did not return a version within five seconds")]
    TimedOut { binary: String, diagnostic: String },
    #[error("{binary} could not be inspected")]
    InspectionFailed { binary: String, diagnostic: String },
}

impl ProviderError {
    pub fn into_installation(self, id: ProviderId) -> ProviderInstallation {
        let diagnostic = match self {
            Self::NotInstalled { diagnostic, .. }
            | Self::TimedOut { diagnostic, .. }
            | Self::InspectionFailed { diagnostic, .. } => diagnostic,
        };

        ProviderInstallation {
            id,
            installed: false,
            version: None,
            diagnostic: Some(diagnostic),
        }
    }
}

pub async fn discover_provider(
    binary: &str,
    id: ProviderId,
) -> Result<ProviderInstallation, ProviderError> {
    let mut command = Command::new(binary);
    command.arg("--version").kill_on_drop(true);

    let output = timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| ProviderError::TimedOut {
            binary: binary.to_owned(),
            diagnostic: "The version command timed out after five seconds.".to_owned(),
        })?
        .map_err(|error| {
            let diagnostic = sanitize_diagnostic(&error.to_string());

            if error.kind() == std::io::ErrorKind::NotFound {
                ProviderError::NotInstalled {
                    binary: binary.to_owned(),
                    diagnostic,
                }
            } else {
                ProviderError::InspectionFailed {
                    binary: binary.to_owned(),
                    diagnostic,
                }
            }
        })?;

    if !output.status.success() {
        return Err(ProviderError::InspectionFailed {
            binary: binary.to_owned(),
            diagnostic: output_diagnostic(&output),
        });
    }

    let version = first_version_line(&output.stdout).or_else(|| first_version_line(&output.stderr));
    match version {
        Some(version) => Ok(ProviderInstallation {
            id,
            installed: true,
            version: Some(version),
            diagnostic: None,
        }),
        None => Err(ProviderError::InspectionFailed {
            binary: binary.to_owned(),
            diagnostic: output_diagnostic(&output),
        }),
    }
}

fn first_version_line(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.to_ascii_lowercase().starts_with("warning"))
        .map(version_from_line)
}

fn version_from_line(line: &str) -> String {
    line.split_whitespace()
        .find(|word| word.chars().any(|character| character.is_ascii_digit()))
        .unwrap_or(line)
        .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '.')
        .to_owned()
}

fn output_diagnostic(output: &Output) -> String {
    let stderr = sanitize_diagnostic(&String::from_utf8_lossy(&output.stderr));
    if stderr.is_empty() {
        sanitize_diagnostic(&String::from_utf8_lossy(&output.stdout))
    } else {
        stderr
    }
}

fn sanitize_diagnostic(diagnostic: &str) -> String {
    diagnostic
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[tokio::test]
    async fn missing_binary_is_reported_without_panicking() {
        let result = discover_provider("definitely-not-installed", ProviderId::Codex).await;
        assert!(matches!(result, Err(ProviderError::NotInstalled { .. })));
    }

    #[tokio::test]
    async fn timed_out_provider_is_terminated() {
        let directory = std::env::temp_dir().join(format!(
            "prompting-time-provider-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after the Unix epoch")
                .as_nanos()
        ));
        let marker = directory.join("still-running");
        let binary = directory.join("hanging-provider");
        fs::create_dir_all(&directory).expect("test directory should be created");
        fs::write(
            &binary,
            format!("#!/bin/sh\nsleep 6\ntouch '{}'\n", marker.display()),
        )
        .expect("test provider should be written");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
            .expect("test provider should be executable");

        let result = discover_provider(
            binary.to_str().expect("test path should be UTF-8"),
            ProviderId::Codex,
        )
        .await;

        assert!(matches!(result, Err(ProviderError::TimedOut { .. })));
        tokio::time::sleep(Duration::from_secs(2)).await;
        let process_continued = marker.exists();
        fs::remove_dir_all(directory).expect("test directory should be removed");
        assert!(!process_continued, "timed-out provider process continued");
    }
}
