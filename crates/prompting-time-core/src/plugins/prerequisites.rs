//! Prerequisite checks: whether each program a plugin's manifest names is on the PATH plugin
//! commands get, and whether the plugin's settings need it.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::Value;

use super::manifest::{Prerequisite, SettingCondition};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrerequisiteStatus {
    pub command: String,
    pub purpose: String,
    /// False when the prerequisite's `only_when` does not hold for the current settings.
    pub needed: bool,
    /// Where PT found the program, or None when it is missing.
    pub path: Option<PathBuf>,
}

/// Checks each prerequisite against `path`. `settings` is what the plugin's get-settings action
/// printed; without it, a prerequisite with `only_when` counts as needed.
pub(crate) fn check(
    prerequisites: &[Prerequisite],
    settings: Option<&Value>,
    path: &OsStr,
) -> Vec<PrerequisiteStatus> {
    prerequisites
        .iter()
        .map(|prerequisite| PrerequisiteStatus {
            command: prerequisite.command.clone(),
            purpose: prerequisite.purpose.clone(),
            needed: match (&prerequisite.only_when, settings) {
                (Some(condition), Some(settings)) => condition_holds(condition, settings),
                _ => true,
            },
            path: find_on_path(&prerequisite.command, path),
        })
        .collect()
}

fn condition_holds(condition: &SettingCondition, settings: &Value) -> bool {
    settings
        .get(&condition.setting)
        .and_then(Value::as_object)
        .is_some_and(|object| {
            object
                .values()
                .any(|value| value.as_str() == Some(condition.value.as_str()))
        })
}

fn find_on_path(command: &str, path: &OsStr) -> Option<PathBuf> {
    env::split_paths(path)
        .map(|directory| directory.join(command))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prerequisite(command: &str, only_when: Option<(&str, &str)>) -> Prerequisite {
        Prerequisite {
            command: command.into(),
            purpose: format!("{command} does a job"),
            only_when: only_when.map(|(setting, value)| SettingCondition {
                setting: setting.into(),
                value: value.into(),
            }),
        }
    }

    #[test]
    fn programs_are_found_on_the_given_path_only() {
        let directory = tempfile::tempdir().unwrap();
        let tool = directory.path().join("fixture-tool");
        fs::write(&tool, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let unexecutable = directory.path().join("fixture-data");
        fs::write(&unexecutable, "").unwrap();
        let path = env::join_paths([
            "/nonexistent-fixture-dir".into(),
            directory.path().to_path_buf(),
        ])
        .unwrap();

        let statuses = check(
            &[
                prerequisite("fixture-tool", None),
                prerequisite("fixture-data", None),
                prerequisite("fixture-missing", None),
            ],
            None,
            &path,
        );

        assert_eq!(statuses[0].path.as_deref(), Some(tool.as_path()));
        assert_eq!(
            statuses[1].path, None,
            "a file without an execute bit is no program"
        );
        assert_eq!(statuses[2].path, None);
        assert!(statuses.iter().all(|status| status.needed));
        assert_eq!(statuses[0].purpose, "fixture-tool does a job");
    }

    #[test]
    fn only_when_reads_the_values_of_the_named_settings_object() {
        let settings = serde_json::json!({
            "jobs": { "review": "claude", "poll": "claude", "summary": "devin" },
            "schedule": { "interval_minutes": 15 }
        });
        let statuses = check(
            &[
                prerequisite("claude", Some(("jobs", "claude"))),
                prerequisite("devin", Some(("jobs", "devin"))),
                prerequisite("codex", Some(("jobs", "codex"))),
                prerequisite("other", Some(("schedule", "15"))),
                prerequisite("absent", Some(("missing", "claude"))),
            ],
            Some(&settings),
            OsStr::new(""),
        );
        let needed: Vec<bool> = statuses.iter().map(|status| status.needed).collect();
        assert_eq!(needed, [true, true, false, false, false]);

        let unknown = check(
            &[prerequisite("codex", Some(("jobs", "codex")))],
            None,
            OsStr::new(""),
        );
        assert!(
            unknown[0].needed,
            "unknown settings count a conditional program as needed"
        );
    }
}
