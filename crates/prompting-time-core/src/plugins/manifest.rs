//! The manifest a plugin ships as `prompting-time-plugin.json`: what Prompting Time reads to
//! install, check, start, and configure it. Whooper's `schemas/prompting-time-plugin.json` is the
//! format's schema; this module applies the same rules, then checks the files the manifest names.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path};

use serde::Deserialize;

pub const MANIFEST_FILE: &str = "prompting-time-plugin.json";

/// The action that prints the plugin's settings as one JSON object. A prerequisite with
/// `only_when` reads its setting from this output.
pub const GET_SETTINGS_ACTION: &str = "get-settings";
/// The action that checks and saves settings, given as one JSON argument.
pub const SAVE_SETTINGS_ACTION: &str = "save-settings";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub manifest_version: u32,
    /// The plugin's ID: it names the plugin's folder and state folder.
    pub name: String,
    pub title: String,
    pub version: String,
    pub description: String,
    /// The command PT starts when it opens: a path in the plugin's folder, then its arguments.
    pub start: Vec<String>,
    pub prerequisites: Vec<Prerequisite>,
    pub page: Page,
    /// The JSON Schema of the settings, as a path in the plugin's folder.
    pub settings: String,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Prerequisite {
    /// A program PT looks for on the PATH it gives plugin commands.
    pub command: String,
    /// Why the plugin needs it, for the message when it is missing.
    pub purpose: String,
    #[serde(default)]
    pub only_when: Option<SettingCondition>,
}

/// The program is needed only when one of the values in the settings object `setting` is
/// `value`: `{"setting": "jobs", "value": "devin"}` holds for `{"jobs": {"review": "devin"}}`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SettingCondition {
    pub setting: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub entry: String,
    pub protocol: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub name: String,
    pub title: String,
    pub description: String,
    /// A path in the plugin's folder and its fixed arguments; PT adds the rest, such as a run ID.
    pub command: Vec<String>,
    /// PT runs the action only after the user confirms what it will do.
    pub confirm: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct ManifestError(pub String);

impl Manifest {
    /// Reads a manifest and checks the rules of manifest version 1.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(|error| {
            ManifestError(format!(
                "{MANIFEST_FILE} does not fit the manifest format: {error}"
            ))
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), ManifestError> {
        if self.manifest_version != 1 {
            return invalid(format!(
                "manifest_version is {}, and Prompting Time reads version 1",
                self.manifest_version
            ));
        }
        if !is_id(&self.name) {
            return invalid(format!(
                "the name {:?} must start with a lowercase letter and hold only lowercase letters, digits, and hyphens",
                self.name
            ));
        }
        if !is_version(&self.version) {
            return invalid(format!(
                "the version {:?} must be three numbers, such as 1.2.3",
                self.version
            ));
        }
        check_command("start", &self.start)?;
        for prerequisite in &self.prerequisites {
            if prerequisite.command.is_empty()
                || !prerequisite
                    .command
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            {
                return invalid(format!(
                    "the prerequisite {:?} must be a program name, such as gh",
                    prerequisite.command
                ));
            }
        }
        if self.page.protocol != 1 {
            return invalid(format!(
                "page.protocol is {}, and Prompting Time speaks protocol 1",
                self.page.protocol
            ));
        }
        check_relative("page.entry", &self.page.entry)?;
        check_relative("settings", &self.settings)?;
        let mut names = HashSet::new();
        for action in &self.actions {
            if !is_id(&action.name) {
                return invalid(format!(
                    "the action name {:?} must start with a lowercase letter and hold only lowercase letters, digits, and hyphens",
                    action.name
                ));
            }
            if !names.insert(action.name.as_str()) {
                return invalid(format!("two actions are named {}", action.name));
            }
            check_command(&format!("the action {}", action.name), &action.command)?;
        }
        if self
            .prerequisites
            .iter()
            .any(|prerequisite| prerequisite.only_when.is_some())
            && !names.contains(GET_SETTINGS_ACTION)
        {
            return invalid(format!(
                "a prerequisite depends on a setting, so the manifest needs a {GET_SETTINGS_ACTION} action"
            ));
        }
        Ok(())
    }

    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.name == name)
    }

    /// The start command's file name, such as `whooper-run`, which a running copy's command line
    /// holds.
    pub fn start_program(&self) -> &str {
        Path::new(&self.start[0])
            .file_name()
            .and_then(|name| name.to_str())
            .expect("validated start paths end in a normal component")
    }
}

/// Reads and checks the manifest in `folder` and the files it names. Returns the manifest and the
/// settings schema's text, which holds a JSON object.
pub fn load(folder: &Path) -> Result<(Manifest, String), ManifestError> {
    let bytes = fs::read(folder.join(MANIFEST_FILE)).map_err(|error| {
        ManifestError(format!(
            "{} has no readable {MANIFEST_FILE}: {error}",
            folder.display()
        ))
    })?;
    let manifest = Manifest::parse(&bytes)?;
    check_executable(folder, "the start command", &manifest.start[0])?;
    for action in &manifest.actions {
        check_executable(
            folder,
            &format!("the action {}", action.name),
            &action.command[0],
        )?;
    }
    let schema = fs::read_to_string(folder.join(&manifest.settings)).map_err(|error| {
        ManifestError(format!(
            "the settings schema {} is not readable: {error}",
            manifest.settings
        ))
    })?;
    if !serde_json::from_str::<serde_json::Value>(&schema).is_ok_and(|value| value.is_object()) {
        return invalid(format!(
            "the settings schema {} is not a JSON object",
            manifest.settings
        ));
    }
    Ok((manifest, schema))
}

fn invalid<T>(message: String) -> Result<T, ManifestError> {
    Err(ManifestError(message))
}

fn is_id(text: &str) -> bool {
    let mut characters = text.chars();
    characters.next().is_some_and(|c| c.is_ascii_lowercase())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

fn check_command(label: &str, command: &[String]) -> Result<(), ManifestError> {
    match command.first() {
        Some(program) => check_relative(label, program),
        None => invalid(format!("{label} names no command")),
    }
}

/// Paths in a manifest stay inside the plugin's folder: relative, with no `..` or `.`.
fn check_relative(label: &str, path: &str) -> Result<(), ManifestError> {
    let inside = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if inside {
        Ok(())
    } else {
        invalid(format!(
            "{label} names {path:?}, which is not a path inside the plugin's folder"
        ))
    }
}

fn check_executable(folder: &Path, label: &str, path: &str) -> Result<(), ManifestError> {
    let executable = fs::metadata(folder.join(path))
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
    if executable {
        Ok(())
    } else {
        invalid(format!(
            "{label} names {path}, which is not an executable file in the plugin's folder"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whooper_like() -> serde_json::Value {
        serde_json::json!({
            "manifest_version": 1,
            "name": "fixture-agent",
            "title": "Fixture agent",
            "version": "0.1.0",
            "description": "An invented plugin.",
            "start": ["bin/run"],
            "prerequisites": [
                { "command": "gh", "purpose": "Reads pull requests." },
                { "command": "devin", "purpose": "Runs jobs.",
                  "only_when": { "setting": "jobs", "value": "devin" } }
            ],
            "page": { "entry": "page/dist/index.html", "protocol": 1 },
            "settings": "schemas/settings.json",
            "actions": [
                { "name": "get-settings", "title": "Read settings", "description": "Prints them.",
                  "command": ["bin/settings", "get"], "confirm": false },
                { "name": "submit", "title": "Submit", "description": "Posts.",
                  "command": ["bin/submit"], "confirm": true }
            ]
        })
    }

    fn parse(value: &serde_json::Value) -> Result<Manifest, ManifestError> {
        Manifest::parse(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn a_whooper_like_manifest_parses() {
        let manifest = parse(&whooper_like()).unwrap();
        assert_eq!(manifest.name, "fixture-agent");
        assert_eq!(manifest.start_program(), "run");
        assert_eq!(
            manifest.prerequisites[1].only_when,
            Some(SettingCondition {
                setting: "jobs".into(),
                value: "devin".into()
            })
        );
        assert!(manifest.action("submit").unwrap().confirm);
        assert!(manifest.action("missing").is_none());
    }

    #[test]
    fn manifests_that_break_a_rule_are_refused_with_the_rule() {
        type Change = fn(&mut serde_json::Value);
        let cases: Vec<(&str, Change)> = vec![
            ("does not fit", |m| m["extra"] = "field".into()),
            ("does not fit", |m| {
                m["prerequisites"][0]["path"] = "/bin".into()
            }),
            ("version 1", |m| m["manifest_version"] = 2.into()),
            ("lowercase letter", |m| m["name"] = "Fixture".into()),
            ("lowercase letter", |m| m["name"] = "../escape".into()),
            ("three numbers", |m| m["version"] = "1.2".into()),
            ("three numbers", |m| m["version"] = "1.2.x".into()),
            ("names no command", |m| m["start"] = serde_json::json!([])),
            ("inside the plugin's folder", |m| {
                m["start"] = serde_json::json!(["../run"])
            }),
            ("inside the plugin's folder", |m| {
                m["start"] = serde_json::json!(["/bin/sh"])
            }),
            ("inside the plugin's folder", |m| {
                m["start"] = serde_json::json!(["./run"])
            }),
            ("inside the plugin's folder", |m| {
                m["settings"] = "../settings.json".into()
            }),
            ("program name", |m| {
                m["prerequisites"][0]["command"] = "bin/gh".into()
            }),
            ("protocol 1", |m| m["page"]["protocol"] = 2.into()),
            ("two actions", |m| {
                let copy = m["actions"][0].clone();
                m["actions"].as_array_mut().unwrap().push(copy);
            }),
            ("action name", |m| m["actions"][1]["name"] = "Submit".into()),
            ("needs a get-settings action", |m| {
                m["actions"].as_array_mut().unwrap().remove(0);
            }),
        ];
        for (expected, change) in cases {
            let mut manifest = whooper_like();
            change(&mut manifest);
            let error = parse(&manifest).unwrap_err();
            assert!(error.0.contains(expected), "{expected}: {error}");
        }
    }
}
