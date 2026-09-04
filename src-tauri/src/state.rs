use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prompting_time_core::app::PromptingTime;
use prompting_time_core::providers::codex::CodexAdapter;
use prompting_time_core::providers::{
    ApprovalResponse, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderErrorCategory,
    ProviderHealth, ProviderId, ProviderInstallation, ProviderSession, ProviderTurn, ResumeSession,
    StartSession, TurnRequest, discover_provider,
};
use prompting_time_core::router::Router;
use prompting_time_core::store::Store;
use prompting_time_core::workspace::WorkspaceManager;
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use crate::commands::{APP_EVENT_NAME, AppEvent};

const DATABASE_FILE: &str = "prompting-time.sqlite3";
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_RECOVERY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppInvalidation {
    Conversation {
        conversation_id: prompting_time_core::domain::ConversationId,
    },
    Run {
        conversation_id: prompting_time_core::domain::ConversationId,
        run_id: prompting_time_core::domain::RunId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderDiagnostic {
    pub id: ProviderId,
    pub installed: bool,
    pub available: bool,
    pub version: Option<String>,
    pub diagnostic: Option<String>,
    pub action: Option<String>,
    pub capabilities: ProviderCapabilities,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupDiagnostic {
    pub code: &'static str,
    pub message: String,
    pub action: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("application services are unavailable")]
    Unavailable,
}

pub struct AppState {
    service: Mutex<Option<Arc<PromptingTime>>>,
    providers: Vec<ProviderDiagnostic>,
    startup_diagnostic: Option<StartupDiagnostic>,
    shutting_down: AtomicBool,
    event_shutdown: watch::Sender<bool>,
    event_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl AppState {
    pub fn failed(diagnostic: StartupDiagnostic) -> Arc<Self> {
        let (event_shutdown, _) = watch::channel(false);
        Arc::new(Self {
            service: Mutex::new(None),
            providers: unavailable_provider_diagnostics(),
            startup_diagnostic: Some(diagnostic),
            shutting_down: AtomicBool::new(false),
            event_shutdown,
            event_task: Mutex::new(None),
        })
    }

    pub async fn initialize(app_data_dir: PathBuf) -> Arc<Self> {
        match Self::try_initialize(&app_data_dir).await {
            Ok((service, providers)) => Arc::new(Self {
                event_shutdown: watch::channel(false).0,
                service: Mutex::new(Some(Arc::new(service))),
                providers,
                startup_diagnostic: None,
                shutting_down: AtomicBool::new(false),
                event_task: Mutex::new(None),
            }),
            Err(diagnostic) => Self::failed(diagnostic),
        }
    }

    async fn try_initialize(
        app_data_dir: &Path,
    ) -> Result<(PromptingTime, Vec<ProviderDiagnostic>), StartupDiagnostic> {
        let store = Store::open(&app_data_dir.join(DATABASE_FILE))
            .await
            .map_err(|_| StartupDiagnostic {
                code: "storage-error",
                message: "Prompting Time could not open its local conversation database."
                    .to_owned(),
                action: Some(
                    "Check that Application Support is writable, then restart Prompting Time."
                        .to_owned(),
                ),
            })?;

        let codex_installation = installation("codex", ProviderId::Codex).await;
        let claude_installation = installation("claude", ProviderId::Claude).await;
        let mut adapters: Vec<Arc<dyn ProviderAdapter>> = Vec::new();
        let codex = match &codex_installation {
            Ok(installation) => match codex_login_status().await {
                Ok(true) => match CodexAdapter::connect_with_initialization_timeout(
                    PathBuf::from("codex"),
                    Duration::from_secs(10),
                )
                .await
                {
                    Ok(adapter) => {
                        let capabilities = adapter.capabilities();
                        adapters.push(Arc::new(adapter));
                        available_diagnostic(installation, capabilities)
                    }
                    Err(error) => unavailable_installed_diagnostic(
                        installation,
                        "Codex App Server could not be initialized.",
                        provider_action(error),
                    ),
                },
                Ok(false) => unavailable_installed_diagnostic(
                    installation,
                    "Codex is installed but is not authenticated.",
                    "Run `codex login`, then restart Prompting Time.".to_owned(),
                ),
                Err(()) => unavailable_installed_diagnostic(
                    installation,
                    "Codex authentication status could not be verified.",
                    "Run `codex login status`, then restart Prompting Time.".to_owned(),
                ),
            },
            Err(diagnostic) => diagnostic.clone(),
        };
        let claude = match &claude_installation {
            Ok(installation) => ProviderDiagnostic {
                id: ProviderId::Claude,
                installed: true,
                available: false,
                version: installation.version.clone(),
                diagnostic: Some(
                    "Claude Code integration is unavailable until its authenticated protocol gate passes."
                        .to_owned(),
                ),
                action: Some(
                    "Run `claude` interactively, use `/login`, then rerun the protocol gate."
                        .to_owned(),
                ),
                capabilities: ProviderCapabilities::default(),
            },
            Err(diagnostic) => diagnostic.clone(),
        };

        if !adapters
            .iter()
            .any(|adapter| adapter.id() == ProviderId::Codex)
        {
            adapters.push(Arc::new(UnavailableAdapter::new(
                ProviderId::Codex,
                codex.diagnostic.as_deref().unwrap_or("codex-unavailable"),
            )));
        }
        adapters.push(Arc::new(UnavailableAdapter::new(
            ProviderId::Claude,
            claude.diagnostic.as_deref().unwrap_or("claude-unavailable"),
        )));

        let app = PromptingTime::new(
            store,
            Router::default(),
            WorkspaceManager::new(app_data_dir),
            adapters,
        )
        .map_err(|_| StartupDiagnostic {
            code: "runtime-error",
            message: "Prompting Time could not start its local run supervisor.".to_owned(),
            action: Some("Restart Prompting Time and inspect provider diagnostics.".to_owned()),
        })?;
        if !recover_or_shutdown(
            app.reconcile_startup(),
            STARTUP_RECOVERY_TIMEOUT,
            app.shutdown_with_grace(SHUTDOWN_TIMEOUT),
        )
        .await
        {
            return Err(StartupDiagnostic {
                code: "recovery-error",
                message: "Prompting Time could not reconcile unfinished local runs.".to_owned(),
                action: Some(
                    "Restart Prompting Time before submitting more work; existing data was retained."
                        .to_owned(),
                ),
            });
        }
        Ok((app, vec![codex, claude]))
    }

    pub fn service(&self) -> Result<Arc<PromptingTime>, StateError> {
        self.service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or(StateError::Unavailable)
    }

    pub fn providers(&self) -> &[ProviderDiagnostic] {
        &self.providers
    }

    pub fn startup_diagnostic(&self) -> Option<&StartupDiagnostic> {
        self.startup_diagnostic.as_ref()
    }

    pub fn start_event_forwarding(self: &Arc<Self>, app: AppHandle) {
        let Ok(service) = self.service() else {
            return;
        };
        let mut changes = service.subscribe_changes();
        let mut shutdown = self.event_shutdown.subscribe();
        let task = tauri::async_runtime::spawn(async move {
            let mut events = AppEventSequencer::new();
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                    change = changes.recv() => match change {
                        Ok(change) => {
                            let invalidation = match change.run_id {
                                Some(run_id) => AppInvalidation::Run {
                                    conversation_id: change.conversation_id,
                                    run_id,
                                },
                                None => AppInvalidation::Conversation {
                                    conversation_id: change.conversation_id,
                                },
                            };
                            events.emit(&app, invalidation);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            events.emit_reload(&app);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        });
        let previous = self
            .event_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(task);
        debug_assert!(previous.is_none(), "event forwarding starts only once");
    }

    pub async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.event_shutdown.send_replace(true);
        let service = self
            .service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(service) = service {
            let shutdown_service = Arc::clone(&service);
            let task =
                tauri::async_runtime::spawn(async move { shutdown_service.shutdown().await });
            if !matches!(finish_owned_task(task, SHUTDOWN_TIMEOUT).await, Ok(Ok(()))) {
                let _ = service.force_shutdown().await;
            }
        }
        let event_task = self
            .event_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(event_task) = event_task {
            let _ = finish_owned_task(event_task, SHUTDOWN_TIMEOUT).await;
        }
    }
}

async fn recover_or_shutdown<F, S>(recovery: F, deadline: Duration, shutdown: S) -> bool
where
    F: Future<Output = Result<usize, prompting_time_core::app::AppError>>,
    S: Future<Output = Result<(), prompting_time_core::app::AppError>>,
{
    if matches!(tokio::time::timeout(deadline, recovery).await, Ok(Ok(_))) {
        true
    } else {
        let _ = shutdown.await;
        false
    }
}

async fn finish_owned_task<T>(
    mut task: tauri::async_runtime::JoinHandle<T>,
    grace: Duration,
) -> Result<T, OwnedTaskError> {
    match tokio::time::timeout(grace, &mut task).await {
        Ok(result) => return result.map_err(|_| OwnedTaskError::Join),
        Err(_) => task.abort(),
    }
    let _ = task.await;
    Err(OwnedTaskError::TimedOut)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnedTaskError {
    TimedOut,
    Join,
}

struct AppEventSequencer {
    next_sequence: u64,
}

impl AppEventSequencer {
    fn new() -> Self {
        Self { next_sequence: 1 }
    }

    fn next(&mut self, invalidation: AppInvalidation) -> AppEvent {
        let (sequence, reload_required) = self.take_sequence();
        if reload_required {
            return AppEvent::ReloadRequired {
                sequence: sequence.to_string(),
            };
        }
        match invalidation {
            AppInvalidation::Conversation { conversation_id } => AppEvent::ConversationChanged {
                sequence: sequence.to_string(),
                conversation_id: conversation_id.to_string(),
            },
            AppInvalidation::Run {
                conversation_id,
                run_id,
            } => AppEvent::RunChanged {
                sequence: sequence.to_string(),
                conversation_id: conversation_id.to_string(),
                run_id: run_id.to_string(),
            },
        }
    }

    fn emit(&mut self, app: &AppHandle, invalidation: AppInvalidation) {
        let _ = app.emit(APP_EVENT_NAME, self.next(invalidation));
    }

    fn emit_reload(&mut self, app: &AppHandle) {
        let (sequence, _) = self.take_sequence();
        let _ = app.emit(
            APP_EVENT_NAME,
            AppEvent::ReloadRequired {
                sequence: sequence.to_string(),
            },
        );
    }

    fn take_sequence(&mut self) -> (u64, bool) {
        let sequence = self.next_sequence;
        if sequence == u64::MAX {
            self.next_sequence = 1;
            (sequence, true)
        } else {
            self.next_sequence = sequence + 1;
            (sequence, false)
        }
    }
}

async fn codex_login_status() -> Result<bool, ()> {
    let mut command = tokio::process::Command::new("codex");
    command.args(["login", "status"]).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    Ok(output.status.success())
}

struct UnavailableAdapter {
    id: ProviderId,
    category: String,
}

impl UnavailableAdapter {
    fn new(id: ProviderId, category: &str) -> Self {
        Self {
            id,
            category: category.to_owned(),
        }
    }

    fn rejected() -> ProviderError {
        ProviderError::NotDispatched {
            category: ProviderErrorCategory::Rejected,
        }
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for UnavailableAdapter {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth::Unavailable {
            category: self.category.clone(),
        })
    }

    async fn start_session(&self, _: StartSession) -> Result<ProviderSession, ProviderError> {
        Err(Self::rejected())
    }

    async fn resume_session(
        &self,
        _: &str,
        _: ResumeSession,
    ) -> Result<ProviderSession, ProviderError> {
        Err(Self::rejected())
    }

    async fn start_turn(
        &self,
        _: &ProviderSession,
        _: TurnRequest,
    ) -> Result<ProviderTurn, ProviderError> {
        Err(Self::rejected())
    }

    async fn steer(&self, _: &ProviderSession, _: &str, _: &str) -> Result<(), ProviderError> {
        Err(Self::rejected())
    }

    async fn respond(
        &self,
        _: &ProviderSession,
        _: &str,
        _: ApprovalResponse,
    ) -> Result<(), ProviderError> {
        Err(Self::rejected())
    }

    async fn interrupt(&self, _: &ProviderSession, _: &str) -> Result<(), ProviderError> {
        Err(Self::rejected())
    }
}

async fn installation(
    binary: &str,
    id: ProviderId,
) -> Result<ProviderInstallation, ProviderDiagnostic> {
    discover_provider(binary, id)
        .await
        .map_err(|error| ProviderDiagnostic {
            id,
            installed: false,
            available: false,
            version: None,
            diagnostic: Some(format!(
                "{binary} is not installed or could not be inspected."
            )),
            action: Some(match error {
                ProviderError::NotInstalled { .. } => {
                    format!("Install {binary} and restart Prompting Time.")
                }
                _ => format!("Run `{binary} --version` in Terminal, then restart Prompting Time."),
            }),
            capabilities: ProviderCapabilities::default(),
        })
}

fn available_diagnostic(
    installation: &ProviderInstallation,
    capabilities: ProviderCapabilities,
) -> ProviderDiagnostic {
    ProviderDiagnostic {
        id: installation.id,
        installed: installation.installed,
        available: true,
        version: installation.version.clone(),
        diagnostic: None,
        action: None,
        capabilities,
    }
}

fn unavailable_installed_diagnostic(
    installation: &ProviderInstallation,
    diagnostic: &str,
    action: String,
) -> ProviderDiagnostic {
    ProviderDiagnostic {
        id: installation.id,
        installed: installation.installed,
        available: false,
        version: installation.version.clone(),
        diagnostic: Some(diagnostic.to_owned()),
        action: Some(action),
        capabilities: ProviderCapabilities::default(),
    }
}

fn provider_action(error: ProviderError) -> String {
    match error {
        ProviderError::Protocol { .. } | ProviderError::NotDispatched { .. } => {
            "Update Codex and verify authentication, then restart Prompting Time.".to_owned()
        }
        _ => "Run `codex --version` and `codex login status`, then restart Prompting Time."
            .to_owned(),
    }
}

fn unavailable_provider_diagnostics() -> Vec<ProviderDiagnostic> {
    [ProviderId::Codex, ProviderId::Claude]
        .into_iter()
        .map(|id| ProviderDiagnostic {
            id,
            installed: false,
            available: false,
            version: None,
            diagnostic: Some(
                "Provider discovery was skipped because application initialization failed."
                    .to_owned(),
            ),
            action: Some("Resolve the startup diagnostic and restart Prompting Time.".to_owned()),
            capabilities: ProviderCapabilities::default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::AtomicUsize;

    use tempfile::tempdir;

    use super::*;

    struct LiveTask(Arc<AtomicUsize>);

    impl Drop for LiveTask {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn owned_task_shutdown_waits_for_graceful_completion() {
        let live = Arc::new(AtomicUsize::new(1));
        let guard = LiveTask(Arc::clone(&live));
        let task = tauri::async_runtime::spawn(async move {
            let _guard = guard;
        });

        let result = finish_owned_task(task, Duration::from_secs(1)).await;

        assert!(result.is_ok());
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn startup_recovery_deadline_runs_and_awaits_owned_fallback() {
        let live = Arc::new(AtomicUsize::new(1));
        let fallback_ran = Arc::new(AtomicBool::new(false));
        let guard = LiveTask(Arc::clone(&live));
        let observed_fallback = Arc::clone(&fallback_ran);
        let recovered = recover_or_shutdown(
            std::future::pending::<Result<usize, prompting_time_core::app::AppError>>(),
            Duration::from_millis(1),
            async move {
                let _guard = guard;
                observed_fallback.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await;

        assert!(!recovered);
        assert!(fallback_ran.load(Ordering::SeqCst));
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn owned_task_shutdown_aborts_and_awaits_after_grace_expires() {
        let live = Arc::new(AtomicUsize::new(1));
        let guard = LiveTask(Arc::clone(&live));
        let task = tauri::async_runtime::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });

        let result = finish_owned_task(task, Duration::from_millis(10)).await;

        assert!(matches!(result, Err(OwnedTaskError::TimedOut)));
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn owned_task_shutdown_preserves_inner_errors_and_join_failures() {
        let inner_error = tauri::async_runtime::spawn(async {
            Err::<(), _>(prompting_time_core::app::AppError::EmptySubmission)
        });
        assert!(matches!(
            finish_owned_task(inner_error, Duration::from_secs(1)).await,
            Ok(Err(prompting_time_core::app::AppError::EmptySubmission))
        ));

        let panic = tauri::async_runtime::spawn(async { panic!("fixture panic") });
        assert!(matches!(
            finish_owned_task(panic, Duration::from_secs(1)).await,
            Err(OwnedTaskError::Join)
        ));
    }

    #[tokio::test]
    async fn initialization_failure_is_preserved_for_bootstrap() {
        let temporary = tempdir().unwrap();
        let not_a_directory = temporary.path().join("file");
        fs::write(&not_a_directory, "fixture").unwrap();

        let state = AppState::initialize(not_a_directory).await;

        assert!(state.service().is_err());
        assert_eq!(
            state.startup_diagnostic().map(|error| error.code),
            Some("storage-error")
        );
        assert_eq!(state.providers().len(), 2);
    }

    #[test]
    fn one_sequencer_orders_all_invalidation_sources() {
        let conversation_id = prompting_time_core::domain::ConversationId::new();
        let run_id = prompting_time_core::domain::RunId::new();
        let mut sequencer = AppEventSequencer::new();

        assert_eq!(
            sequencer.next(AppInvalidation::Conversation { conversation_id }),
            AppEvent::ConversationChanged {
                sequence: "1".to_owned(),
                conversation_id: conversation_id.to_string(),
            }
        );
        assert_eq!(
            sequencer.next(AppInvalidation::Run {
                conversation_id,
                run_id,
            }),
            AppEvent::RunChanged {
                sequence: "2".to_owned(),
                conversation_id: conversation_id.to_string(),
                run_id: run_id.to_string(),
            }
        );
    }

    #[test]
    fn sequence_exhaustion_emits_reload_before_resetting_without_wrapping() {
        let conversation_id = prompting_time_core::domain::ConversationId::new();
        let mut sequencer = AppEventSequencer {
            next_sequence: u64::MAX,
        };

        assert_eq!(
            sequencer.next(AppInvalidation::Conversation { conversation_id }),
            AppEvent::ReloadRequired {
                sequence: u64::MAX.to_string(),
            }
        );
        assert_eq!(
            sequencer.next(AppInvalidation::Conversation { conversation_id }),
            AppEvent::ConversationChanged {
                sequence: "1".to_owned(),
                conversation_id: conversation_id.to_string(),
            }
        );
    }

    #[tokio::test]
    async fn unavailable_providers_remain_visible_to_routing() {
        let temporary = tempdir().unwrap();
        let store = Store::open_in_memory().await.unwrap();
        let adapters: Vec<Arc<dyn ProviderAdapter>> = vec![
            Arc::new(UnavailableAdapter::new(
                ProviderId::Codex,
                "not-authenticated",
            )),
            Arc::new(UnavailableAdapter::new(ProviderId::Claude, "protocol-gate")),
        ];
        let app = PromptingTime::new(
            store,
            Router::default(),
            WorkspaceManager::new(temporary.path()),
            adapters,
        )
        .unwrap();
        let conversation = app
            .create_conversation(prompting_time_core::app::ConversationRequest::projectless(
                "routing fixture",
            ))
            .await
            .unwrap();

        let error = match app
            .submit(prompting_time_core::app::SubmitRequest {
                command_id: "command-1".to_owned(),
                conversation_id: conversation.id,
                content: "hello".to_owned(),
                provider_override: None,
            })
            .await
        {
            Ok(_) => panic!("unavailable providers must not receive a turn"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            prompting_time_core::app::AppError::Routing(
                prompting_time_core::router::RoutingError::NoEligibleProviders { evaluations }
            ) if evaluations.len() == 2
        ));
    }
}
