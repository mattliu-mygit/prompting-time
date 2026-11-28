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
    ACTION_OUTPUT_LIMIT, ActionRequest, PluginChange, PluginError, PluginHost, PluginHostConfig,
    PluginProcess, PluginSource,
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
# Answers like whooper-submit: --print saves the request in the state folder, names the file, and
# prints its fingerprint, the file's SHA-256; --expect refuses a changed request with 13; and a review
# that mentions someone is refused with 12. The runs "outside" and "mismatch" save the request outside
# the state folder, or a file that does not match the fingerprint.
run="$1"
shift
printf '%s\n' "$run $*" >> "$XDG_STATE_HOME/fake-agent-submit.log"
request="request for $run"
fingerprint="$(printf '%s' "$request" | shasum -a 256 | cut -d ' ' -f 1)"
if [ "$run" = mentions ]; then
  echo "fake-submit: nothing posted: the review @-mentions someone" >&2
  exit 12
fi
while [ $# -gt 0 ]; do
  case "$1" in
    --print)
      file="$XDG_STATE_HOME/fake-agent/runs/$run/submit-request.json"
      [ "$run" = outside ] && file="$XDG_STATE_HOME/outside-request.json"
      mkdir -p "$(dirname "$file")"
      printf '%s' "$request" > "$file"
      [ "$run" = mismatch ] && printf ' changed later' >> "$file"
      echo "printed the request to $file; nothing posted"
      echo "fingerprint: $fingerprint"
      exit 0 ;;
    --expect) [ "$2" = "$fingerprint" ] || { echo "fake-submit: the drafts changed" >&2; exit 13; }; shift ;;
  esac
  shift
done
echo "posted the review"
"#;

const RERUN: &str = r#"#!/bin/sh
# A row action of status format 2: logs the arguments it was given, after its fixed one.
printf '%s\n' "$*" >> "$XDG_STATE_HOME/fake-agent-rerun.log"
echo "queued again"
"#;

const DUMP: &str = r#"#!/bin/sh
# Prints as many bytes as its argument says.
head -c "$1" /dev/zero | tr '\0' x
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
            "page": { "entry": "page/index.html", "protocol": 2 },
            "settings": "schemas/settings.json",
            "actions": [
                { "name": "status", "title": "Status", "description": "Prints status.",
                  "command": ["bin/fake-status", "--json"], "confirm": false },
                { "name": "get-settings", "title": "Read settings", "description": "Prints them.",
                  "command": ["bin/fake-settings", "get"], "confirm": false },
                { "name": "save-settings", "title": "Save settings", "description": "Saves them.",
                  "command": ["bin/fake-settings", "set"], "confirm": false },
                { "name": "submit", "title": "Submit", "description": "Posts a review.",
                  "command": ["bin/fake-submit"], "confirm": true },
                { "name": "rerun", "title": "Rerun", "description": "Runs a job again.",
                  "command": ["bin/fake-rerun", "--now"], "confirm": false },
                { "name": "dump", "title": "Dump", "description": "Prints many bytes.",
                  "command": ["bin/fake-dump"], "confirm": false }
            ],
            "status": {
                "format": 2,
                "groups": [
                    { "id": "failed", "title": "Run failed", "needs_you": true, "order": 1 },
                    { "id": "done", "title": "Done", "needs_you": false, "order": 2 }
                ],
                "item_actions": ["rerun", "submit"]
            }
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
            ("fake-rerun", RERUN),
            ("fake-dump", DUMP),
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
        preview.preview.as_deref(),
        Some("request for run-1"),
        "the preview is the saved request whose SHA-256 is the fingerprint"
    );
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
async fn an_action_that_prints_more_than_pt_keeps_says_its_output_is_cut_off() {
    let fixture = Fixture::new();
    let host = fixture.host();
    host.install(PluginSource::Folder {
        path: fixture.plugin(json!([])),
    })
    .await
    .unwrap();
    let limit = ACTION_OUTPUT_LIMIT as usize;

    let whole = host
        .run_action(action("dump", &[&limit.to_string()], None))
        .await
        .unwrap();
    assert_eq!(whole.exit_code, Some(0));
    assert_eq!(whole.stdout.len(), limit);
    assert!(
        !whole.stdout_truncated,
        "output of exactly the limit is whole"
    );

    let cut = host
        .run_action(action("dump", &[&(limit + 1).to_string()], None))
        .await
        .unwrap();
    assert_eq!(cut.exit_code, Some(0));
    assert_eq!(cut.stdout.len(), limit, "PT keeps the start");
    assert!(cut.stdout_truncated);
    host.shutdown().await;
}

#[tokio::test]
async fn a_row_action_gets_the_item_after_its_fixed_arguments_and_confirms_like_any_action() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let snapshot = host
        .install(PluginSource::Folder {
            path: fixture.plugin(json!([])),
        })
        .await
        .unwrap();
    let status = snapshot
        .manifest
        .status
        .expect("the manifest declares status format 2");
    assert_eq!(status.item_actions, vec!["rerun", "submit"]);

    let rerun = host
        .run_action(action("rerun", &["--item", "run-74c91b1d"], None))
        .await
        .unwrap();
    assert_eq!(rerun.exit_code, Some(0));
    assert_eq!(rerun.stdout.trim(), "queued again");
    assert_eq!(
        fs::read_to_string(fixture.path("state").join("fake-agent-rerun.log")).unwrap(),
        "--now --item run-74c91b1d\n",
        "the item follows the action's fixed arguments"
    );

    // A row action marked confirm previews first, with the item, then runs with the fingerprint.
    let preview = host
        .run_action(action("submit", &["run-2", "--item", "run-2"], None))
        .await
        .unwrap();
    let fingerprint = preview
        .fingerprint
        .expect("a preview prints its fingerprint");
    host.run_action(action(
        "submit",
        &["run-2", "--item", "run-2"],
        Some(fingerprint.clone()),
    ))
    .await
    .unwrap();
    assert_eq!(
        fixture.submit_log(),
        format!("run-2 --item run-2 --print\nrun-2 --item run-2 --expect {fingerprint}\n")
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_preview_shows_only_a_saved_request_in_the_state_folder_that_matches_its_fingerprint() {
    let fixture = Fixture::new();
    let host = fixture.host();
    host.install(PluginSource::Folder {
        path: fixture.plugin(json!([])),
    })
    .await
    .unwrap();

    for run in ["outside", "mismatch"] {
        assert!(
            matches!(
                host.run_action(action("submit", &[run], None)).await,
                Err(PluginError::MissingPreview(name)) if name == "submit"
            ),
            "{run}"
        );
    }
    assert!(
        fixture.path("state/outside-request.json").exists(),
        "the request outside the state folder was saved and named, and still not shown"
    );
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

#[tokio::test]
async fn pt_stops_the_copy_it_started_on_request_and_starts_it_again() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let PluginProcess::Started { pid } = host
        .install(PluginSource::Folder {
            path: fixture.plugin(json!([])),
        })
        .await
        .unwrap()
        .process
    else {
        panic!("PT starts the plugin it installs");
    };
    eventually("the round-like child runs", || {
        fixture.state_dir().join("child.pid").exists()
    })
    .await;
    let child: u32 = fs::read_to_string(fixture.state_dir().join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    let stopped = host.stop(NAME).await.unwrap();

    assert_eq!(
        stopped.process,
        PluginProcess::NotRunning { exit: None },
        "a stop PT asked for is not an exit to report"
    );
    assert!(!alive(pid), "the start command stopped");
    assert!(!alive(child), "SIGTERM reached the round in progress");
    assert_eq!(fixture.lock_pid(), None, "it released the run lock");
    assert_eq!(
        host.stop(NAME).await.unwrap().process,
        PluginProcess::NotRunning { exit: None },
        "stopping a stopped plugin changes nothing"
    );

    let started = host.start(NAME).await.unwrap();
    let PluginProcess::Started { pid: again } = started.process else {
        panic!("start runs the start command again: {:?}", started.process);
    };
    assert_ne!(again, pid);
    eventually("the new copy holds the run lock", || {
        fixture.lock_pid() == Some(again)
    })
    .await;
    assert_eq!(
        host.start(NAME).await.unwrap().process,
        PluginProcess::Started { pid: again },
        "starting a running plugin starts no second copy"
    );
    host.shutdown().await;
    assert!(!alive(again));
    assert!(matches!(host.start(NAME).await, Err(PluginError::Quitting)));
}

#[tokio::test]
async fn pt_neither_stops_nor_starts_over_a_copy_started_elsewhere() {
    let fixture = Fixture::new();
    let folder = fixture.plugin(json!([]));
    let elsewhere = fixture.start_elsewhere(&folder);
    let pid = elsewhere.id();
    eventually("the terminal copy holds the run lock", || {
        fixture.lock_pid() == Some(pid)
    })
    .await;
    let host = fixture.host();
    host.install(PluginSource::Folder { path: folder })
        .await
        .unwrap();

    match host.stop(NAME).await {
        Err(PluginError::StartedElsewhere {
            plugin,
            pid: holder,
        }) => {
            assert_eq!(plugin, NAME);
            assert_eq!(holder, pid);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(alive(pid), "the copy PT did not start keeps running");
    assert_eq!(
        host.start(NAME).await.unwrap().process,
        PluginProcess::RunningElsewhere { pid },
        "start honors the run lock"
    );
    assert_eq!(fixture.lock_pid(), Some(pid));
    assert!(matches!(
        host.stop("other").await,
        Err(PluginError::UnknownPlugin(_))
    ));
    host.shutdown().await;
    stop_elsewhere(elsewhere);
}

#[tokio::test]
async fn start_runs_a_plugin_whose_start_command_exited() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path("state")).unwrap();
    fs::write(fixture.path("state").join("fail-start"), "").unwrap();
    let host = fixture.host();
    host.install(PluginSource::Folder {
        path: fixture.plugin(json!([])),
    })
    .await
    .unwrap();
    let mut exited = false;
    for _ in 0..200 {
        if let PluginProcess::NotRunning { exit: Some(exit) } = only_plugin(&host).await.process {
            assert_eq!(exit.code, Some(3));
            exited = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(exited, "the failed start is recorded");
    fs::remove_file(fixture.path("state").join("fail-start")).unwrap();

    let PluginProcess::Started { pid } = host.start(NAME).await.unwrap().process else {
        panic!("start runs an exited plugin again");
    };
    eventually("it holds the run lock", || fixture.lock_pid() == Some(pid)).await;
    host.shutdown().await;
}

#[tokio::test]
async fn the_log_returns_the_end_of_what_the_start_command_printed() {
    let fixture = Fixture::new();
    let host = fixture.host();
    host.install(PluginSource::Folder {
        path: fixture.plugin(json!([])),
    })
    .await
    .unwrap();
    let log_path = fixture.path("plugins").join(format!("{NAME}.log"));
    eventually("the start command printed its first line", || {
        host.read_log(NAME)
            .is_ok_and(|log| log.text == "fake-run started")
    })
    .await;
    let log = host.read_log(NAME).unwrap();
    assert_eq!(log.path, log_path);
    assert!(!log.truncated);
    host.stop(NAME).await.unwrap();

    // A log past the limit comes back from its first whole line, with its end.
    let lines: Vec<String> = (0..20_000)
        .map(|index| format!("round {index}: done"))
        .collect();
    fs::write(&log_path, lines.join("\n") + "\n").unwrap();
    let long = host.read_log(NAME).unwrap();
    assert!(long.truncated);
    assert!(long.text.len() <= 256 * 1024);
    let first = long.text.lines().next().unwrap();
    assert!(lines.iter().any(|line| line == first), "{first:?}");
    assert_eq!(long.text.lines().last(), Some("round 19999: done"));

    fs::remove_file(&log_path).unwrap();
    let empty = host.read_log(NAME).unwrap();
    assert_eq!((empty.text.as_str(), empty.truncated), ("", false));
    assert!(matches!(
        host.read_log("other"),
        Err(PluginError::UnknownPlugin(_))
    ));
    host.shutdown().await;
}

#[tokio::test]
async fn a_confirm_action_refuses_the_confirmation_flags_in_its_arguments() {
    let fixture = Fixture::new();
    let host = fixture.host();
    host.install(PluginSource::Folder {
        path: fixture.plugin(json!([])),
    })
    .await
    .unwrap();
    let fingerprint = "a".repeat(64);
    let expect_equals = format!("--expect={fingerprint}");

    for args in [
        vec!["run-1", "--expect", fingerprint.as_str()],
        vec!["run-1", expect_equals.as_str()],
        vec!["run-1", "--print"],
        vec!["run-1", "--print=yes"],
        vec!["--item", "--expect"],
    ] {
        for confirmed in [None, Some(fingerprint.clone())] {
            assert!(
                matches!(
                    host.run_action(action("submit", &args, confirmed.clone())).await,
                    Err(PluginError::InvalidRequest(message)) if message.contains("--print and --expect")
                ),
                "{args:?} {confirmed:?}"
            );
        }
    }
    assert_eq!(fixture.submit_log(), "", "nothing ran");

    // An action without confirm gets its arguments as they are.
    let rerun = host
        .run_action(action("rerun", &["--print"], None))
        .await
        .unwrap();
    assert_eq!(rerun.exit_code, Some(0));
    host.shutdown().await;
}
