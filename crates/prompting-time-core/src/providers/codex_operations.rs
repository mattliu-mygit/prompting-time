use crate::tool_operation::{
    MAX_OPERATION_BYTES, MAX_OPERATION_TITLE_BYTES, ToolOperation, ToolOperationStatus as Status,
};
use serde_json::Value;

const MAX_SELECTED_ENTRIES: usize = 128;

/// Select display text only. Every string/collection allocation has a fixed byte/row bound.
pub(super) fn extract(item: &Value, completed: bool) -> Option<ToolOperation> {
    let kind = item["type"].as_str()?;
    if matches!(kind, "reasoning" | "agentMessage" | "userMessage") {
        return None;
    }
    let reported = item["status"].as_str();
    let status = match reported {
        Some("inProgress") if !completed => Status::Running,
        Some("completed") => Status::Succeeded,
        Some("failed") => Status::Failed,
        Some("interrupted") => Status::Interrupted,
        _ if !completed && reported.is_none() => Status::Running,
        _ => Status::Unknown,
    };
    let mut operation = ToolOperation::new("Tool", status);
    operation.conflicted = completed && reported == Some("inProgress");
    if reported == Some("declined") {
        operation.error = Some("Execution declined".into());
    }
    match kind {
        "commandExecution" => {
            operation.title = label("Run", item["command"].as_str(), &mut operation.truncated);
            operation.input = text(&item["command"], &mut operation.truncated);
            operation.context = text(&item["cwd"], &mut operation.truncated);
            operation.output = text(&item["aggregatedOutput"], &mut operation.truncated);
            operation.exit_code = item["exitCode"]
                .as_i64()
                .and_then(|code| code.try_into().ok());
            operation.duration_ms = item["durationMs"].as_u64();
            if let Some(actions) = item["commandActions"].as_array()
                && actions.len() == 1
            {
                let action = &actions[0];
                let title = match action["type"].as_str() {
                    Some("read") => Some(label(
                        "Read",
                        action["path"].as_str(),
                        &mut operation.truncated,
                    )),
                    Some("listFiles") => Some(label(
                        "List files",
                        action["path"].as_str(),
                        &mut operation.truncated,
                    )),
                    Some("search") => action["query"].as_str().map(|query| {
                        let query = clipped(
                            query,
                            MAX_OPERATION_TITLE_BYTES - 10,
                            &mut operation.truncated,
                        );
                        format!("Search \"{query}\"")
                    }),
                    _ => None,
                };
                if let Some(title) = title {
                    operation.title = title;
                }
            }
            if operation.exit_code.is_some_and(|code| code != 0) {
                if reported == Some("completed") {
                    // Completion describes the finished command; its explicit exit
                    // code is stronger outcome evidence than that generic label.
                    operation.status = Status::Failed;
                } else {
                    outcome(&mut operation, Status::Failed);
                }
            }
        }
        "fileChange" => {
            operation.title = "Change files".into();
            if let Some(changes) = item["changes"].as_array() {
                if changes.len() == 1 {
                    operation.title = label(
                        change_action(&changes[0]),
                        changes[0]["path"].as_str(),
                        &mut operation.truncated,
                    );
                }
                let mut output = String::new();
                operation.truncated |= changes.len() > MAX_SELECTED_ENTRIES;
                for change in changes.iter().take(MAX_SELECTED_ENTRIES) {
                    if !output.is_empty() {
                        append(&mut output, "\n", &mut operation.truncated);
                    }
                    append(&mut output, change_action(change), &mut operation.truncated);
                    append(&mut output, " ", &mut operation.truncated);
                    if let Some(path) = change["path"].as_str() {
                        append(&mut output, path, &mut operation.truncated);
                    }
                    if let Some(path) = change.pointer("/kind/move_path").and_then(Value::as_str) {
                        append(&mut output, " -> ", &mut operation.truncated);
                        append(&mut output, path, &mut operation.truncated);
                    }
                    if let Some(diff) = change["diff"].as_str() {
                        append(&mut output, "\n", &mut operation.truncated);
                        append(&mut output, diff, &mut operation.truncated);
                    }
                }
                operation.output = Some(output);
            }
        }
        "mcpToolCall" => {
            operation.title = label("Tool", item["tool"].as_str(), &mut operation.truncated);
            operation.context = text(&item["server"], &mut operation.truncated);
            operation.error =
                text(&item["error"]["message"], &mut operation.truncated).or(operation.error);
            operation.output =
                text_blocks(&item["result"]["content"], "text", &mut operation.truncated);
            operation.duration_ms = item["durationMs"].as_u64();
            if operation
                .error
                .as_ref()
                .is_some_and(|error| !error.is_empty())
            {
                outcome(&mut operation, Status::Failed);
            }
        }
        "dynamicToolCall" => {
            operation.title = label("Tool", item["tool"].as_str(), &mut operation.truncated);
            operation.context = text(&item["namespace"], &mut operation.truncated);
            operation.output =
                text_blocks(&item["contentItems"], "inputText", &mut operation.truncated);
            operation.duration_ms = item["durationMs"].as_u64();
            if let Some(success) = item["success"].as_bool() {
                outcome(
                    &mut operation,
                    if success {
                        Status::Succeeded
                    } else {
                        Status::Failed
                    },
                );
            }
        }
        "webSearch" => {
            operation.status = if completed {
                Status::Unknown
            } else {
                Status::Running
            };
            operation.title = label("Search", item["query"].as_str(), &mut operation.truncated);
            operation.input = text(&item["query"], &mut operation.truncated);
            let action = &item["action"];
            if let Some(action_type) = action["type"].as_str() {
                match action_type {
                    "openPage" => {
                        operation.title =
                            label("Open", action["url"].as_str(), &mut operation.truncated);
                        operation.context = text(&action["url"], &mut operation.truncated);
                    }
                    "findInPage" => {
                        operation.title =
                            label("Find", action["pattern"].as_str(), &mut operation.truncated);
                        operation.context = text(&action["url"], &mut operation.truncated);
                    }
                    _ => {}
                }
            }
        }
        _ => {
            operation.title = label("Tool", Some(kind), &mut operation.truncated);
        }
    }
    Some(operation.bounded())
}

fn outcome(operation: &mut ToolOperation, evidence: Status) {
    if operation.status == Status::Running
        || operation.status == Status::Unknown && !operation.conflicted
    {
        operation.status = evidence;
    } else if operation.status != evidence {
        operation.status = Status::Unknown;
        operation.conflicted = true;
    }
}

fn change_action(change: &Value) -> &'static str {
    match change.pointer("/kind/type").and_then(Value::as_str) {
        Some("add") => "Add",
        Some("delete") => "Delete",
        Some("update") => "Update",
        _ => "Change",
    }
}

fn clipped<'a>(value: &'a str, limit: usize, truncated: &mut bool) -> &'a str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    *truncated |= end < value.len();
    &value[..end]
}

fn text(value: &Value, truncated: &mut bool) -> Option<String> {
    value
        .as_str()
        .map(|value| clipped(value, MAX_OPERATION_BYTES, truncated).to_owned())
}

fn label(prefix: &str, value: Option<&str>, truncated: &mut bool) -> String {
    value.filter(|value| !value.is_empty()).map_or_else(
        || prefix.to_owned(),
        |value| {
            format!(
                "{prefix} {}",
                clipped(
                    value,
                    MAX_OPERATION_TITLE_BYTES.saturating_sub(prefix.len() + 1),
                    truncated
                )
            )
        },
    )
}

fn append(output: &mut String, value: &str, truncated: &mut bool) {
    output.push_str(clipped(
        value,
        MAX_OPERATION_BYTES.saturating_sub(output.len()),
        truncated,
    ));
}

fn text_blocks(value: &Value, kind: &str, truncated: &mut bool) -> Option<String> {
    let blocks = value.as_array()?;
    let mut output = String::new();
    let mut captured = blocks.is_empty();
    *truncated |= blocks.len() > MAX_SELECTED_ENTRIES;
    for block in blocks.iter().take(MAX_SELECTED_ENTRIES) {
        if block["type"].as_str() == Some(kind)
            && let Some(text) = block["text"].as_str()
        {
            if captured && !output.is_empty() {
                append(&mut output, "\n", truncated);
            }
            append(&mut output, text, truncated);
            captured = true;
        }
    }
    captured.then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_operation::ToolOperationStatus;
    use serde_json::json;

    #[test]
    fn tool_operation_action_metadata_is_used_only_for_one_known_action() {
        let mut item = json!({"type":"commandExecution", "command":"cat source", "status":"inProgress", "commandActions":[{"type":"read","path":"src/main.rs","name":"main.rs","command":"cat source"}]});
        assert_eq!(extract(&item, false).unwrap().title, "Read src/main.rs");
        item["commandActions"] =
            json!([{"type":"search","query":"routing","path":"src","command":"search"}]);
        assert_eq!(extract(&item, false).unwrap().title, "Search \"routing\"");
        item["commandActions"] = json!([{"type":"read","path":"one"},{"type":"read","path":"two"}]);
        assert_eq!(extract(&item, false).unwrap().title, "Run cat source");
    }

    #[test]
    fn tool_operation_file_and_text_results_are_allowlisted() {
        let file = extract(&json!({"type":"fileChange","status":"completed","changes":[{"path":"src/main.rs","kind":{"type":"update"},"diff":"-old\n+new"}]}), true).unwrap();
        assert_eq!(file.title, "Update src/main.rs");
        assert!(file.output.unwrap().contains("-old\n+new"));
        let mcp = extract(&json!({"type":"mcpToolCall","server":"docs","tool":"lookup","status":"completed","arguments":{"auth":"PRIVATE_AUTH"},"result":{"content":[{"type":"text","text":"selected"},{"type":"image","data":"PRIVATE_IMAGE"}],"structuredContent":{"secret":"PRIVATE_STRUCTURED"}}}), true).unwrap();
        assert_eq!(mcp.output.as_deref(), Some("selected"));
        assert!(mcp.input.is_none());
        assert!(!serde_json::to_string(&mcp).unwrap().contains("PRIVATE_"));
        let dynamic = extract(&json!({"type":"dynamicToolCall","tool":"lookup","status":"completed","success":true,"contentItems":[{"type":"inputText","text":"dynamic text"},{"type":"inputImage","imageUrl":"PRIVATE_IMAGE"}]}), true).unwrap();
        assert_eq!(dynamic.output.as_deref(), Some("dynamic text"));
    }

    #[test]
    fn tool_operation_absent_empty_unsupported_and_conflicting_evidence_remain_distinct() {
        for (result, expected) in [
            (json!(null), None),
            (json!({"content":[]}), Some("")),
            (json!({"content":[{"type":"image","data":"private"}]}), None),
        ] {
            assert_eq!(extract(&json!({"type":"mcpToolCall","tool":"lookup","server":"docs","status":"completed","result":result}), true).unwrap().output.as_deref(), expected);
        }
        let conflict = extract(&json!({"type":"dynamicToolCall","tool":"check","status":"completed","success":false,"contentItems":[{"type":"inputText","text":"broken"}]}), true).unwrap();
        assert_eq!(conflict.status, ToolOperationStatus::Unknown);
        assert!(conflict.conflicted);
        let failed = extract(
            &json!({"type":"commandExecution","command":"check","status":"completed","exitCode":1}),
            true,
        )
        .unwrap();
        assert_eq!(failed.status, ToolOperationStatus::Failed);
        assert!(!failed.conflicted);
        let declined = extract(
            &json!({"type":"commandExecution","command":"check","status":"declined"}),
            true,
        )
        .unwrap();
        assert_eq!(declined.status, ToolOperationStatus::Unknown);
        assert_eq!(declined.error.as_deref(), Some("Execution declined"));
    }

    #[test]
    fn tool_operation_reasoning_unknown_and_search_are_conservative() {
        assert!(
            extract(
                &json!({"type":"reasoning","content":"PRIVATE_REASONING_MARKER"}),
                true
            )
            .is_none()
        );
        let unknown = extract(&json!({"type":"futureTool","arguments":{"secret":"PRIVATE_ARGS"},"output":"PRIVATE_RESULT"}), true).unwrap();
        assert_eq!(unknown.status, ToolOperationStatus::Unknown);
        assert!(
            !serde_json::to_string(&unknown)
                .unwrap()
                .contains("PRIVATE_")
        );
        let search =
            json!({"type":"webSearch","query":"compiler","results":{"secret":"PRIVATE_RESULTS"}});
        assert_eq!(
            extract(&search, false).unwrap().status,
            ToolOperationStatus::Running
        );
        assert_eq!(
            extract(&search, true).unwrap().status,
            ToolOperationStatus::Unknown
        );
    }

    #[test]
    fn tool_operation_collection_and_display_buffers_are_bounded() {
        let item = json!({"type":"mcpToolCall","server":"docs","tool":"lookup","status":"completed","result":{"content":(0..300).map(|_| json!({"type":"text","text":"界".repeat(2000)})).collect::<Vec<_>>()}});
        let operation = extract(&item, true).unwrap();
        assert!(operation.truncated);
        assert!(operation.display_bytes() <= 256 * 1024);
        assert!(operation.output.unwrap().len() <= 256 * 1024);
    }
}
