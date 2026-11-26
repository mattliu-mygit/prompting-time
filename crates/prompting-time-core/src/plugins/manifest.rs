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
    /// What the `status` action prints, when it prints status format 2: the groups its items go
    /// in and the actions their rows offer. None for a plugin whose status PT reads another way.
    #[serde(default)]
    pub status: Option<StatusConfig>,
}

/// The vocabulary of a plugin's status format 2, which its `status` action fills in. PT keeps no
/// group names of its own: it places, orders, and folds items by the groups declared here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StatusConfig {
    /// The status format the `status` action prints; PT reads format 2.
    pub format: u32,
    pub groups: Vec<StatusGroup>,
    /// The actions an item's row may offer. PT runs one as `<command> --item <id>`, with the same
    /// confirmation as any other action.
    #[serde(default)]
    pub item_actions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StatusGroup {
    /// What each item names as its `group`.
    pub id: String,
    pub title: String,
    /// Whether the group's items wait on the user, which lists them under Needs you.
    pub needs_you: bool,
    /// Where the group lists among the plugin's groups, lowest first.
    pub order: u32,
    /// Whether the group's section starts folded.
    #[serde(default)]
    pub folded: bool,
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
    /// The page protocol the page speaks. PT speaks only protocol 2, in which the page runs the
    /// manifest's actions itself.
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

impl Page {
    /// The folder of the page's files, relative to the plugin's folder: the entry's folder, such as
    /// `page/dist` for `page/dist/index.html`.
    pub fn folder(&self) -> &Path {
        Path::new(&self.entry)
            .parent()
            .expect("validated paths have a parent, if only the empty one")
    }

    /// The entry's path in the page's folder, such as `index.html`.
    pub fn file_name(&self) -> &str {
        Path::new(&self.entry)
            .file_name()
            .and_then(|name| name.to_str())
            .expect("validated paths end in a normal component")
    }
}

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
        if self.page.protocol != 2 {
            return invalid(format!(
                "page.protocol is {}, and Prompting Time speaks page protocol 2",
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
        if let Some(status) = &self.status {
            check_status(status, &names)?;
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

/// The rules of the manifest's `status` key: format 2, at least one group, group IDs unique and
/// lowercase-kebab, titles not empty, and item actions that name declared actions, each once.
fn check_status(status: &StatusConfig, actions: &HashSet<&str>) -> Result<(), ManifestError> {
    if status.format != 2 {
        return invalid(format!(
            "status.format is {}, and Prompting Time reads status format 2",
            status.format
        ));
    }
    if status.groups.is_empty() {
        return invalid("status.groups declares no group for items to go in".to_owned());
    }
    let mut ids = HashSet::new();
    for group in &status.groups {
        if !is_id(&group.id) {
            return invalid(format!(
                "the status group {:?} must start with a lowercase letter and hold only lowercase letters, digits, and hyphens",
                group.id
            ));
        }
        if !ids.insert(group.id.as_str()) {
            return invalid(format!("two status groups are named {}", group.id));
        }
        if group.title.trim().is_empty() {
            return invalid(format!("the status group {} has no title", group.id));
        }
    }
    let mut item_actions = HashSet::new();
    for name in &status.item_actions {
        if !actions.contains(name.as_str()) {
            return invalid(format!(
                "status.item_actions names {name}, which is not one of the manifest's actions"
            ));
        }
        if !item_actions.insert(name.as_str()) {
            return invalid(format!("status.item_actions names {name} twice"));
        }
    }
    Ok(())
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
            "page": { "entry": "page/dist/index.html", "protocol": 2 },
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
        assert_eq!(manifest.page.folder(), Path::new("page/dist"));
        assert_eq!(manifest.page.file_name(), "index.html");
        assert!(manifest.action("submit").unwrap().confirm);
        assert!(manifest.action("missing").is_none());
    }

    #[test]
    fn a_page_speaks_protocol_2() {
        assert_eq!(parse(&whooper_like()).unwrap().page.protocol, 2);
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
            ("page protocol 2", |m| m["page"]["protocol"] = 0.into()),
            ("page protocol 2", |m| m["page"]["protocol"] = 1.into()),
            ("page protocol 2", |m| m["page"]["protocol"] = 3.into()),
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

    /// The fixture with a `status` key in format 2: two groups and one row action.
    fn with_status() -> serde_json::Value {
        let mut manifest = whooper_like();
        manifest["actions"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "name": "rerun", "title": "Rerun", "description": "Runs it again.",
                "command": ["bin/rerun"], "confirm": false
            }));
        manifest["status"] = serde_json::json!({
            "format": 2,
            "groups": [
                { "id": "failed", "title": "Run failed", "needs_you": true, "order": 1 },
                { "id": "done", "title": "Done", "needs_you": false, "order": 2, "folded": true }
            ],
            "item_actions": ["rerun", "submit"]
        });
        manifest
    }

    #[test]
    fn a_status_key_declares_the_groups_and_row_actions() {
        assert_eq!(parse(&whooper_like()).unwrap().status, None);
        let status = parse(&with_status()).unwrap().status.unwrap();
        assert_eq!(status.format, 2);
        assert_eq!(
            status.groups,
            vec![
                StatusGroup {
                    id: "failed".into(),
                    title: "Run failed".into(),
                    needs_you: true,
                    order: 1,
                    folded: false
                },
                StatusGroup {
                    id: "done".into(),
                    title: "Done".into(),
                    needs_you: false,
                    order: 2,
                    folded: true
                },
            ]
        );
        assert_eq!(status.item_actions, vec!["rerun", "submit"]);
        let mut no_actions = with_status();
        no_actions["status"]
            .as_object_mut()
            .unwrap()
            .remove("item_actions");
        assert!(
            parse(&no_actions)
                .unwrap()
                .status
                .unwrap()
                .item_actions
                .is_empty()
        );
    }

    #[test]
    fn status_keys_that_break_a_rule_are_refused_with_the_rule() {
        type Change = fn(&mut serde_json::Value);
        let cases: Vec<(&str, Change)> = vec![
            ("does not fit", |m| m["status"]["extra"] = true.into()),
            ("does not fit", |m| {
                m["status"]["groups"][0]["color"] = "red".into()
            }),
            ("does not fit", |m| {
                m["status"]["groups"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("needs_you");
            }),
            ("status format 2", |m| m["status"]["format"] = 1.into()),
            ("declares no group", |m| {
                m["status"]["groups"] = serde_json::json!([])
            }),
            ("lowercase letter", |m| {
                m["status"]["groups"][0]["id"] = "Failed".into()
            }),
            ("lowercase letter", |m| {
                m["status"]["groups"][0]["id"] = "run_failed".into()
            }),
            ("two status groups", |m| {
                m["status"]["groups"][1]["id"] = "failed".into()
            }),
            ("has no title", |m| {
                m["status"]["groups"][0]["title"] = " ".into()
            }),
            ("not one of the manifest's actions", |m| {
                m["status"]["item_actions"] = serde_json::json!(["delete"])
            }),
            ("twice", |m| {
                m["status"]["item_actions"] = serde_json::json!(["rerun", "rerun"])
            }),
        ];
        for (expected, change) in cases {
            let mut manifest = with_status();
            change(&mut manifest);
            let error = parse(&manifest).unwrap_err();
            assert!(error.0.contains(expected), "{expected}: {error}");
        }
    }
}
