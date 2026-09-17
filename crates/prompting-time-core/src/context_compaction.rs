use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionObservation {
    pub native_session_id: String,
    pub native_turn_id: Option<String>,
    pub native_agent_id: Option<String>,
    pub phase: CompactionPhase,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CompactionPhase {
    Started,
    Cleared,
    Failed,
    Completed {
        #[serde(rename = "boundaryId")]
        boundary_id: String,
    },
}

impl CompactionObservation {
    pub fn validate(&self) -> Result<(), CompactionObservationError> {
        for id in [
            Some(self.native_session_id.as_str()),
            self.native_turn_id.as_deref(),
            self.native_agent_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                return Err(CompactionObservationError);
            }
        }
        // A Codex boundary is a JSON tuple of two bounded native identifiers.
        if let CompactionPhase::Completed { boundary_id } = &self.phase
            && (boundary_id.is_empty()
                || boundary_id.len() > 4096
                || boundary_id.chars().any(char::is_control))
        {
            return Err(CompactionObservationError);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Invalid compaction identity")]
pub struct CompactionObservationError;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionActivity {
    pub run_id: crate::domain::RunId,
    pub agent_id: crate::domain::AgentId,
    pub provider: crate::providers::ProviderId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_identifiers_are_bounded_before_ingress() {
        let valid = CompactionObservation {
            native_session_id: "session".into(),
            native_turn_id: Some("turn".into()),
            native_agent_id: None,
            phase: CompactionPhase::Started,
        };
        assert!(valid.validate().is_ok());
        for invalid in ["".to_owned(), "x".repeat(257), "bad\nidentity".into()] {
            let mut observation = valid.clone();
            observation.native_session_id = invalid.clone();
            assert!(observation.validate().is_err());
            observation = valid.clone();
            observation.native_turn_id = Some(invalid.clone());
            assert!(observation.validate().is_err());
            observation = valid.clone();
            observation.native_agent_id = Some(invalid);
            assert!(observation.validate().is_err());
        }
        for boundary_id in ["".to_owned(), "x".repeat(4097)] {
            let mut observation = valid.clone();
            observation.phase = CompactionPhase::Completed { boundary_id };
            assert!(observation.validate().is_err());
        }
        let mut tuple = valid;
        tuple.phase = CompactionPhase::Completed {
            boundary_id: serde_json::to_string(&("\\".repeat(256), "\"".repeat(256))).unwrap(),
        };
        assert!(tuple.validate().is_ok());
    }
}
