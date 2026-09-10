use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ThinkingPreference {
    #[default]
    Auto,
    ProviderDefault,
    Manual {
        level: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingDecision {
    pub preference: ThinkingPreference,
    pub requested_effort: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfiguration {
    pub model: String,
    pub supported_efforts: Vec<String>,
    pub configured_effort: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ThinkingError {
    #[error("thinking level must be 1 to 64 ASCII letters, digits, underscores or hyphens")]
    InvalidLevel,
    #[error("thinking configuration exceeds model or supported-level bounds")]
    InvalidConfiguration,
}

fn validate_level(level: &str) -> Result<(), ThinkingError> {
    if level.is_empty()
        || level.len() > 64
        || !level
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ThinkingError::InvalidLevel);
    }
    Ok(())
}

impl ThinkingPreference {
    pub fn validate(&self) -> Result<(), ThinkingError> {
        match self {
            Self::Manual { level } => validate_level(level),
            Self::Auto | Self::ProviderDefault => Ok(()),
        }
    }
}

impl ThinkingDecision {
    pub fn provider_default() -> Self {
        Self {
            preference: ThinkingPreference::ProviderDefault,
            requested_effort: None,
            reason: "Uses the provider's configured/default effort.".into(),
        }
    }

    pub fn resolve(preference: ThinkingPreference, text: &str) -> Result<Self, ThinkingError> {
        preference.validate()?;
        let (effort, reason) = match &preference {
            ThinkingPreference::ProviderDefault => return Ok(Self::provider_default()),
            ThinkingPreference::Manual { level } => (level.clone(), "Your manual choice."),
            ThinkingPreference::Auto => {
                let text = text.trim().to_ascii_lowercase();
                if matches!(
                    text.as_str(),
                    "hi" | "hi!" | "hello" | "hello!" | "hey" | "hey!"
                ) {
                    ("low".into(), "A short greeting needs little reasoning.")
                } else if bounded_transformation(&text) {
                    (
                        "medium".into(),
                        "A bounded text transformation needs moderate reasoning.",
                    )
                } else {
                    (
                        "high".into(),
                        "Quality-first default for coding, complex requests and follow-ups.",
                    )
                }
            }
        };
        Ok(Self {
            preference,
            requested_effort: Some(effort),
            reason: reason.into(),
        })
    }
}

fn bounded_transformation(text: &str) -> bool {
    if text.len() > 512 || text.contains(['\n', '\r', '`']) {
        return false;
    }
    let Some((instruction, content)) = text.split_once(':') else {
        return false;
    };
    if content.trim().is_empty() {
        return false;
    }
    matches!(
        instruction,
        "rewrite this sentence"
            | "capitalize this sentence"
            | "uppercase this sentence"
            | "lowercase this sentence"
    ) || instruction
        .strip_prefix("translate this sentence to ")
        .is_some_and(|language| {
            !language.is_empty()
                && language.len() <= 32
                && language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphabetic() || byte == b' ')
        })
}

impl ThinkingConfiguration {
    pub fn validate(&self) -> Result<(), ThinkingError> {
        if self.model.is_empty() || self.model.len() > 256 || self.supported_efforts.len() > 32 {
            return Err(ThinkingError::InvalidConfiguration);
        }
        for (index, level) in self.supported_efforts.iter().enumerate() {
            validate_level(level)?;
            if self.supported_efforts[..index].contains(level) {
                return Err(ThinkingError::InvalidConfiguration);
            }
        }
        if let Some(level) = &self.configured_effort {
            validate_level(level)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_auto_is_conservative_and_explains_each_choice() {
        for (text, effort, reason) in [
            ("Hi!", "low", "A short greeting needs little reasoning."),
            ("hello", "low", "A short greeting needs little reasoning."),
            (
                "Rewrite this sentence: The meeting is tomorrow.",
                "medium",
                "A bounded text transformation needs moderate reasoning.",
            ),
            (
                "Translate this sentence to French: Good morning.",
                "medium",
                "A bounded text transformation needs moderate reasoning.",
            ),
            (
                "go ahead",
                "high",
                "Quality-first default for coding, complex requests and follow-ups.",
            ),
            (
                "Fix the race in app.rs",
                "high",
                "Quality-first default for coding, complex requests and follow-ups.",
            ),
            (
                "Review this code",
                "high",
                "Quality-first default for coding, complex requests and follow-ups.",
            ),
            (
                "Hi, debug this error",
                "high",
                "Quality-first default for coding, complex requests and follow-ups.",
            ),
            (
                "Rewrite this entire module",
                "high",
                "Quality-first default for coding, complex requests and follow-ups.",
            ),
        ] {
            let decision = ThinkingDecision::resolve(ThinkingPreference::Auto, text).unwrap();
            assert_eq!(decision.requested_effort.as_deref(), Some(effort), "{text}");
            assert_eq!(decision.reason, reason);
        }
        let long = format!("Rewrite this sentence: {}", "word ".repeat(500));
        assert_eq!(
            ThinkingDecision::resolve(ThinkingPreference::Auto, &long)
                .unwrap()
                .requested_effort
                .as_deref(),
            Some("high")
        );
    }

    #[test]
    fn thinking_manual_preserves_native_levels_and_rejects_invalid_values() {
        for level in ["low", "medium", "high", "xhigh", "native_level-2"] {
            let preference = ThinkingPreference::Manual {
                level: level.into(),
            };
            let decision = ThinkingDecision::resolve(preference.clone(), "hi").unwrap();
            assert_eq!(decision.preference, preference);
            assert_eq!(decision.requested_effort.as_deref(), Some(level));
            assert_eq!(decision.reason, "Your manual choice.");
        }
        for level in ["", " high", "a.b", "é", &"a".repeat(65)] {
            assert!(
                ThinkingDecision::resolve(
                    ThinkingPreference::Manual {
                        level: level.into()
                    },
                    "hi"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn thinking_legacy_default_is_distinct_from_auto() {
        assert_eq!(ThinkingPreference::default(), ThinkingPreference::Auto);
        let legacy = ThinkingDecision::provider_default();
        assert_eq!(legacy.preference, ThinkingPreference::ProviderDefault);
        assert_eq!(legacy.requested_effort, None);
        assert_eq!(
            legacy.reason,
            "Uses the provider's configured/default effort."
        );
        let request: crate::providers::TurnRequest =
            serde_json::from_str(r#"{"prompt":"hi"}"#).unwrap();
        assert_eq!(request.thinking, legacy);
        assert_eq!(crate::providers::TurnRequest::new("hi").thinking, legacy);
    }

    #[test]
    fn thinking_configuration_rejects_unbounded_or_duplicate_metadata() {
        let valid = ThinkingConfiguration {
            model: "fixture-model".into(),
            supported_efforts: vec!["low".into(), "high".into()],
            configured_effort: Some("high".into()),
        };
        assert!(valid.validate().is_ok());
        assert!(
            ThinkingConfiguration {
                model: "m".repeat(257),
                ..valid.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            ThinkingConfiguration {
                supported_efforts: vec!["low".into(), "low".into()],
                ..valid.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            ThinkingConfiguration {
                configured_effort: Some("bad value".into()),
                ..valid.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            ThinkingConfiguration {
                supported_efforts: (0..33).map(|n| n.to_string()).collect(),
                ..valid
            }
            .validate()
            .is_err()
        );
    }
}
