use serde::{Deserialize, Serialize};

pub const MAX_OPERATION_BYTES: usize = 256 * 1024;
pub const MAX_OPERATION_TITLE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolOperationStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
    /// No active indicator: the provider did not establish an outcome.
    Unknown,
}

/// Selected display fields only; native identity belongs to the event owner.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOperation {
    pub title: String,
    pub status: ToolOperationStatus,
    pub input: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
    pub context: Option<String>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub conflicted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOperationSummary {
    pub status: ToolOperationStatus,
    pub has_details: bool,
    pub detail_revision: u64,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub conflicted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOperationDetail {
    pub operation: ToolOperation,
    pub revision: u64,
}

impl ToolOperation {
    pub fn new(title: impl Into<String>, status: ToolOperationStatus) -> Self {
        Self {
            title: title.into(),
            status,
            input: None,
            output: None,
            error: None,
            context: None,
            duration_ms: None,
            exit_code: None,
            truncated: false,
            conflicted: false,
        }
    }

    pub fn display_bytes(&self) -> usize {
        [&self.input, &self.output, &self.error, &self.context]
            .into_iter()
            .fold(self.title.len(), |total, field| {
                total.saturating_add(field.as_ref().map_or(0, String::len))
            })
    }

    pub fn bounded(mut self) -> Self {
        self.truncated |= truncate(&mut self.title, MAX_OPERATION_TITLE_BYTES);
        let mut remaining = MAX_OPERATION_BYTES.saturating_sub(self.title.len());
        // Retain failure and input evidence before potentially enormous stdout.
        for field in [
            &mut self.error,
            &mut self.input,
            &mut self.context,
            &mut self.output,
        ]
        .into_iter()
        .flatten()
        {
            self.truncated |= truncate(field, remaining);
            remaining = remaining.saturating_sub(field.len());
        }
        self
    }

    /// Incoming fields are snapshots, never chunks. Absent fields do not erase evidence.
    pub fn merge(mut self, incoming: Self) -> Self {
        let incoming = incoming.bounded();
        let preserve_failure = self.conflicted
            || (self.status == ToolOperationStatus::Failed
                && incoming.status != ToolOperationStatus::Failed);
        let delayed_start = self.status != ToolOperationStatus::Running
            && incoming.status == ToolOperationStatus::Running;
        self.conflicted |= incoming.conflicted
            || (self.status != ToolOperationStatus::Running
                && incoming.status != ToolOperationStatus::Running
                && self.status != incoming.status);
        if self.conflicted {
            self.status = ToolOperationStatus::Unknown;
        } else if !delayed_start {
            self.status = incoming.status;
        }
        if !delayed_start {
            if !incoming.title.is_empty() {
                self.title = incoming.title;
            }
            if incoming.output.is_some() {
                self.output = incoming.output;
            }
            if incoming.error.is_some() && (!preserve_failure || self.error.is_none()) {
                self.error = incoming.error;
            }
            if incoming.duration_ms.is_some() {
                self.duration_ms = incoming.duration_ms;
            }
            if incoming.exit_code.is_some() && (!preserve_failure || self.exit_code.is_none()) {
                self.exit_code = incoming.exit_code;
            }
        }
        if incoming.input.is_some() && (!delayed_start || self.input.is_none()) {
            self.input = incoming.input;
        }
        if incoming.context.is_some() && (!delayed_start || self.context.is_none()) {
            self.context = incoming.context;
        }
        self.truncated |= incoming.truncated;
        self.bounded()
    }

    pub fn summary(&self, revision: u64) -> ToolOperationSummary {
        ToolOperationSummary {
            status: self.status,
            has_details: [&self.input, &self.output, &self.error, &self.context]
                .into_iter()
                .any(Option::is_some),
            detail_revision: revision,
            duration_ms: self.duration_ms,
            exit_code: self.exit_code,
            truncated: self.truncated,
            conflicted: self.conflicted,
        }
    }
}

fn truncate(value: &mut String, limit: usize) -> bool {
    if value.len() <= limit {
        return false;
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_snapshots_survive_delayed_starts_and_duplicates() {
        let mut done = ToolOperation::new("Run check", ToolOperationStatus::Succeeded);
        done.output = Some("READY\n".into());
        let merged = done.clone().merge(ToolOperation::new(
            "Run check",
            ToolOperationStatus::Running,
        ));
        assert_eq!(merged.status, ToolOperationStatus::Succeeded);
        assert_eq!(merged.output.as_deref(), Some("READY\n"));
        assert_eq!(merged.clone().merge(done), merged);
    }

    #[test]
    fn snapshots_replace_and_missing_is_distinct_from_empty() {
        let mut first = ToolOperation::new("Run", ToolOperationStatus::Running);
        first.input = Some(String::new());
        let mut next = first.clone();
        next.input = Some("check".into());
        next.output = Some(String::new());
        let merged = first.merge(next);
        assert_eq!(merged.input.as_deref(), Some("check"));
        assert_eq!(merged.output.as_deref(), Some(""));
    }

    #[test]
    fn conflicts_preserve_failure_evidence() {
        let mut failed = ToolOperation::new("Run", ToolOperationStatus::Failed);
        failed.error = Some("broken".into());
        let merged = failed.merge(ToolOperation::new("Run", ToolOperationStatus::Succeeded));
        assert_eq!(merged.status, ToolOperationStatus::Unknown);
        assert!(merged.conflicted);
        assert_eq!(merged.error.as_deref(), Some("broken"));
    }

    #[test]
    fn conflicting_success_cannot_erase_failure_fields() {
        let mut failed = ToolOperation::new("Run", ToolOperationStatus::Failed);
        failed.error = Some("broken".into());
        failed.exit_code = Some(1);
        let mut success = ToolOperation::new("Run", ToolOperationStatus::Succeeded);
        success.error = Some(String::new());
        success.exit_code = Some(0);
        let merged = failed.merge(success);
        assert_eq!(merged.error.as_deref(), Some("broken"));
        assert_eq!(merged.exit_code, Some(1));
    }

    #[test]
    fn repeated_failed_snapshot_can_enrich_error_and_exit() {
        let mut failed = ToolOperation::new("Run", ToolOperationStatus::Failed);
        failed.error = Some(String::new());
        let mut incoming = failed.clone();
        incoming.error = Some("actual failure".into());
        incoming.exit_code = Some(1);
        let merged = failed.merge(incoming);
        assert_eq!(merged.error.as_deref(), Some("actual failure"));
        assert_eq!(merged.exit_code, Some(1));
        assert!(!merged.conflicted);
    }

    #[test]
    fn combined_display_is_bounded_without_splitting_utf8() {
        let mut value = ToolOperation::new("é".repeat(1000), ToolOperationStatus::Running);
        value.output = Some("界".repeat(300_000));
        value.error = Some("error".repeat(100_000));
        let bounded = value.bounded();
        assert!(bounded.truncated);
        assert!(bounded.display_bytes() <= 256 * 1024);
        assert!(bounded.title.len() <= 1024);
        assert!(std::str::from_utf8(bounded.output.as_ref().unwrap().as_bytes()).is_ok());
        assert!(
            bounded
                .clone()
                .merge(ToolOperation::new("Run", ToolOperationStatus::Running))
                .truncated
        );
    }
}
