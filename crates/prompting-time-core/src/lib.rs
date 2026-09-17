pub mod app;
pub mod context_budget;
pub mod domain;
pub mod error;
pub mod handoff;
mod owned_process;
pub mod providers;
pub mod router;
pub mod runtime;
pub mod store;
pub mod thinking;
pub mod tool_operation;
pub mod workspace;

#[cfg(test)]
mod context_budget_contract_tests {
    use crate::context_budget::ContextBudget;
    use crate::providers::{ResumeSession, StartSession, TurnRequest};

    #[test]
    fn context_budget_presets_and_legacy_requests() {
        assert_eq!(ContextBudget::default().requested_tokens(), Some(300_000));
        for tokens in [200_000, 300_000, 400_000, 500_000] {
            let budget = ContextBudget::Tokens { tokens };
            assert!(budget.validate().is_ok());
            assert_eq!(
                serde_json::from_value::<ContextBudget>(serde_json::to_value(budget).unwrap())
                    .unwrap(),
                budget
            );
        }
        assert!(
            ContextBudget::Tokens { tokens: 250_000 }
                .validate()
                .is_err()
        );
        assert!(ContextBudget::ProviderDefault.validate().is_ok());
        assert_eq!(ContextBudget::ProviderDefault.requested_tokens(), None);
        assert_eq!(
            serde_json::from_str::<TurnRequest>(r#"{"prompt":"old"}"#)
                .unwrap()
                .context_budget,
            ContextBudget::ProviderDefault
        );
        assert_eq!(
            TurnRequest::new("old").context_budget,
            ContextBudget::ProviderDefault
        );
        let turn =
            TurnRequest::new("new").with_context_budget(ContextBudget::Tokens { tokens: 500_000 });
        assert_eq!(
            serde_json::from_str::<TurnRequest>(&serde_json::to_string(&turn).unwrap()).unwrap(),
            turn
        );
        let old_session = serde_json::json!({ "conversationId": crate::domain::ConversationId::new(), "workingDirectory": "/tmp/fixture" });
        assert_eq!(
            serde_json::from_value::<StartSession>(old_session.clone())
                .unwrap()
                .context_budget,
            ContextBudget::ProviderDefault
        );
        assert_eq!(
            serde_json::from_value::<ResumeSession>(old_session)
                .unwrap()
                .context_budget,
            ContextBudget::ProviderDefault
        );
    }
}
