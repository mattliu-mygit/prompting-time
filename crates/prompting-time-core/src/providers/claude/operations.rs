//! Selected text from Claude tool blocks, never arbitrary argument envelopes.
use serde_json::Value;

use crate::tool_operation::{MAX_OPERATION_BYTES, ToolOperation, ToolOperationStatus};

fn text(value: &str, truncated: &mut bool) -> String {
    let mut result = String::new();
    append(&mut result, value, truncated);
    result
}

fn append(target: &mut String, value: &str, truncated: &mut bool) {
    let mut end = value
        .len()
        .min(MAX_OPERATION_BYTES.saturating_sub(target.len()));
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    *truncated |= end != value.len();
    target.push_str(&value[..end]);
}

pub(super) fn input(name: &str, value: &Value) -> ToolOperation {
    let mut operation = ToolOperation::new(name, ToolOperationStatus::Running);
    let (action, target) = match name {
        "Bash" => ("Run", "command"),
        "Read" => ("Read", "file_path"),
        "Write" => ("Write", "file_path"),
        "Edit" => ("Edit", "file_path"),
        "NotebookEdit" => ("Edit", "notebook_path"),
        "Grep" => ("Search", "pattern"),
        "Glob" => ("Find files", "pattern"),
        "WebSearch" => ("Search", "query"),
        "WebFetch" => ("Fetch", "url"),
        _ => return operation,
    };
    if let Some(target) = value.get(target).and_then(Value::as_str) {
        let target = text(target, &mut operation.truncated);
        operation.title = format!("{action} {target}");
        operation.input = Some(target);
    }
    if matches!(name, "Grep" | "Glob") {
        operation.context = value
            .get("path")
            .and_then(Value::as_str)
            .map(|path| text(path, &mut operation.truncated));
    }
    if matches!(name, "Write" | "Edit") {
        operation.context = value
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| text(path, &mut operation.truncated));
        if name == "Write" {
            operation.input = value
                .get("content")
                .and_then(Value::as_str)
                .map(|content| text(content, &mut operation.truncated));
        } else if let (Some(old), Some(new)) = (
            value.get("old_string").and_then(Value::as_str),
            value.get("new_string").and_then(Value::as_str),
        ) {
            let mut change = String::new();
            for part in ["Old text:\n", old, "\nNew text:\n", new] {
                append(&mut change, part, &mut operation.truncated);
            }
            operation.input = Some(change);
        }
    }
    operation.bounded()
}

pub(super) fn result(name: &str, block: &Value) -> ToolOperation {
    let status = match block.get("is_error") {
        Some(Value::Bool(true)) => ToolOperationStatus::Failed,
        Some(Value::Bool(false)) | None => ToolOperationStatus::Succeeded,
        _ => ToolOperationStatus::Unknown,
    };
    let mut operation = ToolOperation::new(name, status);
    if let Some(content) = block.get("content").and_then(Value::as_str) {
        operation.output = Some(text(content, &mut operation.truncated));
    } else if let Some(content) = block.get("content").and_then(Value::as_array) {
        let mut output = content.is_empty().then(String::new);
        for block in content.iter().take(1024) {
            if block.get("type").and_then(Value::as_str) == Some("text")
                && let Some(part) = block.get("text").and_then(Value::as_str)
            {
                if let Some(output) = &mut output {
                    append(output, "\n", &mut operation.truncated);
                    append(output, part, &mut operation.truncated);
                } else {
                    output = Some(text(part, &mut operation.truncated));
                }
            }
        }
        operation.truncated |= content.len() > 1024;
        operation.output = output;
    }
    if status == ToolOperationStatus::Failed {
        operation.error = operation.output.clone();
    }
    operation.bounded()
}
