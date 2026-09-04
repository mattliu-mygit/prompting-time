use sha2::{Digest, Sha256};

use crate::providers::ProviderId;
use crate::router::{RoutingReason, TaskKind};
use crate::workspace::WorkspaceSnapshot;

const BOUNDARY: &str =
    "Imported context from Prompting Time; provider-native history remains separate.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandoffMessage {
    pub role: HandoffRole,
    pub content: String,
}

impl HandoffMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: HandoffRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: HandoffRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HandoffInput {
    pub objective: String,
    pub current_request: String,
    pub constraints: Vec<String>,
    pub decisions: Vec<DurableDecision>,
    pub child_agent_outcomes: Vec<ChildAgentOutcome>,
    pub workspace_state: Option<WorkspaceSnapshot>,
    pub messages: Vec<HandoffMessage>,
    pub unresolved_failure: Option<UnresolvedFailure>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurableDecision {
    pub provider: ProviderId,
    pub reason: RoutingReason,
    pub task_kind: TaskKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildAgentOutcome {
    pub provider: ProviderId,
    pub provider_native_id: String,
    pub summary: Option<String>,
    pub status: ChildAgentStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildAgentStatus {
    Pending,
    Running,
    Waiting,
    Interrupted,
    Completed,
    Errored,
    Shutdown,
    NotFound,
    Unrecognized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnresolvedFailure {
    ProviderUnavailable,
    ProviderRejectedBeforeDispatch,
    ProviderStateUnknown,
    WorkspaceChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandoffCapsule {
    pub rendered: String,
    pub content_hash: String,
    pub messages: Vec<HandoffMessage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandoffBuilder {
    max_chars: usize,
}

impl HandoffBuilder {
    pub fn new(max_chars: usize) -> Self {
        Self { max_chars }
    }

    pub fn build(&self, input: HandoffInput) -> Result<HandoffCapsule, HandoffError> {
        if self.max_chars == 0 {
            return Err(HandoffError::EmptyBudget);
        }

        let mut reserved = render_required(&input);
        if reserved.chars().count() > self.max_chars {
            return Err(HandoffError::RequiredContextExceedsBudget {
                required_chars: reserved.chars().count(),
                budget_chars: self.max_chars,
            });
        }
        if let Some(failure) = input.unresolved_failure {
            append_if_fits(
                &mut reserved,
                "Unresolved failure",
                &[failure.safe_summary().to_owned()],
                self.max_chars,
            );
        }
        append_optional_sections(&mut reserved, render_optional(&input), self.max_chars);

        let mut included_reversed = Vec::new();
        let mut rendered_messages_reversed = Vec::new();

        for message in input.messages.iter().rev() {
            let rendered = render_message(message);
            let candidate = std::iter::once(rendered.as_str())
                .chain(rendered_messages_reversed.iter().rev().map(String::as_str));
            let candidate_chars = required_with_messages(&reserved, candidate).chars().count();
            if candidate_chars <= self.max_chars {
                included_reversed.push(message.clone());
                rendered_messages_reversed.push(rendered);
            } else {
                break;
            }
        }

        included_reversed.reverse();
        rendered_messages_reversed.reverse();
        let rendered = required_with_messages(
            &reserved,
            rendered_messages_reversed.iter().map(String::as_str),
        );
        let content_hash = format!("{:x}", Sha256::digest(rendered.as_bytes()));

        Ok(HandoffCapsule {
            rendered,
            content_hash,
            messages: included_reversed,
        })
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum HandoffError {
    #[error("handoff budget must be greater than zero")]
    EmptyBudget,
    #[error(
        "required handoff context is {required_chars} characters, exceeding budget {budget_chars}"
    )]
    RequiredContextExceedsBudget {
        required_chars: usize,
        budget_chars: usize,
    },
}

fn render_required(input: &HandoffInput) -> String {
    [
        BOUNDARY.to_owned(),
        section("Objective", std::slice::from_ref(&input.objective)),
        section(
            "Current request",
            std::slice::from_ref(&input.current_request),
        ),
        section("Constraints", &input.constraints),
    ]
    .join("\n\n")
}

struct OptionalSection {
    title: &'static str,
    lines: Vec<String>,
    omission: &'static str,
}

fn render_optional(input: &HandoffInput) -> Vec<OptionalSection> {
    let mut sections = Vec::new();
    if !input.decisions.is_empty() {
        sections.push(OptionalSection {
            title: "Durable decisions",
            lines: input.decisions.iter().map(render_decision).collect(),
            omission: "Additional durable decisions were omitted.",
        });
    }
    if !input.child_agent_outcomes.is_empty() {
        sections.push(OptionalSection {
            title: "Child-agent outcomes",
            lines: input
                .child_agent_outcomes
                .iter()
                .map(render_child_outcome)
                .collect(),
            omission: "Additional child-agent outcomes were omitted.",
        });
    }
    if let Some(workspace) = &input.workspace_state {
        sections.push(OptionalSection {
            title: "Workspace state",
            lines: workspace.safe_lines(),
            omission: "Additional workspace state was omitted.",
        });
    }
    sections
}

fn append_optional_sections(
    rendered: &mut String,
    sections: Vec<OptionalSection>,
    max_chars: usize,
) {
    if sections.is_empty() {
        return;
    }
    let full = sections
        .iter()
        .map(|value| section(value.title, &value.lines))
        .collect::<Vec<_>>();
    let full_candidate = format!("{rendered}\n\n{}", full.join("\n\n"));
    if full_candidate.chars().count() <= max_chars {
        *rendered = full_candidate;
        return;
    }

    let section_count = sections.len();
    for (index, value) in sections.into_iter().enumerate() {
        let sections_left = section_count - index;
        let remaining = max_chars.saturating_sub(rendered.chars().count());
        let separators = sections_left.saturating_mul(2);
        let share = remaining.saturating_sub(separators) / sections_left;
        if let Some(section) = bounded_section(&value, share) {
            rendered.push_str("\n\n");
            rendered.push_str(&section);
        }
    }
}

fn bounded_section(value: &OptionalSection, max_chars: usize) -> Option<String> {
    let header = format!("## {}", value.title);
    if header.chars().count() > max_chars {
        return None;
    }
    let mut rendered = header;
    for (index, line) in value.lines.iter().enumerate() {
        let candidate = format!("{rendered}\n- {line}");
        let has_more = index + 1 < value.lines.len();
        let with_omission = format!("{candidate}\n- {}", value.omission);
        if candidate.chars().count() <= max_chars
            && (!has_more || with_omission.chars().count() <= max_chars)
        {
            rendered = candidate;
            continue;
        }
        let omission = format!("{rendered}\n- {}", value.omission);
        if omission.chars().count() <= max_chars {
            rendered = omission;
        }
        return Some(rendered);
    }
    Some(rendered)
}

fn append_if_fits(rendered: &mut String, title: &str, lines: &[String], max_chars: usize) {
    let candidate = format!("{rendered}\n\n{}", section(title, lines));
    if candidate.chars().count() <= max_chars {
        *rendered = candidate;
    }
}

fn render_decision(decision: &DurableDecision) -> String {
    format!(
        "{} selected for a {} task by {}.",
        provider_name(decision.provider),
        task_kind_name(decision.task_kind),
        routing_reason_name(decision.reason)
    )
}

fn render_child_outcome(outcome: &ChildAgentOutcome) -> String {
    let identity = serde_json::to_string(&outcome.provider_native_id)
        .expect("a Rust string always serializes to JSON");
    let mut rendered = format!(
        "{} child agent {identity} {}.",
        provider_name(outcome.provider),
        child_status_name(outcome.status)
    );
    if let Some(summary) = &outcome.summary {
        let summary = serde_json::to_string(summary).expect("a Rust string always serializes");
        rendered.push_str(&format!(" Summary: {summary}."));
    }
    rendered
}

fn provider_name(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Codex => "Codex",
        ProviderId::Claude => "Claude",
    }
}

fn task_kind_name(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Implementation => "implementation",
        TaskKind::Review => "review",
        TaskKind::Research => "research",
        TaskKind::General => "general",
    }
}

fn routing_reason_name(reason: RoutingReason) -> &'static str {
    match reason {
        RoutingReason::ManualOverride => "manual override",
        RoutingReason::RequiredCapabilities => "required capabilities",
        RoutingReason::Continuity => "provider continuity",
        RoutingReason::OnlyEligibleProvider => "only eligible provider",
        RoutingReason::LeastUsed => "usage balancing",
        RoutingReason::DeterministicTieBreak => "deterministic tie-break",
        RoutingReason::SafeFallback => "safe fallback",
    }
}

fn child_status_name(status: ChildAgentStatus) -> &'static str {
    match status {
        ChildAgentStatus::Pending => "is pending",
        ChildAgentStatus::Running => "is running",
        ChildAgentStatus::Waiting => "is waiting",
        ChildAgentStatus::Interrupted => "was interrupted",
        ChildAgentStatus::Completed => "completed",
        ChildAgentStatus::Errored => "failed",
        ChildAgentStatus::Shutdown => "shut down",
        ChildAgentStatus::NotFound => "was not found",
        ChildAgentStatus::Unrecognized => "has an unrecognized status",
    }
}

fn section(title: &str, values: &[String]) -> String {
    let body = if values.is_empty() {
        "(none)".to_owned()
    } else {
        values
            .iter()
            .map(|value| format!("- {value}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!("## {title}\n{body}")
}

fn render_message(message: &HandoffMessage) -> String {
    let role = match message.role {
        HandoffRole::User => "User",
        HandoffRole::Assistant => "Assistant",
    };
    format!("{role}: {}", message.content)
}

impl UnresolvedFailure {
    fn safe_summary(self) -> &'static str {
        match self {
            Self::ProviderUnavailable => "The previous provider became unavailable.",
            Self::ProviderRejectedBeforeDispatch => {
                "The previous provider rejected the request before dispatch."
            }
            Self::ProviderStateUnknown => {
                "The previous provider may have partially handled the request."
            }
            Self::WorkspaceChanged => "The workspace changed during the previous provider attempt.",
        }
    }
}

fn required_with_messages<'a>(required: &str, messages: impl Iterator<Item = &'a str>) -> String {
    let mut sections = vec![required.to_owned()];
    let messages = messages.collect::<Vec<_>>();
    if !messages.is_empty() {
        let message_section = format!("## Recent visible messages\n{}", messages.join("\n\n"));
        sections.push(message_section);
    }
    sections.join("\n\n")
}
