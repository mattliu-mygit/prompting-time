// The schemas in docs/ that plugins write to: the manifest's, which must accept what Prompting Time
// reads and refuse what it refuses where a schema can say so, and status format 2's, which must
// accept the fields Prompting Time ignores.

use std::path::Path;

use prompting_time_core::plugins::Manifest;
use serde_json::{Value, json};

fn validator(name: &str) -> jsonschema::Validator {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs")
        .join(name);
    let schema: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}

fn manifest() -> Value {
    json!({
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
        "status": {
            "format": 2,
            "groups": [{ "id": "failed", "title": "Run failed", "needs_you": true, "order": 1 }],
            "item_actions": ["rerun"]
        },
        "settings": "schemas/settings.json",
        "actions": [
            { "name": "get-settings", "title": "Read settings", "description": "Prints them.",
              "command": ["bin/settings", "get"], "confirm": false },
            { "name": "rerun", "title": "Rerun", "description": "Runs it again.",
              "command": ["bin/rerun"], "confirm": false }
        ]
    })
}

#[test]
fn the_manifest_schema_agrees_with_what_prompting_time_reads() {
    let schema = validator("prompting-time-plugin.schema.json");
    let parses = |value: &Value| Manifest::parse(&serde_json::to_vec(value).unwrap()).is_ok();
    let valid = manifest();
    assert!(schema.is_valid(&valid));
    assert!(parses(&valid));

    type Change = fn(&mut Value);
    let refused: Vec<(&str, Change)> = vec![
        ("an extra field", |m| m["extra"] = true.into()),
        ("manifest version 2", |m| m["manifest_version"] = 2.into()),
        ("an uppercase name", |m| m["name"] = "Fixture".into()),
        ("a two-part version", |m| m["version"] = "1.2".into()),
        ("no start command", |m| m["start"] = json!([])),
        ("an entry at the plugin's top", |m| {
            m["page"]["entry"] = "index.html".into()
        }),
        ("an absolute entry", |m| {
            m["page"]["entry"] = "/page/index.html".into()
        }),
        ("page protocol 1", |m| m["page"]["protocol"] = 1.into()),
        ("a prerequisite path", |m| {
            m["prerequisites"][0]["command"] = "bin/gh".into()
        }),
        ("status format 1", |m| m["status"]["format"] = 1.into()),
        ("no status groups", |m| m["status"]["groups"] = json!([])),
        ("an underscore group", |m| {
            m["status"]["groups"][0]["id"] = "run_failed".into()
        }),
        ("a blank group title", |m| {
            m["status"]["groups"][0]["title"] = " ".into()
        }),
        ("a repeated item action", |m| {
            m["status"]["item_actions"] = json!(["rerun", "rerun"])
        }),
        ("an uppercase action", |m| {
            m["actions"][1]["name"] = "Rerun".into()
        }),
        ("an action without confirm", |m| {
            m["actions"][1].as_object_mut().unwrap().remove("confirm");
        }),
    ];
    for (what, change) in refused {
        let mut value = manifest();
        change(&mut value);
        assert!(!schema.is_valid(&value), "the schema accepts {what}");
        assert!(!parses(&value), "Prompting Time reads {what}");
    }
}

#[test]
fn the_status_schema_accepts_the_fields_prompting_time_ignores() {
    let schema = validator("plugin-status-v2.schema.json");
    let status = json!({
        "format": 2,
        "agent": { "words": "Polling every 10 min", "dot": "running", "since": "10:00" },
        "blockers": [
            { "id": "signed-out", "title": "Codex is signed out", "dot": "error", "route": "/",
              "action": null, "hint": "codex login" }
        ],
        "items": [
            { "id": "run-1", "group": "failed", "title": "Review failed", "dot": "error",
              "updated_at": "2026-11-27T10:00:00Z", "route": "/runs/run-1", "detail": null,
              "project": null, "actions": ["rerun"], "pr": 74 }
        ],
        "truncated": false,
        "paused": false,
        "overview": { "for": "the plugin's page" }
    });
    assert!(schema.is_valid(&status));
    let mut without_agent = status.clone();
    without_agent.as_object_mut().unwrap().remove("agent");
    assert!(schema.is_valid(&without_agent), "agent is optional");

    type Change = fn(&mut Value);
    let refused: Vec<(&str, Change)> = vec![
        ("format 1", |s| s["format"] = 1.into()),
        ("no items", |s| {
            s.as_object_mut().unwrap().remove("items");
        }),
        ("an item without a route", |s| {
            s["items"][0].as_object_mut().unwrap().remove("route");
        }),
        ("a relative route", |s| {
            s["items"][0]["route"] = "runs/run-1".into()
        }),
        ("an unknown dot", |s| s["blockers"][0]["dot"] = "red".into()),
        ("an agent without words", |s| {
            s["agent"].as_object_mut().unwrap().remove("words");
        }),
    ];
    for (what, change) in refused {
        let mut value = status.clone();
        change(&mut value);
        assert!(!schema.is_valid(&value), "the schema accepts {what}");
    }
}
