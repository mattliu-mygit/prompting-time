// A fake plugin, laid out like Whooper's release: a manifest, shell commands, and a settings schema.
// Its start command takes the run lock the way whooper-run does. Everything is invented and local:
// nothing here reaches the network, GitHub, Slack, or a model.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::Duration;

use prompting_time_core::plugins::{
    ActionRequest, PluginChange, PluginError, PluginHost, PluginHostConfig, PluginProcess,
    PluginSource,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const NAME: &str = "fake-agent";
const MISSING_TOOL: &str = "prompting-time-fixture-missing-tool";

const RUN: &str = r#"#!/bin/sh
# Holds the run lock as whooper-run does, then runs until stopped, with a child in its process group
# like a round in progress.
state="$XDG_STATE_HOME/fake-agent"
lock="$state/run.lock"
mkdir -p "$state"
if [ -f "$XDG_STATE_HOME/fail-start" ]; then
  echo "fake-run: no config at the fixture path" >&2
  exit 3
fi
if ! mkdir "$lock" 2>/dev/null; then
  holder="$(cat "$lock/pid" 2>/dev/null)"
  if [ -n "$holder" ] && ps -o command= -p "$holder" 2>/dev/null | grep -q fake-run; then
    echo "fake-run: process $holder already runs" >&2
    exit 1
  fi
  rm -rf "$lock"
  mkdir "$lock" || exit 1
fi
echo $$ > "$lock/pid.tmp" && mv "$lock/pid.tmp" "$lock/pid"
trap 'rm -rf "$lock"' EXIT
trap 'exit 130' TERM INT
echo "fake-run started"
sleep 600 &
echo $! > "$state/child.pid"
while :; do sleep 0.1; done
"#;

const STATUS: &str = r#"#!/bin/sh
[ "$1" = --json ] || exit 2
printf '{"version": 1, "loop": {"running": true, "pid": null}, "paused": false}\n'
"#;

const SETTINGS: &str = r#"#!/bin/sh
settings="$XDG_STATE_HOME/fake-agent.settings.json"
case "$1" in
  get)
    if [ -f "$settings" ]; then cat "$settings"; else printf '{"jobs": {"review": "sh"}}\n'; fi ;;
  set)
    case "$2" in
      *'"interval_minutes":3'*) echo "fake-settings: interval_minutes must be at least 5" >&2; exit 1 ;;
    esac
    printf '%s\n' "$2" > "$settings" ;;
  *) exit 2 ;;
esac
"#;

const SUBMIT: &str = r#"#!/bin/sh
# Answers like whooper-submit: --print shows the request's fingerprint, --expect refuses a changed
# request with 13, and a review that mentions someone is refused with 12.
run="$1"
shift
printf '%s\n' "$run $*" >> "$XDG_STATE_HOME/fake-agent-submit.log"
fingerprint="$(printf 'request for %s' "$run" | shasum -a 256 | cut -d ' ' -f 1)"
if [ "$run" = mentions ]; then
  echo "fake-submit: nothing posted: the review @-mentions someone" >&2
  exit 12
fi
while [ $# -gt 0 ]; do
  case "$1" in
    --print) echo "printed the request; nothing posted"; echo "fingerprint: $fingerprint"; exit 0 ;;
    --expect) [ "$2" = "$fingerprint" ] || { echo "fake-submit: the drafts changed" >&2; exit 13; }; shift ;;
  esac
  shift
done
echo "posted the review"
"#;

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn config(&self) -> PluginHostConfig {
        PluginHostConfig {
            plugins_dir: self.path("plugins"),
            state_home: self.path("state"),
            downloader: self.path("fake-curl"),
        }
    }

    fn host(&self) -> PluginHost {
        PluginHost::new(self.config()).unwrap()
    }

    fn state_dir(&self) -> PathBuf {
        self.path("state").join(NAME)
    }

    /// Writes the fake plugin to `source/fake-agent-0.1.0/` with these prerequisites besides `sh`.
    fn plugin(&self, extra_prerequisites: Value) -> PathBuf {
        let folder = self.path("source").join("fake-agent-0.1.0");
        let mut prerequisites = vec![json!({ "command": "sh", "purpose": "Runs the scripts." })];
        prerequisites.extend(extra_prerequisites.as_array().unwrap().iter().cloned());
        prerequisites.push(json!({
            "command": MISSING_TOOL,
            "purpose": "Runs the jobs the settings give it.",
            "only_when": { "setting": "jobs", "value": "missing-agent" }
        }));
        let manifest = json!({
            "manifest_version": 1,
            "name": NAME,
            "title": "Fake agent",
            "version": "0.1.0",
            "description": "An invented background agent.",
            "start": ["bin/fake-run"],
            "prerequisites": prerequisites,
            "page": { "entry": "page/index.html", "protocol": 1 },
            "settings": "schemas/settings.json",
            "actions": [
                { "name": "status", "title": "Status", "description": "Prints status.",
                  "command": ["bin/fake-status", "--json"], "confirm": false },
                { "name": "get-settings", "title": "Read settings", "description": "Prints them.",
                  "command": ["bin/fake-settings", "get"], "confirm": false },
                { "name": "save-settings", "title": "Save settings", "description": "Saves them.",
                  "command": ["bin/fake-settings", "set"], "confirm": false },
                { "name": "submit", "title": "Submit", "description": "Posts a review.",
                  "command": ["bin/fake-submit"], "confirm": true }
            ]
        });
        fs::create_dir_all(folder.join("schemas")).unwrap();
        fs::write(
            folder.join("prompting-time-plugin.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(
            folder.join("schemas/settings.json"),
            r#"{"type": "object", "properties": {"jobs": {"type": "object"}}}"#,
        )
        .unwrap();
        for (name, body) in [
            ("fake-run", RUN),
            ("fake-status", STATUS),
            ("fake-settings", SETTINGS),
            ("fake-submit", SUBMIT),
        ] {
            executable(&folder.join("bin").join(name), body);
        }
        folder
    }

    /// Packs the plugin folder the way Whooper's build-release does: one folder in a .tar.gz.
    fn archive(&self, folder: &Path) -> (PathBuf, String) {
        let archive = self.path("fake-agent-0.1.0.tar.gz");
        let status = std::process::Command::new("/usr/bin/tar")
            .env("COPYFILE_DISABLE", "1")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(folder.parent().unwrap())
            .arg(folder.file_name().unwrap())
            .status()
            .unwrap();
        assert!(status.success());
        let sha256 = format!("{:x}", Sha256::digest(fs::read(&archive).unwrap()));
        (archive, sha256)
    }

    /// Starts the start command outside PT, as a terminal would, in its own process group.
    fn start_elsewhere(&self, folder: &Path) -> Child {
        std::process::Command::new(folder.join("bin/fake-run"))
            .env("XDG_STATE_HOME", self.path("state"))
            .stdout(Stdio::null())
            // The shell reports its stopped sleep on stderr when the test stops it.
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn lock_pid(&self) -> Option<u32> {
        fs::read_to_string(self.state_dir().join("run.lock/pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn submit_log(&self) -> String {
        fs::read_to_string(self.path("state").join("fake-agent-submit.log")).unwrap_or_default()
    }
}

fn executable(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn stop_elsewhere(mut child: Child) {
    let _ = std::process::Command::new("/bin/kill")
        .args(["-TERM", "--", &format!("-{}", child.id())])
        .status();
    let _ = child.wait();
}

async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting until {what}");
}

fn action(name: &str, args: &[&str], confirmed_fingerprint: Option<String>) -> ActionRequest {
    ActionRequest {
        plugin: NAME.to_owned(),
        action: name.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        confirmed_fingerprint,
    }
}

async fn only_plugin(host: &PluginHost) -> prompting_time_core::plugins::PluginSnapshot {
    let mut plugins = host.list().await;
    assert_eq!(plugins.len(), 1);
    plugins.remove(0)
}

#[tokio::test]
async fn an_archive_install_unpacks_checks_starts_and_stops_its_process_group() {
    let fixture = Fixture::new();
    let (archive, sha256) = fixture.archive(&fixture.plugin(json!([])));
    let host = fixture.host();
    let mut changes = host.subscribe();

    let snapshot = host
        .install(PluginSource::Archive {
            path: archive.clone(),
            sha256: sha256.to_ascii_uppercase(),
        })
        .await
        .unwrap();

    assert_eq!(snapshot.manifest.name, NAME);
    assert_eq!(snapshot.folder, fixture.path("plugins").join(NAME));
    assert!(snapshot.folder.join("bin/fake-run").exists());
    assert_eq!(
        snapshot.source,
        PluginSource::Archive {
            path: archive,
            sha256: sha256.clone()
        },
        "the checksum is recorded in lowercase"
    );
    assert_eq!(snapshot.state_folder, fixture.state_dir());
    assert!(snapshot.settings_schema.contains("jobs"));
    assert_eq!(snapshot.settings_error, None);
    let sh = &snapshot.prerequisites[0];
    assert!(sh.needed && sh.path.is_some());
    let conditional = &snapshot.prerequisites[1];
    assert_eq!(conditional.command, MISSING_TOOL);
    assert!(!conditional.needed, "the settings give no job to it");
    assert_eq!(conditional.path, None);
    let PluginProcess::Started { pid } = snapshot.process else {
        panic!("PT starts the plugin it installs: {:?}", snapshot.process);
    };
    eventually("the start command holds the run lock", || {
        fixture.lock_pid() == Some(pid)
    })
    .await;
    eventually("the round-like child runs", || {
        fixture.state_dir().join("child.pid").exists()
    })
    .await;
    let child: u32 = fs::read_to_string(fixture.state_dir().join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        changes.recv().await.unwrap(),
        PluginChange {
            plugin: NAME.to_owned(),
            files: Some(Vec::new())
        },
        "PT's own changes list no files"
    );

    host.shutdown().await;

    assert!(!alive(pid), "the start command stopped");
    assert!(!alive(child), "SIGTERM reached the whole process group");
    assert_eq!(
        fixture.lock_pid(),
        None,
        "the start command cleaned up its lock"
    );
    let reopened = fixture.host();
    let snapshot = only_plugin(&reopened).await;
    assert_eq!(snapshot.manifest.name, NAME, "the install was recorded");
    let PluginProcess::NotRunning { exit: None } = snapshot.process else {
        panic!("a reopened host starts nothing until start_installed");
    };
    reopened.shutdown().await;
}

#[tokio::test]
async fn a_checksum_mismatch_installs_nothing() {
    let fixture = Fixture::new();
    let (archive, sha256) = fixture.archive(&fixture.plugin(json!([])));
    let host = fixture.host();

    let error = host
        .install(PluginSource::Archive {
            path: archive,
            sha256: "0".repeat(64),
        })
        .await
        .unwrap_err();

    match error {
        PluginError::ChecksumMismatch { expected, actual } => {
            assert_eq!(expected, "0".repeat(64));
            assert_eq!(actual, sha256);
        }
        other => panic!("expected a checksum refusal, got {other:?}"),
    }
    assert!(host.list().await.is_empty());
    let plugins_dir = fixture.path("plugins");
    assert!(!plugins_dir.join(NAME).exists());
    assert!(!plugins_dir.join(format!("{NAME}.json")).exists());
    assert_eq!(
        fs::read_dir(plugins_dir.join(".staging")).unwrap().count(),
        0,
        "nothing is left in the staging folder"
    );
    assert!(!fixture.state_dir().exists(), "nothing started");
    assert!(matches!(
        host.install(PluginSource::Archive {
            path: PathBuf::from("relative.tar.gz"),
            sha256: "0".repeat(64)
        })
        .await,
        Err(PluginError::InvalidRequest(_))
    ));
    assert!(matches!(
        host.install(PluginSource::Archive {
            path: fixture.path("fake-agent-0.1.0.tar.gz"),
            sha256: "not hex".to_owned()
        })
        .await,
        Err(PluginError::InvalidRequest(_))
    ));
    host.shutdown().await;
}

#[tokio::test]
async fn a_release_is_downloaded_over_https_and_checked() {
    let fixture = Fixture::new();
    let (archive, sha256) = fixture.archive(&fixture.plugin(json!([])));
    executable(
        &fixture.path("fake-curl"),
        &format!(
            r#"#!/bin/sh
while [ $# -gt 1 ]; do
  [ "$1" = --output ] && {{ output="$2"; shift; }}
  shift
done
printf '%s\n' "$1" > "{downloaded}"
case "$1" in
  *missing*) echo "curl: (22) The requested URL returned error: 404" >&2; exit 22 ;;
esac
cp "{archive}" "$output"
"#,
            downloaded = fixture.path("downloaded-url").display(),
            archive = archive.display()
        ),
    );
    let host = fixture.host();
    let url = "https://downloads.example.invalid/v0.1.0/fake-agent-0.1.0.tar.gz";

    let snapshot = host
        .install(PluginSource::Release {
            url: url.to_owned(),
            sha256: sha256.clone(),
        })
        .await
        .unwrap();

    assert_eq!(
        fs::read_to_string(fixture.path("downloaded-url"))
            .unwrap()
            .trim(),
        url
    );
    assert_eq!(
        snapshot.source,
        PluginSource::Release {
            url: url.to_owned(),
            sha256: sha256.clone()
        }
    );
    match host
        .install(PluginSource::Release {
            url: "https://downloads.example.invalid/missing.tar.gz".to_owned(),
            sha256: sha256.clone(),
        })
        .await
    {
        Err(PluginError::Download(message)) => assert!(message.contains("404"), "{message}"),
        other => panic!("expected a download failure, got {other:?}"),
    }
    assert!(matches!(
        host.install(PluginSource::Release {
            url: "http://downloads.example.invalid/fake-agent-0.1.0.tar.gz".to_owned(),
            sha256,
        })
        .await,
        Err(PluginError::InvalidRequest(_))
    ));
    host.shutdown().await;
}

#[tokio::test]
async fn a_copy_holding_the_run_lock_is_attached_and_left_running_when_pt_quits() {
    let fixture = Fixture::new();
    let folder = fixture.plugin(json!([]));
    let elsewhere = fixture.start_elsewhere(&folder);
    let pid = elsewhere.id();
    eventually("the terminal copy holds the run lock", || {
        fixture.lock_pid() == Some(pid)
    })
    .await;
    let host = fixture.host();

    let snapshot = host
        .install(PluginSource::Folder {
            path: folder.clone(),
        })
        .await
        .unwrap();

    assert_eq!(snapshot.process, PluginProcess::RunningElsewhere { pid });
    assert_eq!(
        snapshot.folder,
        folder.canonicalize().unwrap(),
        "a folder runs in place"
    );
    assert!(!fixture.path("plugins").join(NAME).exists());
    host.shutdown().await;
    assert!(alive(pid), "PT leaves a copy it did not start running");
    assert_eq!(fixture.lock_pid(), Some(pid));
    stop_elsewhere(elsewhere);
}

#[tokio::test]
async fn a_stale_run_lock_does_not_keep_pt_from_starting_the_plugin() {
    let fixture = Fixture::new();
    let folder = fixture.plugin(json!([]));
    // The lock names a live process that is not the start command: this test's own.
    let lock = fixture.state_dir().join("run.lock");
    fs::create_dir_all(&lock).unwrap();
    fs::write(lock.join("pid"), std::process::id().to_string()).unwrap();
    let host = fixture.host();

    let snapshot = host
        .install(PluginSource::Folder { path: folder })
        .await
        .unwrap();

    let PluginProcess::Started { pid } = snapshot.process else {
        panic!("a stale lock holds nothing: {:?}", snapshot.process);
    };
    eventually("PT's copy takes the lock over", || {
        fixture.lock_pid() == Some(pid)
    })
    .await;
    host.shutdown().await;
    assert!(!alive(pid));
}

#[tokio::test]
async fn pt_reports_how_its_start_command_ended() {
    let fixture = Fixture::new();
    let folder = fixture.plugin(json!([]));
    fs::create_dir_all(fixture.path("state")).unwrap();
    fs::write(fixture.path("state").join("fail-start"), "").unwrap();
    let host = fixture.host();
    host.install(PluginSource::Folder { path: folder })
        .await
        .unwrap();

    let mut exit = None;
    for _ in 0..200 {
        if let PluginProcess::NotRunning {
            exit: Some(recorded),
        } = only_plugin(&host).await.process
        {
            exit = Some(recorded);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let exit = exit.expect("the start command's exit is recorded");
    assert_eq!(exit.code, Some(3));
    assert!(exit.output.contains("no config"), "{exit:?}");
    host.shutdown().await;
}

#[tokio::test]
async fn actions_return_their_output_and_exit_code_and_confirm_actions_need_a_fingerprint() {
    let fixture = Fixture::new();
    let host = fixture.host();
    host.install(PluginSource::Folder {
        path: fixture.plugin(json!([])),
    })
    .await
    .unwrap();

    let status = host.run_action(action("status", &[], None)).await.unwrap();
    assert_eq!(status.exit_code, Some(0));
    let json: Value = serde_json::from_str(&status.stdout).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(status.fingerprint, None);

    let preview = host
        .run_action(action("submit", &["run-1"], None))
        .await
        .unwrap();
    let expected = format!("{:x}", Sha256::digest(b"request for run-1"));
    assert_eq!(preview.exit_code, Some(0));
    assert_eq!(preview.fingerprint.as_deref(), Some(expected.as_str()));
    assert_eq!(
        fixture.submit_log(),
        "run-1 --print\n",
        "a preview posts nothing"
    );

    let submitted = host
        .run_action(action("submit", &["run-1"], Some(expected.clone())))
        .await
        .unwrap();
    assert_eq!(submitted.exit_code, Some(0));
    assert!(submitted.stdout.contains("posted the review"));
    assert!(
        fixture
            .submit_log()
            .ends_with(&format!("run-1 --expect {expected}\n"))
    );

    let changed = host
        .run_action(action("submit", &["run-1"], Some("b".repeat(64))))
        .await
        .unwrap();
    assert_eq!(changed.exit_code, Some(13));
    assert!(changed.stderr.contains("drafts changed"));

    let refused = host
        .run_action(action("submit", &["mentions"], None))
        .await
        .unwrap();
    assert_eq!(refused.exit_code, Some(12));
    assert_eq!(refused.fingerprint, None);
    assert!(refused.stderr.contains("@-mentions"));

    for (request, what) in [
        (
            action("status", &[], Some("a".repeat(64))),
            "a fingerprint for an action without confirm",
        ),
        (
            action("submit", &["run-1"], Some("not-hex".to_owned())),
            "a malformed fingerprint",
        ),
    ] {
        assert!(
            matches!(
                host.run_action(request).await,
                Err(PluginError::InvalidRequest(_))
            ),
            "{what}"
        );
    }
    assert!(matches!(
        host.run_action(action("missing", &[], None)).await,
        Err(PluginError::UnknownAction { .. })
    ));
    assert!(matches!(
        host.run_action(ActionRequest {
            plugin: "other".to_owned(),
            ..action("status", &[], None)
        })
        .await,
        Err(PluginError::UnknownPlugin(_))
    ));
    host.shutdown().await;
}

#[tokio::test]
async fn saved_settings_go_through_the_plugin_and_decide_which_programs_are_needed() {
    let fixture = Fixture::new();
    let host = fixture.host();
    host.install(PluginSource::Folder {
        path: fixture.plugin(json!([])),
    })
    .await
    .unwrap();

    let refused = host
        .save_settings(NAME, r#"{"schedule": {"interval_minutes": 3}}"#)
        .await
        .unwrap();
    assert_eq!(refused.exit_code, Some(1));
    assert!(refused.stderr.contains("at least 5"));
    assert!(matches!(
        host.save_settings(NAME, "[1, 2]").await,
        Err(PluginError::InvalidRequest(_))
    ));

    let saved = host
        .save_settings(NAME, r#"{"jobs": {"review": "missing-agent"}}"#)
        .await
        .unwrap();
    assert_eq!(saved.exit_code, Some(0));
    let conditional = only_plugin(&host).await.prerequisites[1].clone();
    assert!(conditional.needed, "the settings now give a job to it");
    assert_eq!(conditional.path, None);
    host.shutdown().await;

    let reopened = fixture.host();
    reopened.start_installed().await;
    let snapshot = only_plugin(&reopened).await;
    assert_eq!(
        snapshot.process,
        PluginProcess::NotRunning { exit: None },
        "PT does not start a plugin whose needed program is missing"
    );
    reopened.shutdown().await;
}

#[tokio::test]
async fn a_missing_program_keeps_the_plugin_from_starting() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let snapshot = host
        .install(PluginSource::Folder {
            path: fixture.plugin(json!([
                { "command": "prompting-time-fixture-absent", "purpose": "Reads the fixture's data." }
            ])),
        })
        .await
        .unwrap();

    let absent = &snapshot.prerequisites[1];
    assert_eq!(absent.command, "prompting-time-fixture-absent");
    assert!(absent.needed && absent.path.is_none());
    assert_eq!(snapshot.process, PluginProcess::NotRunning { exit: None });
    assert_eq!(fixture.lock_pid(), None);
    host.shutdown().await;
}

#[tokio::test]
async fn a_change_in_the_state_folder_reaches_subscribers_as_its_relative_path() {
    let fixture = Fixture::new();
    let host = fixture.host();
    host.install(PluginSource::Folder {
        path: fixture.plugin(json!([])),
    })
    .await
    .unwrap();
    let mut changes = host.subscribe();
    let run = fixture.state_dir().join("runs/run-1");
    fs::create_dir_all(&run).unwrap();

    // FSEvents can take a moment to start delivering, so write until a change arrives.
    let seen = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            fs::write(run.join("drafts.json.tmp"), r#"{"event": "COMMENT"}"#).unwrap();
            fs::rename(run.join("drafts.json.tmp"), run.join("drafts.json")).unwrap();
            let wait = tokio::time::sleep(Duration::from_millis(500));
            tokio::pin!(wait);
            loop {
                tokio::select! {
                    _ = &mut wait => break,
                    change = changes.recv() => {
                        let change = change.unwrap();
                        if change.files.as_ref().is_some_and(|files| {
                            files.iter().any(|file| file == "runs/run-1/drafts.json")
                        }) {
                            return change;
                        }
                    }
                }
            }
        }
    })
    .await
    .expect("a change event arrives");

    assert_eq!(seen.plugin, NAME);
    host.shutdown().await;
}
