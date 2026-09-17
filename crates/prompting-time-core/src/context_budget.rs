use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Requested native automatic-compaction budget, not an effective model limit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ContextBudget {
    ProviderDefault,
    Tokens { tokens: u32 },
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self::Tokens { tokens: 300_000 }
    }
}

impl ContextBudget {
    pub fn provider_default() -> Self {
        Self::ProviderDefault
    }

    pub fn validate(self) -> Result<(), ContextBudgetError> {
        match self {
            Self::ProviderDefault
            | Self::Tokens {
                tokens: 200_000 | 300_000 | 400_000 | 500_000,
            } => Ok(()),
            Self::Tokens { .. } => Err(ContextBudgetError),
        }
    }

    pub fn requested_tokens(self) -> Option<u32> {
        match self {
            Self::ProviderDefault => None,
            Self::Tokens { tokens } => Some(tokens),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("Choose a context budget of 200k, 300k, 400k, 500k, or Provider default.")]
pub struct ContextBudgetError;
