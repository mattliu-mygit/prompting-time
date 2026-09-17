//! Version-specific, owned-thread configuration. The reserved turn fences both reload and prompt.
use super::*;

pub(super) fn unsupported() -> ProviderError {
    ProviderError::NotDispatched {
        category: super::super::ProviderErrorCategory::UnsupportedContextBudget,
    }
}

#[derive(Default)]
pub(super) struct ReloadEvidence {
    unloaded: bool,
    idle: bool,
}

impl ReloadEvidence {
    pub(super) fn observe(&mut self, status: Option<&str>) {
        match status {
            Some("notLoaded") => {
                self.unloaded = true;
                self.idle = false;
            }
            Some("idle") if self.unloaded => self.idle = true,
            _ => self.idle = false,
        }
    }
    pub(super) fn complete(self) -> bool {
        self.unloaded && self.idle
    }
}

impl Client {
    async fn context_budget_reload(
        &self,
        thread_id: &str,
        registration_id: u64,
        finish: bool,
    ) -> Result<(), ProviderError> {
        let (response, receiver) = oneshot::channel();
        self.commands
            .send(ClientCommand::ContextBudgetReload {
                thread_id: thread_id.into(),
                registration_id,
                finish,
                response,
            })
            .await
            .map_err(|_| unsupported())?;
        receiver.await.map_err(|_| unsupported())?
    }
}

impl CodexAdapter {
    pub(super) fn supports_context_budget(&self) -> bool {
        let version = self.inner.version.lock().expect("version mutex");
        // initialize returns a user-agent, not just a semantic version.
        version
            .strip_prefix("codex-cli ")
            .or_else(|| version.strip_prefix("codex_cli_rs/"))
            .and_then(|s| s.split_whitespace().next())
            == Some("0.153.4")
    }

    pub(super) async fn configure_context_budget(
        &self,
        session: &ProviderSession,
        budget: ContextBudget,
        registration_id: u64,
    ) -> Result<(), ProviderError> {
        budget.validate().map_err(|_| unsupported())?;
        let metadata = self
            .inner
            .thinking_sessions
            .lock()
            .expect("thinking sessions mutex")
            .iter()
            .find(|(id, _)| id == &session.native_id)
            .map(|(_, meta)| meta.clone());
        let Some(metadata) = metadata else {
            return Err(unsupported());
        };
        if metadata.context_budget == Some(budget) {
            return Ok(());
        }
        if !self.supports_context_budget() {
            // This adapter could not previously install an override on an unsupported native
            // version. Provider default preserves its baseline behavior without a reload.
            return if budget == ContextBudget::ProviderDefault && metadata.context_budget.is_none()
            {
                Ok(())
            } else {
                Err(unsupported())
            };
        }
        // Invalidate before any uncertain RPC. Dropping this future cannot restore a stale claim.
        self.set_context_budget(&session.native_id, None);
        let transaction = async {
            let current = self
                .inner
                .client
                .request(
                    "thread/read",
                    json!({"threadId":session.native_id,"includeTurns":false}),
                )
                .await?;
            if current.pointer("/thread/id").and_then(Value::as_str) != Some(&session.native_id)
                || current
                    .pointer("/thread/status/type")
                    .and_then(Value::as_str)
                    != Some("idle")
            {
                return Err(unsupported());
            }
            self.inner
                .client
                .context_budget_reload(&session.native_id, registration_id, false)
                .await?;
            self.inner
                .client
                .request("thread/unsubscribe", json!({"threadId":session.native_id}))
                .await?;
            let config = budget.requested_tokens().map_or_else(
                || json!({}),
                |tokens| json!({"model_auto_compact_token_limit":tokens}),
            );
            let result = self.inner.client.request("thread/resume",json!({"threadId":session.native_id,"cwd":metadata.cwd,"approvalPolicy":"on-request","sandbox":"workspace-write","config":config})).await?;
            let resumed = parse_session(result.clone())?;
            if resumed != *session {
                return Err(unsupported());
            }
            self.inner
                .client
                .context_budget_reload(&session.native_id, registration_id, true)
                .await?;
            self.bind_thinking_session(result, &metadata.cwd, Some(budget))?;
            Ok(())
        };
        tokio::time::timeout(REQUEST_TIMEOUT, transaction)
            .await
            .map_err(|_| unsupported())?
            .map_err(|_| unsupported())
    }

    fn set_context_budget(&self, thread_id: &str, budget: Option<ContextBudget>) {
        if let Some((_, meta)) = self
            .inner
            .thinking_sessions
            .lock()
            .expect("thinking sessions mutex")
            .iter_mut()
            .find(|(id, _)| id == thread_id)
        {
            meta.context_budget = budget;
        }
    }
}
