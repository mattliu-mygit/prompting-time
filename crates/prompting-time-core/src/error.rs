use thiserror::Error;

use crate::domain::{AgentId, RunId};

#[derive(Debug, Error, Eq, PartialEq)]
pub enum DomainError {
    #[error("cannot transition {entity} from {from} to {to}")]
    InvalidTransition {
        entity: &'static str,
        from: &'static str,
        to: &'static str,
    },
    #[error("agent {root} is not present")]
    RootNotFound { root: AgentId },
    #[error("agent {agent} appears more than once")]
    DuplicateAgent { agent: AgentId },
    #[error("agent {agent} references missing parent {parent}")]
    MissingParent { agent: AgentId, parent: AgentId },
    #[error("agent {agent} in run {run} has parent {parent} in run {parent_run}")]
    ParentRunMismatch {
        agent: AgentId,
        parent: AgentId,
        run: RunId,
        parent_run: RunId,
    },
    #[error("agent tree contains a cycle at {agent}")]
    AgentCycle { agent: AgentId },
}
