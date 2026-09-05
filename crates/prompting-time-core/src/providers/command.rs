use std::env;
use std::ffi::OsStr;
use std::path::PathBuf;

use tokio::process::Command;

/// Builds a provider command with desktop CLI/runtime locations added to its child-only PATH.
pub fn provider_command(binary: impl AsRef<OsStr>) -> Command {
    command_with_environment(
        binary,
        env::var_os("PATH").as_deref(),
        env::var_os("HOME").as_deref(),
    )
}

fn command_with_environment(
    binary: impl AsRef<OsStr>,
    inherited_path: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Command {
    let candidates = env::split_paths(inherited_path.unwrap_or_default())
        .chain(home.map(|home| PathBuf::from(home).join(".local/bin")))
        .chain(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/sbin",
            ]
            .map(PathBuf::from),
        );
    let mut paths = Vec::new();
    for path in candidates {
        // Never resolve PATH entries against a conversation's working directory. HOME may also
        // contain a separator that cannot be represented as a single PATH entry.
        if path.is_absolute() && env::join_paths([&path]).is_ok() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    let mut command = Command::new(binary);
    command.env(
        "PATH",
        env::join_paths(paths).expect("PATH entries were validated"),
    );
    command
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::*;

    fn child_path(command: &Command) -> Vec<PathBuf> {
        let path = command
            .as_std()
            .get_envs()
            .find_map(|(key, value)| (key == "PATH").then_some(value))
            .flatten()
            .unwrap();
        env::split_paths(path).collect()
    }

    fn executable(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn inherited_absolute_paths_keep_precedence_and_are_deduplicated() {
        let command = command_with_environment(
            "provider",
            Some(OsStr::new(
                "/custom tools/bin:/usr/bin:/custom tools/bin:/opt/homebrew/bin",
            )),
            Some(OsStr::new("/example home")),
        );
        assert_eq!(
            child_path(&command),
            [
                "/custom tools/bin",
                "/usr/bin",
                "/opt/homebrew/bin",
                "/example home/.local/bin",
                "/usr/local/bin",
                "/bin",
                "/usr/sbin",
                "/sbin",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn empty_relative_entries_and_invalid_home_are_excluded() {
        for home in [None, Some(""), Some("relative home"), Some("/invalid:home")] {
            let command = command_with_environment(
                "./explicit-provider",
                Some(OsStr::new(":relative/bin:.:/usr/bin::../bin")),
                home.map(OsStr::new),
            );
            assert_eq!(
                child_path(&command),
                [
                    "/usr/bin",
                    "/opt/homebrew/bin",
                    "/usr/local/bin",
                    "/bin",
                    "/usr/sbin",
                    "/sbin"
                ]
                .map(PathBuf::from)
            );
            assert_eq!(command.as_std().get_program(), "./explicit-provider");
        }
    }

    #[tokio::test]
    async fn minimal_path_discovers_home_cli_with_spaces() {
        let directory = tempfile::Builder::new()
            .prefix("provider home ")
            .tempdir()
            .unwrap();
        executable(
            &directory
                .path()
                .join(".local/bin/prompting-time-test-provider"),
            "#!/bin/sh\nprintf 'home-provider\\n'\n",
        );
        let output = command_with_environment(
            "prompting-time-test-provider",
            Some(OsStr::new("/usr/bin:/bin")),
            Some(directory.path().as_os_str()),
        )
        .output()
        .await
        .expect("home CLI should be discoverable");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"home-provider\n");
    }

    #[tokio::test]
    async fn absolute_cli_finds_shebang_runtime_in_child_path() {
        let directory = tempfile::Builder::new()
            .prefix("runtime home ")
            .tempdir()
            .unwrap();
        let binary = directory.path().join("separate installation/provider");
        executable(&binary, "#!/usr/bin/env prompting-time-test-runtime\n");
        executable(
            &directory
                .path()
                .join(".local/bin/prompting-time-test-runtime"),
            "#!/bin/sh\nprintf 'runtime:%s\\n' \"$2\"\n",
        );
        let output = command_with_environment(
            &binary,
            Some(OsStr::new("/usr/bin:/bin")),
            Some(directory.path().as_os_str()),
        )
        .arg("argument with spaces")
        .output()
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"runtime:argument with spaces\n");
    }

    #[tokio::test]
    async fn inherited_installation_wins_over_home_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let preferred = directory.path().join("preferred tools");
        let name = "prompting-time-test-provider";
        executable(&preferred.join(name), "#!/bin/sh\nprintf 'preferred\\n'\n");
        executable(
            &directory.path().join(".local/bin").join(name),
            "#!/bin/sh\nprintf 'fallback\\n'\n",
        );
        let output = command_with_environment(
            name,
            Some(preferred.as_os_str()),
            Some(directory.path().as_os_str()),
        )
        .output()
        .await
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"preferred\n");
    }

    #[tokio::test]
    async fn missing_executable_remains_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let error = command_with_environment(
            "prompting-time-test-provider-not-installed",
            None,
            Some(directory.path().as_os_str()),
        )
        .output()
        .await
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
    }
}
