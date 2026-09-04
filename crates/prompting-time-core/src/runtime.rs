use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
use tokio::sync::Barrier;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
use tokio::task::{Id, JoinHandle, JoinSet};
use uuid::Uuid;

use crate::domain::{
    AgentId, ApprovalResolution, ApprovalResponseIntentStatus, ConversationId, MutationState,
    RunId, RunStatus,
};
use crate::providers::{
    ApprovalResponse, DispatchCertainty, ProviderAdapter, ProviderError, ProviderErrorCategory,
    ProviderEvent, ProviderId, ProviderSession, ProviderTurn, ResumeSession, StartSession,
    TurnRequest,
};
use crate::router::RoutingDecision;
use crate::store::{
    NewFallbackAttempt, NewSubmission, PreparedSubmission, ProviderEventRecord,
    StageWaitingEventOutcome, Store, StoreError,
};

pub const MAX_CONCURRENT_ROOT_RUNS: usize = 4;
pub const MAX_QUEUED_ROOT_RUNS: usize = 64;
pub const MAX_CONCURRENT_APPROVAL_RESPONSES: usize = 4;
pub const MAX_APPROVAL_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_ADMITTED_ROOT_RUNS: usize = MAX_CONCURRENT_ROOT_RUNS + MAX_QUEUED_ROOT_RUNS;
const SUPERVISOR_COMMAND_CAPACITY: usize =
    MAX_ADMITTED_ROOT_RUNS + MAX_CONCURRENT_APPROVAL_RESPONSES;
const TERMINAL_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const RESPONSE_ACK_GRACE_TIMEOUT: Duration = Duration::from_millis(500);
const TURN_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const FORCED_OWNER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(not(test))]
pub(crate) const DISPATCH_LEASE_DURATION: Duration = Duration::from_secs(120);
#[cfg(test)]
pub(crate) const DISPATCH_LEASE_DURATION: Duration = Duration::from_millis(300);
#[cfg(not(test))]
const DISPATCH_LEASE_REFRESH_INTERVAL: Duration = Duration::from_secs(15);
#[cfg(test)]
const DISPATCH_LEASE_REFRESH_INTERVAL: Duration = Duration::from_millis(50);
pub(crate) const DISPATCH_LEASE_STALE_GRACE: Duration = Duration::from_secs(300);

#[cfg(test)]
struct ResponseIntentBarrier {
    committed: Notify,
    release: Notify,
}

#[cfg(test)]
struct ResponseAcknowledgementBarrier {
    committed: Notify,
    release: Notify,
    panic_after_release: bool,
}

#[cfg(test)]
struct ResponsePreAcknowledgementBarrier {
    ready: Notify,
    release: Notify,
}

#[cfg(test)]
impl ResponsePreAcknowledgementBarrier {
    fn new() -> Self {
        Self {
            ready: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[cfg(test)]
impl ResponseAcknowledgementBarrier {
    fn new() -> Self {
        Self {
            committed: Notify::new(),
            release: Notify::new(),
            panic_after_release: false,
        }
    }

    fn panicking() -> Self {
        Self {
            committed: Notify::new(),
            release: Notify::new(),
            panic_after_release: true,
        }
    }
}

#[cfg(test)]
impl ResponseIntentBarrier {
    fn new() -> Self {
        Self {
            committed: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[cfg(test)]
struct TerminalReceiptBarrier {
    received: Notify,
    release: Notify,
}

#[cfg(test)]
impl TerminalReceiptBarrier {
    fn new() -> Self {
        Self {
            received: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[cfg(test)]
struct ActiveTurnPanicBarrier {
    started: Notify,
    release: Notify,
}

#[cfg(test)]
struct OwnedTaskCompletionBarrier {
    completed: Notify,
    release: Notify,
}

#[cfg(test)]
struct FallbackCreationBarrier {
    created: Notify,
    release: Notify,
}

#[cfg(test)]
struct FallbackTransitionBarrier {
    ready: Notify,
    release: Notify,
}

#[cfg(test)]
struct RecoveryClaimBarrier {
    ready: Notify,
    release: Notify,
}

#[cfg(test)]
struct RecoveryPromotionBarrier {
    ready: Notify,
    release: Notify,
}

#[cfg(test)]
impl RecoveryClaimBarrier {
    fn new() -> Self {
        Self {
            ready: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[cfg(test)]
impl RecoveryPromotionBarrier {
    fn new() -> Self {
        Self {
            ready: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[cfg(test)]
struct QueuedInterruptBarrier {
    ready: Notify,
    release: Notify,
    finished: Notify,
}

#[cfg(test)]
impl FallbackCreationBarrier {
    fn new() -> Self {
        Self {
            created: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[cfg(test)]
impl FallbackTransitionBarrier {
    fn new() -> Self {
        Self {
            ready: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[cfg(test)]
impl QueuedInterruptBarrier {
    fn new() -> Self {
        Self {
            ready: Notify::new(),
            release: Notify::new(),
            finished: Notify::new(),
        }
    }
}

#[cfg(test)]
impl OwnedTaskCompletionBarrier {
    fn new() -> Self {
        Self {
            completed: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[cfg(test)]
impl ActiveTurnPanicBarrier {
    fn new() -> Self {
        Self {
            started: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunRequest {
    conversation_id: ConversationId,
    working_directory: PathBuf,
    provider: ProviderId,
    fallback: Option<FallbackRequest>,
    native_session_id: Option<String>,
    turn: TurnRequest,
}

#[derive(Clone, Debug)]
pub(crate) struct FallbackRequest {
    pub provider: ProviderId,
    pub native_session_id: Option<String>,
    pub turn: TurnRequest,
    pub handoff_rendered: Option<String>,
    pub handoff_hash: Option<String>,
    pub routing_decision: Option<Box<RoutingDecision>>,
}

impl RunRequest {
    pub fn new(
        conversation_id: ConversationId,
        working_directory: PathBuf,
        provider: ProviderId,
        turn: TurnRequest,
    ) -> Self {
        Self {
            conversation_id,
            working_directory,
            provider,
            fallback: None,
            native_session_id: None,
            turn,
        }
    }

    pub fn with_fallback(mut self, provider: ProviderId) -> Self {
        self.fallback = Some(FallbackRequest {
            provider,
            native_session_id: None,
            turn: self.turn.clone(),
            handoff_rendered: None,
            handoff_hash: None,
            routing_decision: None,
        });
        self
    }

    pub(crate) fn with_fallback_request(mut self, fallback: FallbackRequest) -> Self {
        self.fallback = Some(fallback);
        self
    }

    pub fn resume(mut self, native_session_id: impl Into<String>) -> Self {
        self.native_session_id = Some(native_session_id.into());
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunOutcome {
    pub primary_run_id: RunId,
    pub fallback_run_id: Option<RunId>,
    pub terminal_run_id: RunId,
    pub status: RunStatus,
}

#[derive(Clone, Copy)]
struct OperationState {
    current_run_id: RunId,
    fallback_run_id: Option<RunId>,
    status: RunStatus,
    reconciliation_failed: bool,
}

pub struct RunHandle {
    primary_run_id: RunId,
    state: watch::Receiver<OperationState>,
}

pub(crate) struct PreparedRunHandle {
    pub handle: RunHandle,
    pub duplicate: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct RecoveryClaim {
    run_id: RunId,
    owner_id: String,
}

impl RecoveryClaim {
    pub(crate) fn owner_id(&self) -> &str {
        &self.owner_id
    }
}

impl RunHandle {
    pub fn run_id(&self) -> RunId {
        self.primary_run_id
    }

    pub fn status(&self) -> RunStatus {
        self.state.borrow().status
    }

    pub async fn wait_for(&self, expected: RunStatus) -> Result<(), RuntimeError> {
        let mut state = self.state.clone();
        loop {
            let snapshot = *state.borrow();
            if snapshot.reconciliation_failed {
                return Err(RuntimeError::ReconciliationFailed);
            }
            let current = snapshot.status;
            if current == expected {
                return Ok(());
            }
            if is_terminal(current) {
                return Err(RuntimeError::UnexpectedTerminal {
                    expected,
                    actual: current,
                });
            }
            state
                .changed()
                .await
                .map_err(|_| RuntimeError::SupervisorClosed)?;
        }
    }

    pub async fn wait(&self) -> Result<RunOutcome, RuntimeError> {
        let mut state = self.state.clone();
        loop {
            let current = *state.borrow();
            if current.reconciliation_failed {
                return Err(RuntimeError::ReconciliationFailed);
            }
            if is_terminal(current.status) {
                return Ok(RunOutcome {
                    primary_run_id: self.primary_run_id,
                    fallback_run_id: current.fallback_run_id,
                    terminal_run_id: current.current_run_id,
                    status: current.status,
                });
            }
            state
                .changed()
                .await
                .map_err(|_| RuntimeError::SupervisorClosed)?;
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("provider adapter {0:?} is not registered")]
    MissingAdapter(ProviderId),
    #[error("provider adapter {0:?} is registered more than once")]
    DuplicateAdapter(ProviderId),
    #[error("fallback provider must differ from primary provider {0:?}")]
    InvalidFallbackProvider(ProviderId),
    #[error("run {0} is not active")]
    UnknownRun(RunId),
    #[error("run {0} has no active provider session")]
    SessionNotReady(RunId),
    #[error("approval request {request_id} is not pending for run {run_id}")]
    UnknownApproval { run_id: RunId, request_id: String },
    #[error("run supervisor is closed")]
    SupervisorClosed,
    #[error("persisted run is not a queued root that can be recovered safely")]
    InvalidRecoveryState,
    #[error("provider dispatch for run {0} was already claimed")]
    DispatchAlreadyClaimed(RunId),
    #[error("provider dispatch lease for run {0} is no longer owned by this supervisor")]
    DispatchLeaseLost(RunId),
    #[error("run admission queue is full (limit {limit})")]
    RunQueueFull { limit: usize },
    #[error("supervisor command queue is full (limit {limit})")]
    CommandQueueFull { limit: usize },
    #[error("approval response exceeds the {limit}-byte limit")]
    ApprovalResponseTooLarge { limit: usize },
    #[error("approval response operation queue is full (limit {limit})")]
    ApprovalResponseQueueFull { limit: usize },
    #[error("approval response is already in flight for run {run_id}, request {request_id}")]
    ApprovalResponseBusy { run_id: RunId, request_id: String },
    #[error("provider control operation was cancelled")]
    OperationCancelled,
    #[error("run reached {actual:?} while waiting for {expected:?}")]
    UnexpectedTerminal {
        expected: RunStatus,
        actual: RunStatus,
    },
    #[error("owned run manager task failed")]
    TaskJoin,
    #[error("an owned run task failed and was reconciled")]
    OwnedTaskFailed,
    #[error("a provider adapter did not stop after forced shutdown")]
    AdapterShutdownTimedOut,
    #[error("an owned run task failed and its durable state could not be reconciled")]
    ReconciliationFailed,
}

struct ActiveAttempt {
    run_id: RunId,
    root_id: AgentId,
    adapter: Arc<dyn ProviderAdapter>,
    session: ProviderSession,
    native_turn_id: Option<String>,
    pending_request_id: Option<String>,
    response_in_flight: bool,
    response_operation_id: Option<u64>,
    done: watch::Sender<bool>,
}

struct ActiveRun {
    cancellation: watch::Sender<bool>,
    shutdown: watch::Sender<bool>,
    state: watch::Sender<OperationState>,
    attempt_identity: Mutex<(RunId, AgentId)>,
    attempt: Mutex<Option<ActiveAttempt>>,
    turn: tokio::sync::Mutex<Option<ProviderTurn>>,
    attempt_gate: tokio::sync::Mutex<()>,
    approval_changed: Notify,
    #[cfg(test)]
    terminal_receipt_barrier: Option<Arc<TerminalReceiptBarrier>>,
    #[cfg(test)]
    active_turn_panic_barrier: Option<Arc<ActiveTurnPanicBarrier>>,
    #[cfg(test)]
    root_task_completion_barrier: Option<Arc<OwnedTaskCompletionBarrier>>,
    #[cfg(test)]
    response_task_completion_barrier: Option<Arc<OwnedTaskCompletionBarrier>>,
    #[cfg(test)]
    fallback_creation_barrier: Option<Arc<FallbackCreationBarrier>>,
    #[cfg(test)]
    fallback_transition_barrier: Option<Arc<FallbackTransitionBarrier>>,
    #[cfg(test)]
    queued_interrupt_barrier: Option<Arc<QueuedInterruptBarrier>>,
}

struct Job {
    request: RunRequest,
    primary_run_id: RunId,
    primary_root_id: AgentId,
    primary_dispatch_claimed: bool,
    dispatch_owner_id: String,
    active: Arc<ActiveRun>,
}

struct AdmittedJob {
    job: Job,
    admission: OwnedSemaphorePermit,
}

enum TaskOwner {
    Run {
        active: Arc<ActiveRun>,
        dispatch_owner_id: String,
        admission: OwnedSemaphorePermit,
    },
    Response {
        active: Arc<ActiveRun>,
        dispatch_owner_id: String,
        request_id: String,
        operation_id: u64,
        admission: OwnedSemaphorePermit,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
}

struct ResponseJob {
    requested_run_id: RunId,
    request_id: String,
    response: ApprovalResponse,
    operation_id: u64,
    dispatch_owner_id: String,
    active: Arc<ActiveRun>,
    #[cfg(test)]
    intent_barrier: Option<Arc<ResponseIntentBarrier>>,
    #[cfg(test)]
    acknowledgement_barrier: Option<Arc<ResponseAcknowledgementBarrier>>,
    #[cfg(test)]
    pre_acknowledgement_barrier: Option<Arc<ResponsePreAcknowledgementBarrier>>,
}

enum ManagerCommand {
    Spawn(AdmittedJob),
    Respond {
        job: ResponseJob,
        admission: OwnedSemaphorePermit,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
}

struct ManagerPersistence {
    store: Store,
    instance_id: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Lifecycle {
    Open,
    Closing,
    Closed,
}

pub struct RunSupervisor {
    store: Store,
    instance_id: String,
    adapters: Arc<HashMap<ProviderId, Arc<dyn ProviderAdapter>>>,
    active: Arc<Mutex<HashMap<RunId, Arc<ActiveRun>>>>,
    commands: mpsc::Sender<ManagerCommand>,
    shutdowns: mpsc::Sender<Option<oneshot::Sender<Result<(), RuntimeError>>>>,
    force_shutdown: watch::Sender<bool>,
    interrupts: Arc<Notify>,
    root_admission: Arc<Semaphore>,
    response_admission: Arc<Semaphore>,
    next_response_operation: AtomicU64,
    lifecycle: tokio::sync::Mutex<Lifecycle>,
    manager: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    #[cfg(test)]
    response_intent_barrier: Option<Arc<ResponseIntentBarrier>>,
    #[cfg(test)]
    response_acknowledgement_barrier: Option<Arc<ResponseAcknowledgementBarrier>>,
    #[cfg(test)]
    response_pre_acknowledgement_barrier: Option<Arc<ResponsePreAcknowledgementBarrier>>,
    #[cfg(test)]
    terminal_receipt_barrier: Option<Arc<TerminalReceiptBarrier>>,
    #[cfg(test)]
    active_turn_panic_barrier: Option<Arc<ActiveTurnPanicBarrier>>,
    #[cfg(test)]
    root_task_completion_barrier: Option<Arc<OwnedTaskCompletionBarrier>>,
    #[cfg(test)]
    response_task_completion_barrier: Option<Arc<OwnedTaskCompletionBarrier>>,
    #[cfg(test)]
    fallback_creation_barrier: Option<Arc<FallbackCreationBarrier>>,
    #[cfg(test)]
    fallback_transition_barrier: Option<Arc<FallbackTransitionBarrier>>,
    #[cfg(test)]
    queued_interrupt_barrier: Option<Arc<QueuedInterruptBarrier>>,
    #[cfg(test)]
    recovery_claim_barrier: Mutex<Option<Arc<Barrier>>>,
    #[cfg(test)]
    recovery_claim_gate: Mutex<Option<Arc<RecoveryClaimBarrier>>>,
    #[cfg(test)]
    recovery_promotion_gate: Mutex<Option<Arc<RecoveryPromotionBarrier>>>,
}

impl RunSupervisor {
    pub fn new(
        store: Store,
        adapters: Vec<Arc<dyn ProviderAdapter>>,
    ) -> Result<Self, RuntimeError> {
        let mut by_id = HashMap::new();
        for adapter in adapters {
            let id = adapter.id();
            if by_id.insert(id, adapter).is_some() {
                return Err(RuntimeError::DuplicateAdapter(id));
            }
        }
        let adapters = Arc::new(by_id);
        let active = Arc::new(Mutex::new(HashMap::new()));
        let root_admission = Arc::new(Semaphore::new(MAX_ADMITTED_ROOT_RUNS));
        let (commands, receiver) = mpsc::channel(SUPERVISOR_COMMAND_CAPACITY);
        let (shutdowns, shutdown_receiver) = mpsc::channel(1);
        let (force_shutdown, force_shutdown_receiver) = watch::channel(false);
        let interrupts = Arc::new(Notify::new());
        let instance_id = Uuid::now_v7().to_string();
        let manager = tokio::spawn(run_manager(
            ManagerPersistence {
                store: store.clone(),
                instance_id: instance_id.clone(),
            },
            Arc::clone(&adapters),
            Arc::clone(&active),
            receiver,
            shutdown_receiver,
            force_shutdown_receiver,
            Arc::clone(&interrupts),
        ));
        Ok(Self {
            store,
            instance_id,
            adapters,
            active,
            commands,
            shutdowns,
            force_shutdown,
            interrupts,
            root_admission,
            response_admission: Arc::new(Semaphore::new(MAX_CONCURRENT_APPROVAL_RESPONSES)),
            next_response_operation: AtomicU64::new(1),
            lifecycle: tokio::sync::Mutex::new(Lifecycle::Open),
            manager: tokio::sync::Mutex::new(Some(manager)),
            #[cfg(test)]
            response_intent_barrier: None,
            #[cfg(test)]
            response_acknowledgement_barrier: None,
            #[cfg(test)]
            response_pre_acknowledgement_barrier: None,
            #[cfg(test)]
            terminal_receipt_barrier: None,
            #[cfg(test)]
            active_turn_panic_barrier: None,
            #[cfg(test)]
            root_task_completion_barrier: None,
            #[cfg(test)]
            response_task_completion_barrier: None,
            #[cfg(test)]
            fallback_creation_barrier: None,
            #[cfg(test)]
            fallback_transition_barrier: None,
            #[cfg(test)]
            queued_interrupt_barrier: None,
            #[cfg(test)]
            recovery_claim_barrier: Mutex::new(None),
            #[cfg(test)]
            recovery_claim_gate: Mutex::new(None),
            #[cfg(test)]
            recovery_promotion_gate: Mutex::new(None),
        })
    }

    #[cfg(test)]
    pub(crate) async fn crash_for_test(&self) {
        if let Some(manager) = self.manager.lock().await.take() {
            manager.abort();
            let _ = manager.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn synchronize_recovery_claim_for_test(&self, barrier: Arc<Barrier>) {
        *self
            .recovery_claim_barrier
            .lock()
            .expect("recovery claim barrier mutex must not be poisoned") = Some(barrier);
    }

    #[cfg(test)]
    fn hold_recovery_claim_for_test(&self, barrier: Arc<RecoveryClaimBarrier>) {
        *self
            .recovery_claim_gate
            .lock()
            .expect("recovery claim gate mutex must not be poisoned") = Some(barrier);
    }

    #[cfg(test)]
    fn hold_recovery_promotion_for_test(&self, barrier: Arc<RecoveryPromotionBarrier>) {
        *self
            .recovery_promotion_gate
            .lock()
            .expect("recovery promotion gate mutex must not be poisoned") = Some(barrier);
    }

    pub async fn submit(&self, request: RunRequest) -> Result<RunHandle, RuntimeError> {
        let lifecycle = self.lifecycle.lock().await;
        if *lifecycle != Lifecycle::Open {
            return Err(RuntimeError::SupervisorClosed);
        }
        self.adapter(request.provider)?;
        if let Some(fallback) = &request.fallback {
            if fallback.provider == request.provider {
                return Err(RuntimeError::InvalidFallbackProvider(fallback.provider));
            }
            self.adapter(fallback.provider)?;
        }
        let admission = Arc::clone(&self.root_admission)
            .try_acquire_owned()
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::NoPermits => RuntimeError::RunQueueFull {
                    limit: MAX_QUEUED_ROOT_RUNS,
                },
                tokio::sync::TryAcquireError::Closed => RuntimeError::SupervisorClosed,
            })?;
        let command = self
            .commands
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RuntimeError::CommandQueueFull {
                    limit: SUPERVISOR_COMMAND_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => RuntimeError::SupervisorClosed,
            })?;
        let (run, root) = self
            .store
            .create_run(request.conversation_id, request.provider)
            .await?;
        if !self
            .store
            .claim_provider_dispatch(run.id, &self.instance_id, DISPATCH_LEASE_DURATION)
            .await?
        {
            return Err(RuntimeError::DispatchAlreadyClaimed(run.id));
        }
        let handle = self.enqueue_prepared(request, run, root, true, admission, command);
        drop(lifecycle);
        Ok(handle)
    }

    pub(crate) async fn submit_persisted(
        &self,
        request: RunRequest,
        submission: NewSubmission,
    ) -> Result<PreparedRunHandle, RuntimeError> {
        let lifecycle = self.lifecycle.lock().await;
        if *lifecycle != Lifecycle::Open {
            return Err(RuntimeError::SupervisorClosed);
        }
        self.adapter(request.provider)?;
        if let Some(fallback) = &request.fallback {
            if fallback.provider == request.provider {
                return Err(RuntimeError::InvalidFallbackProvider(fallback.provider));
            }
            self.adapter(fallback.provider)?;
        }
        let admission = Arc::clone(&self.root_admission)
            .try_acquire_owned()
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::NoPermits => RuntimeError::RunQueueFull {
                    limit: MAX_QUEUED_ROOT_RUNS,
                },
                tokio::sync::TryAcquireError::Closed => RuntimeError::SupervisorClosed,
            })?;
        let command = self
            .commands
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RuntimeError::CommandQueueFull {
                    limit: SUPERVISOR_COMMAND_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => RuntimeError::SupervisorClosed,
            })?;
        match self
            .store
            .prepare_claimed_submission(submission, &self.instance_id, DISPATCH_LEASE_DURATION)
            .await?
        {
            PreparedSubmission::Created { run, root } => {
                let handle = self.enqueue_prepared(request, run, root, true, admission, command);
                drop(lifecycle);
                Ok(PreparedRunHandle {
                    handle,
                    duplicate: false,
                })
            }
            PreparedSubmission::Duplicate(run) => {
                drop(command);
                drop(admission);
                let handle = self.handle_for_run(run);
                drop(lifecycle);
                Ok(PreparedRunHandle {
                    handle,
                    duplicate: true,
                })
            }
        }
    }

    /// Acquires a unique durable recovery owner before admission or any run mutation. A caller
    /// that loses this CAS must leave the run to the winning reconciler.
    pub(crate) async fn claim_recovery_run(
        &self,
        run_id: RunId,
        reap_stale_claim: bool,
    ) -> Result<Option<RecoveryClaim>, RuntimeError> {
        let lifecycle = self.lifecycle.lock().await;
        if *lifecycle != Lifecycle::Open {
            return Err(RuntimeError::SupervisorClosed);
        }
        #[cfg(test)]
        let recovery_claim_gate = self
            .recovery_claim_gate
            .lock()
            .expect("recovery claim gate mutex must not be poisoned")
            .clone();
        #[cfg(test)]
        if let Some(barrier) = recovery_claim_gate {
            barrier.ready.notify_one();
            barrier.release.notified().await;
        }
        #[cfg(test)]
        let recovery_claim_barrier = self
            .recovery_claim_barrier
            .lock()
            .expect("recovery claim barrier mutex must not be poisoned")
            .clone();
        #[cfg(test)]
        if let Some(barrier) = recovery_claim_barrier {
            barrier.wait().await;
        }
        let owner_id = format!("recovery:{}", Uuid::now_v7());
        let claimed = if reap_stale_claim {
            self.store
                .claim_stale_provider_dispatch(
                    run_id,
                    &owner_id,
                    &self.instance_id,
                    DISPATCH_LEASE_DURATION,
                    DISPATCH_LEASE_STALE_GRACE,
                )
                .await?
        } else {
            self.store
                .claim_unowned_provider_dispatch_recovery(
                    run_id,
                    &owner_id,
                    DISPATCH_LEASE_DURATION,
                )
                .await?
        };
        drop(lifecycle);
        Ok(claimed.then_some(RecoveryClaim { run_id, owner_id }))
    }

    /// Re-enters an already durable queued run after a unique recovery claim. Admission and
    /// command reservation happen only after that claim, and promotion to this supervisor is a
    /// second owner-fenced CAS immediately before enqueue.
    pub(crate) async fn recover_persisted(
        &self,
        request: RunRequest,
        run: crate::domain::ProviderRun,
        root: crate::domain::AgentNode,
        claim: &RecoveryClaim,
    ) -> Result<RunHandle, RuntimeError> {
        let lifecycle = self.lifecycle.lock().await;
        if *lifecycle != Lifecycle::Open {
            return Err(RuntimeError::SupervisorClosed);
        }
        if claim.run_id != run.id
            || run.status != RunStatus::Queued
            || root.run_id != run.id
            || root.parent_id.is_some()
        {
            return Err(RuntimeError::InvalidRecoveryState);
        }
        self.adapter(request.provider)?;
        let admission = Arc::clone(&self.root_admission)
            .try_acquire_owned()
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::NoPermits => RuntimeError::RunQueueFull {
                    limit: MAX_QUEUED_ROOT_RUNS,
                },
                tokio::sync::TryAcquireError::Closed => RuntimeError::SupervisorClosed,
            })?;
        let command = self
            .commands
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RuntimeError::CommandQueueFull {
                    limit: SUPERVISOR_COMMAND_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => RuntimeError::SupervisorClosed,
            })?;
        #[cfg(test)]
        let recovery_promotion_gate = self
            .recovery_promotion_gate
            .lock()
            .expect("recovery promotion gate mutex must not be poisoned")
            .clone();
        #[cfg(test)]
        if let Some(barrier) = recovery_promotion_gate {
            barrier.ready.notify_one();
            barrier.release.notified().await;
        }
        if !self
            .store
            .promote_recovery_dispatch_claim(
                run.id,
                &claim.owner_id,
                &self.instance_id,
                DISPATCH_LEASE_DURATION,
            )
            .await?
        {
            return Err(RuntimeError::DispatchAlreadyClaimed(run.id));
        }
        let handle = self.enqueue_prepared(request, run, root, true, admission, command);
        drop(lifecycle);
        Ok(handle)
    }

    fn enqueue_prepared(
        &self,
        request: RunRequest,
        run: crate::domain::ProviderRun,
        root: crate::domain::AgentNode,
        dispatch_claimed: bool,
        admission: OwnedSemaphorePermit,
        command: mpsc::OwnedPermit<ManagerCommand>,
    ) -> RunHandle {
        let (cancellation, _) = watch::channel(false);
        let (shutdown, _) = watch::channel(false);
        let initial = OperationState {
            current_run_id: run.id,
            fallback_run_id: None,
            status: RunStatus::Queued,
            reconciliation_failed: false,
        };
        let (state, receiver) = watch::channel(initial);
        let active = Arc::new(ActiveRun {
            cancellation,
            shutdown,
            state,
            attempt_identity: Mutex::new((run.id, root.id)),
            attempt: Mutex::new(None),
            turn: tokio::sync::Mutex::new(None),
            attempt_gate: tokio::sync::Mutex::new(()),
            approval_changed: Notify::new(),
            #[cfg(test)]
            terminal_receipt_barrier: self.terminal_receipt_barrier.clone(),
            #[cfg(test)]
            active_turn_panic_barrier: self.active_turn_panic_barrier.clone(),
            #[cfg(test)]
            root_task_completion_barrier: self.root_task_completion_barrier.clone(),
            #[cfg(test)]
            response_task_completion_barrier: self.response_task_completion_barrier.clone(),
            #[cfg(test)]
            fallback_creation_barrier: self.fallback_creation_barrier.clone(),
            #[cfg(test)]
            fallback_transition_barrier: self.fallback_transition_barrier.clone(),
            #[cfg(test)]
            queued_interrupt_barrier: self.queued_interrupt_barrier.clone(),
        });
        self.active
            .lock()
            .expect("active run mutex must not be poisoned")
            .insert(run.id, Arc::clone(&active));
        command.send(ManagerCommand::Spawn(AdmittedJob {
            job: Job {
                request,
                primary_run_id: run.id,
                primary_root_id: root.id,
                primary_dispatch_claimed: dispatch_claimed,
                dispatch_owner_id: self.instance_id.clone(),
                active: Arc::clone(&active),
            },
            admission,
        }));
        RunHandle {
            primary_run_id: run.id,
            state: receiver,
        }
    }

    fn handle_for_run(&self, run: crate::domain::ProviderRun) -> RunHandle {
        if let Some(active) = self
            .active
            .lock()
            .expect("active run mutex must not be poisoned")
            .get(&run.id)
        {
            return RunHandle {
                primary_run_id: run.id,
                state: active.state.subscribe(),
            };
        }
        let initial = OperationState {
            current_run_id: run.id,
            fallback_run_id: run.fallback_from_run_id,
            status: run.status,
            reconciliation_failed: false,
        };
        let (_, state) = watch::channel(initial);
        RunHandle {
            primary_run_id: run.id,
            state,
        }
    }

    pub(crate) fn existing_handle(
        &self,
        run: crate::domain::ProviderRun,
        fallback: Option<crate::domain::ProviderRun>,
    ) -> RunHandle {
        if self
            .active
            .lock()
            .expect("active run mutex must not be poisoned")
            .contains_key(&run.id)
        {
            return self.handle_for_run(run);
        }
        let terminal = fallback.as_ref().unwrap_or(&run);
        let initial = OperationState {
            current_run_id: terminal.id,
            fallback_run_id: fallback.as_ref().map(|fallback| fallback.id),
            status: terminal.status,
            reconciliation_failed: false,
        };
        let (_, state) = watch::channel(initial);
        RunHandle {
            primary_run_id: run.id,
            state,
        }
    }

    pub async fn respond(
        &self,
        run_id: RunId,
        request_id: &str,
        response: ApprovalResponse,
    ) -> Result<(), RuntimeError> {
        let response_bytes = match &response {
            ApprovalResponse::Answer(answer) => answer.len(),
            ApprovalResponse::Answers(answers) => {
                answers.iter().fold(0usize, |total, (id, values)| {
                    values
                        .iter()
                        .fold(total.saturating_add(id.len()), |total, value| {
                            total.saturating_add(value.len())
                        })
                })
            }
            ApprovalResponse::Approved | ApprovalResponse::Denied => 0,
        };
        if response_bytes > MAX_APPROVAL_RESPONSE_BYTES {
            return Err(RuntimeError::ApprovalResponseTooLarge {
                limit: MAX_APPROVAL_RESPONSE_BYTES,
            });
        }
        let lifecycle = self.lifecycle.lock().await;
        if *lifecycle != Lifecycle::Open {
            return Err(RuntimeError::SupervisorClosed);
        }
        let active = self.active_run(run_id)?;
        let operation_id = self.next_response_operation.fetch_add(1, Ordering::Relaxed);
        let response_admission = {
            let mut attempt = active
                .attempt
                .lock()
                .expect("active attempt mutex must not be poisoned");
            let attempt = attempt
                .as_mut()
                .ok_or(RuntimeError::SessionNotReady(run_id))?;
            if attempt.response_operation_id.is_some() {
                return Err(RuntimeError::ApprovalResponseBusy {
                    run_id,
                    request_id: request_id.to_owned(),
                });
            }
            let admission = Arc::clone(&self.response_admission)
                .try_acquire_owned()
                .map_err(|error| match error {
                    tokio::sync::TryAcquireError::NoPermits => {
                        RuntimeError::ApprovalResponseQueueFull {
                            limit: MAX_CONCURRENT_APPROVAL_RESPONSES,
                        }
                    }
                    tokio::sync::TryAcquireError::Closed => RuntimeError::SupervisorClosed,
                })?;
            attempt.response_operation_id = Some(operation_id);
            admission
        };
        let command = match self.commands.clone().try_reserve_owned() {
            Ok(command) => command,
            Err(error) => {
                clear_response_operation(&active, operation_id);
                return Err(match error {
                    mpsc::error::TrySendError::Full(_) => RuntimeError::CommandQueueFull {
                        limit: SUPERVISOR_COMMAND_CAPACITY,
                    },
                    mpsc::error::TrySendError::Closed(_) => RuntimeError::SupervisorClosed,
                });
            }
        };
        let (reply, result) = oneshot::channel();
        command.send(ManagerCommand::Respond {
            job: ResponseJob {
                requested_run_id: run_id,
                request_id: request_id.to_owned(),
                response,
                operation_id,
                dispatch_owner_id: self.instance_id.clone(),
                active,
                #[cfg(test)]
                intent_barrier: self.response_intent_barrier.clone(),
                #[cfg(test)]
                acknowledgement_barrier: self.response_acknowledgement_barrier.clone(),
                #[cfg(test)]
                pre_acknowledgement_barrier: self.response_pre_acknowledgement_barrier.clone(),
            },
            admission: response_admission,
            reply,
        });
        drop(lifecycle);
        result.await.map_err(|_| RuntimeError::SupervisorClosed)?
    }

    pub async fn steer(&self, run_id: RunId, text: &str) -> Result<(), RuntimeError> {
        let active = self.active_run(run_id)?;
        let (attempt_run_id, adapter, session, native_turn_id, mut done) = {
            let attempt = active
                .attempt
                .lock()
                .expect("active attempt mutex must not be poisoned");
            let attempt = attempt
                .as_ref()
                .ok_or(RuntimeError::SessionNotReady(run_id))?;
            (
                attempt.run_id,
                Arc::clone(&attempt.adapter),
                attempt.session.clone(),
                attempt
                    .native_turn_id
                    .clone()
                    .ok_or(RuntimeError::SessionNotReady(run_id))?,
                attempt.done.subscribe(),
            )
        };
        let mut cancelled = active.cancellation.subscribe();
        let mut shutdown = active.shutdown.subscribe();
        if !self
            .store
            .refresh_provider_dispatch_lease(
                attempt_run_id,
                &self.instance_id,
                DISPATCH_LEASE_DURATION,
                DISPATCH_LEASE_STALE_GRACE,
            )
            .await?
        {
            return Err(RuntimeError::DispatchLeaseLost(attempt_run_id));
        }
        race_attempt_control(
            adapter.steer(&session, &native_turn_id, text),
            &mut cancelled,
            &mut shutdown,
            &mut done,
        )
        .await??;
        Ok(())
    }

    pub async fn interrupt(&self, run_id: RunId) -> Result<(), RuntimeError> {
        let active = self.active_run(run_id)?;
        active.cancellation.send_replace(true);
        active.approval_changed.notify_one();
        self.interrupts.notify_one();
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        {
            let mut lifecycle = self.lifecycle.lock().await;
            match *lifecycle {
                Lifecycle::Closed => return Ok(()),
                Lifecycle::Closing => return Err(RuntimeError::SupervisorClosed),
                Lifecycle::Open => *lifecycle = Lifecycle::Closing,
            }
        }
        let mut manager = self.manager.lock().await;
        let Some(handle) = manager.as_mut() else {
            *self.lifecycle.lock().await = Lifecycle::Closed;
            return self.shutdown_adapters().await;
        };
        let (reply, response) = oneshot::channel();
        if self.shutdowns.send(Some(reply)).await.is_err() {
            let join_result = handle.await.map_err(|_| RuntimeError::TaskJoin);
            manager.take();
            drop(manager);
            let _ = self.shutdown_adapters().await;
            *self.lifecycle.lock().await = Lifecycle::Closed;
            return join_result.and(Err(RuntimeError::SupervisorClosed));
        }
        let result = response
            .await
            .map_err(|_| RuntimeError::SupervisorClosed)
            .and_then(|result| result);
        let join_result = handle.await.map_err(|_| RuntimeError::TaskJoin);
        manager.take();
        drop(manager);
        let adapter_result = self.shutdown_adapters().await;
        *self.lifecycle.lock().await = Lifecycle::Closed;
        result.and(join_result).and(adapter_result)
    }

    pub async fn shutdown_with_grace(&self, grace: Duration) -> Result<(), RuntimeError> {
        match tokio::time::timeout(grace, self.shutdown()).await {
            Ok(result) => result,
            Err(_) => self.force_shutdown().await,
        }
    }

    pub async fn force_shutdown(&self) -> Result<(), RuntimeError> {
        for run in unique_active_runs(&self.active) {
            run.shutdown.send_replace(true);
            run.approval_changed.notify_one();
        }
        for adapter in self.adapters.values() {
            adapter.force_shutdown();
        }
        self.force_shutdown.send_replace(true);
        self.interrupts.notify_waiters();

        let mut manager = self.manager.lock().await;
        let manager_result = match manager.as_mut() {
            Some(handle) => {
                match tokio::time::timeout(FORCED_OWNER_SHUTDOWN_TIMEOUT, &mut *handle).await {
                    Ok(result) => result.map_err(|_| RuntimeError::TaskJoin),
                    Err(_) => {
                        handle.abort();
                        handle.await.map_err(|error| {
                            if error.is_cancelled() {
                                RuntimeError::OwnedTaskFailed
                            } else {
                                RuntimeError::TaskJoin
                            }
                        })
                    }
                }
            }
            None => Ok(()),
        };
        manager.take();
        drop(manager);
        let adapter_result = self
            .shutdown_adapters_with_timeout(FORCED_OWNER_SHUTDOWN_TIMEOUT)
            .await;
        *self.lifecycle.lock().await = Lifecycle::Closed;
        manager_result.and(adapter_result)
    }

    async fn shutdown_adapters(&self) -> Result<(), RuntimeError> {
        let mut first_error = None;
        for adapter in self.adapters.values() {
            if let Err(error) = adapter.shutdown().await
                && first_error.is_none()
            {
                first_error = Some(RuntimeError::Provider(error));
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn shutdown_adapters_with_timeout(&self, timeout: Duration) -> Result<(), RuntimeError> {
        let mut tasks = JoinSet::new();
        for adapter in self.adapters.values() {
            let adapter = Arc::clone(adapter);
            tasks.spawn(async move { adapter.shutdown().await });
        }
        let shutdown = async {
            let mut first_error = None;
            while let Some(result) = tasks.join_next().await {
                match result {
                    Ok(Err(error)) if first_error.is_none() => {
                        first_error = Some(RuntimeError::Provider(error));
                    }
                    Err(_) if first_error.is_none() => {
                        first_error = Some(RuntimeError::TaskJoin);
                    }
                    _ => {}
                }
            }
            first_error.map_or(Ok(()), Err)
        };
        match tokio::time::timeout(timeout, shutdown).await {
            Ok(result) => result,
            Err(_) => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                Err(RuntimeError::AdapterShutdownTimedOut)
            }
        }
    }

    fn adapter(&self, id: ProviderId) -> Result<Arc<dyn ProviderAdapter>, RuntimeError> {
        self.adapters
            .get(&id)
            .cloned()
            .ok_or(RuntimeError::MissingAdapter(id))
    }

    fn active_run(&self, id: RunId) -> Result<Arc<ActiveRun>, RuntimeError> {
        self.active
            .lock()
            .expect("active run mutex must not be poisoned")
            .get(&id)
            .cloned()
            .ok_or(RuntimeError::UnknownRun(id))
    }

    #[cfg(test)]
    fn set_response_intent_barrier(&mut self, barrier: Arc<ResponseIntentBarrier>) {
        self.response_intent_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_response_pre_acknowledgement_barrier(
        &mut self,
        barrier: Arc<ResponsePreAcknowledgementBarrier>,
    ) {
        self.response_pre_acknowledgement_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_response_acknowledgement_barrier(
        &mut self,
        barrier: Arc<ResponseAcknowledgementBarrier>,
    ) {
        self.response_acknowledgement_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_terminal_receipt_barrier(&mut self, barrier: Arc<TerminalReceiptBarrier>) {
        self.terminal_receipt_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_active_turn_panic_barrier(&mut self, barrier: Arc<ActiveTurnPanicBarrier>) {
        self.active_turn_panic_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_root_task_completion_barrier(&mut self, barrier: Arc<OwnedTaskCompletionBarrier>) {
        self.root_task_completion_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_response_task_completion_barrier(&mut self, barrier: Arc<OwnedTaskCompletionBarrier>) {
        self.response_task_completion_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_fallback_creation_barrier(&mut self, barrier: Arc<FallbackCreationBarrier>) {
        self.fallback_creation_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_fallback_transition_barrier(&mut self, barrier: Arc<FallbackTransitionBarrier>) {
        self.fallback_transition_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn set_queued_interrupt_barrier(&mut self, barrier: Arc<QueuedInterruptBarrier>) {
        self.queued_interrupt_barrier = Some(barrier);
    }

    #[cfg(test)]
    fn available_root_admission_for_test(&self) -> usize {
        self.root_admission.available_permits()
    }
}

impl Drop for RunSupervisor {
    fn drop(&mut self) {
        for run in unique_active_runs(&self.active) {
            run.shutdown.send_replace(true);
            run.approval_changed.notify_one();
        }
        for adapter in self.adapters.values() {
            adapter.force_shutdown();
        }
        self.force_shutdown.send_replace(true);
        let _ = self.shutdowns.try_send(None);
    }
}

async fn run_manager(
    persistence: ManagerPersistence,
    adapters: Arc<HashMap<ProviderId, Arc<dyn ProviderAdapter>>>,
    active: Arc<Mutex<HashMap<RunId, Arc<ActiveRun>>>>,
    mut commands: mpsc::Receiver<ManagerCommand>,
    mut shutdowns: mpsc::Receiver<Option<oneshot::Sender<Result<(), RuntimeError>>>>,
    mut force_shutdown: watch::Receiver<bool>,
    interrupts: Arc<Notify>,
) {
    let ManagerPersistence { store, instance_id } = persistence;
    let mut tasks = JoinSet::new();
    let mut owners: HashMap<Id, TaskOwner> = HashMap::new();
    let mut pending = VecDeque::with_capacity(MAX_QUEUED_ROOT_RUNS);
    let mut active_root_tasks = 0usize;
    let mut task_failed = false;
    let first_lease_refresh = tokio::time::Instant::now() + DISPATCH_LEASE_REFRESH_INTERVAL;
    let mut lease_refreshes =
        tokio::time::interval_at(first_lease_refresh, DISPATCH_LEASE_REFRESH_INTERVAL);
    lease_refreshes.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let shutdown_reply = loop {
        task_failed |= interrupt_cancelled_pending(&store, &active, &mut pending).await;
        while active_root_tasks < MAX_CONCURRENT_ROOT_RUNS {
            let Some(job) = pending.pop_front() else {
                break;
            };
            spawn_root_task(&store, &adapters, &active, &mut tasks, &mut owners, job);
            active_root_tasks += 1;
        }
        tokio::select! {
            biased;
            changed = force_shutdown.changed() => {
                if changed.is_err() || *force_shutdown.borrow() {
                    break None;
                }
            }
            shutdown = shutdowns.recv() => break shutdown.flatten(),
            _ = interrupts.notified() => {}
            _ = lease_refreshes.tick() => {
                if refresh_manager_dispatch_leases(&store, &instance_id, &active).await.is_err() {
                    task_failed = true;
                    break None;
                }
            }
            command = commands.recv() => match command {
                Some(ManagerCommand::Spawn(job)) => {
                    if active_root_tasks < MAX_CONCURRENT_ROOT_RUNS {
                        spawn_root_task(
                            &store,
                            &adapters,
                            &active,
                            &mut tasks,
                            &mut owners,
                            job,
                        );
                        active_root_tasks += 1;
                    } else {
                        debug_assert!(pending.len() < MAX_QUEUED_ROOT_RUNS);
                        pending.push_back(job);
                    }
                }
                Some(ManagerCommand::Respond { job, admission, reply }) => {
                    let owner = TaskOwner::Response {
                        active: Arc::clone(&job.active),
                        dispatch_owner_id: job.dispatch_owner_id.clone(),
                        request_id: job.request_id.clone(),
                        operation_id: job.operation_id,
                        admission,
                        reply,
                    };
                    #[cfg(test)]
                    let completion_barrier = job.active.response_task_completion_barrier.clone();
                    let response_store = store.clone();
                    let task = tasks.spawn(async move {
                        let result = execute_response(response_store, job).await;
                        #[cfg(test)]
                        if let Some(barrier) = completion_barrier {
                            barrier.completed.notify_one();
                            barrier.release.notified().await;
                        }
                        result
                    });
                    owners.insert(task.id(), owner);
                }
                None => break None,
            },
            result = tasks.join_next_with_id(), if !tasks.is_empty() => {
                if let Some(result) = result {
                    let id = match &result {
                        Ok((id, _)) => *id,
                        Err(error) => error.id(),
                    };
                    if matches!(owners.get(&id), Some(TaskOwner::Run { .. })) {
                        active_root_tasks -= 1;
                    }
                    task_failed |= reconcile_task(&store, &active, &mut owners, result).await;
                }
            }
        }
    };

    for run in unique_manager_runs(&active, &owners, &pending) {
        run.shutdown.send_replace(true);
        run.approval_changed.notify_one();
    }
    commands.close();
    while let Some(command) = commands.recv().await {
        match command {
            ManagerCommand::Spawn(job) => pending.push_back(job),
            ManagerCommand::Respond {
                job,
                admission: _,
                reply,
            } => {
                clear_response_operation(&job.active, job.operation_id);
                let _ = reply.send(Err(RuntimeError::OperationCancelled));
            }
        }
    }
    while let Some(job) = pending.pop_front() {
        if interrupt_queued(&store, &job.job).await.is_err() {
            mark_reconciliation_failed(&job.job.active);
            task_failed = true;
        }
        remove_joined_run(&active, &job.job.active);
    }
    let mut forced = *force_shutdown.borrow();
    if forced {
        tasks.abort_all();
    }
    while !tasks.is_empty() {
        tokio::select! {
            biased;
            changed = force_shutdown.changed(), if !forced => {
                if changed.is_err() || *force_shutdown.borrow() {
                    forced = true;
                    tasks.abort_all();
                }
            }
            result = tasks.join_next_with_id() => {
                if let Some(result) = result {
                    task_failed |= reconcile_task(&store, &active, &mut owners, result).await;
                }
            }
        }
    }
    if let Some(reply) = shutdown_reply {
        let result = if task_failed {
            Err(RuntimeError::OwnedTaskFailed)
        } else {
            Ok(())
        };
        let _ = reply.send(result);
    }
}

async fn refresh_manager_dispatch_leases(
    store: &Store,
    owner_id: &str,
    active: &Mutex<HashMap<RunId, Arc<ActiveRun>>>,
) -> Result<(), StoreError> {
    for run in unique_active_runs(active) {
        let run_id = run
            .attempt_identity
            .lock()
            .expect("attempt identity mutex must not be poisoned")
            .0;
        if !store
            .refresh_provider_dispatch_lease(
                run_id,
                owner_id,
                DISPATCH_LEASE_DURATION,
                DISPATCH_LEASE_STALE_GRACE,
            )
            .await?
            && !store.is_owned_fallback_transition(run_id, owner_id).await?
        {
            run.shutdown.send_replace(true);
            run.approval_changed.notify_one();
        }
    }
    Ok(())
}

async fn interrupt_cancelled_pending(
    store: &Store,
    active: &Mutex<HashMap<RunId, Arc<ActiveRun>>>,
    pending: &mut VecDeque<AdmittedJob>,
) -> bool {
    let mut failed = false;
    let mut index = 0;
    while index < pending.len() {
        if *pending[index].job.active.cancellation.borrow() {
            let job = pending.remove(index).expect("pending index must exist");
            if interrupt_queued(store, &job.job).await.is_err() {
                mark_reconciliation_failed(&job.job.active);
                failed = true;
            }
            remove_joined_run(active, &job.job.active);
        } else {
            index += 1;
        }
    }
    failed
}

fn spawn_root_task(
    store: &Store,
    adapters: &Arc<HashMap<ProviderId, Arc<dyn ProviderAdapter>>>,
    active_runs: &Arc<Mutex<HashMap<RunId, Arc<ActiveRun>>>>,
    tasks: &mut JoinSet<Result<(), RuntimeError>>,
    owners: &mut HashMap<Id, TaskOwner>,
    admitted: AdmittedJob,
) {
    let AdmittedJob { job, admission } = admitted;
    let owner = TaskOwner::Run {
        active: Arc::clone(&job.active),
        dispatch_owner_id: job.dispatch_owner_id.clone(),
        admission,
    };
    let store = store.clone();
    let adapters = Arc::clone(adapters);
    let active_runs = Arc::clone(active_runs);
    #[cfg(test)]
    let completion_barrier = job.active.root_task_completion_barrier.clone();
    let task = tasks.spawn(async move {
        let result = execute_job(store, adapters, active_runs, job).await;
        #[cfg(test)]
        if let Some(barrier) = completion_barrier {
            barrier.completed.notify_one();
            barrier.release.notified().await;
        }
        result
    });
    owners.insert(task.id(), owner);
}

fn unique_manager_runs(
    active: &Mutex<HashMap<RunId, Arc<ActiveRun>>>,
    owners: &HashMap<Id, TaskOwner>,
    pending: &VecDeque<AdmittedJob>,
) -> Vec<Arc<ActiveRun>> {
    let mut unique = unique_active_runs(active);
    for run in owners.values().map(|owner| match owner {
        TaskOwner::Run { active, .. } | TaskOwner::Response { active, .. } => active,
    }) {
        if !unique.iter().any(|candidate| Arc::ptr_eq(candidate, run)) {
            unique.push(Arc::clone(run));
        }
    }
    for admitted in pending {
        let job = &admitted.job;
        if !unique
            .iter()
            .any(|candidate| Arc::ptr_eq(candidate, &job.active))
        {
            unique.push(Arc::clone(&job.active));
        }
    }
    unique
}

async fn execute_response(store: Store, job: ResponseJob) -> Result<(), RuntimeError> {
    let gate = job.active.attempt_gate.lock().await;
    let (attempt_run_id, root_id, adapter, session, mut done, request_is_pending) = {
        let state = job
            .active
            .attempt
            .lock()
            .expect("active attempt mutex must not be poisoned");
        let attempt = state
            .as_ref()
            .ok_or(RuntimeError::SessionNotReady(job.requested_run_id))?;
        (
            attempt.run_id,
            attempt.root_id,
            Arc::clone(&attempt.adapter),
            attempt.session.clone(),
            attempt.done.subscribe(),
            attempt.pending_request_id.as_deref() == Some(job.request_id.as_str()),
        )
    };
    if !request_is_pending {
        return match store.load_approval(attempt_run_id, &job.request_id).await {
            Ok(approval)
                if approval.response_intent.as_ref().is_some_and(|intent| {
                    intent.status == ApprovalResponseIntentStatus::Acknowledged
                }) =>
            {
                Err(StoreError::ApprovalResponseAlreadyAcknowledged.into())
            }
            Ok(_) | Err(StoreError::NotFound { .. }) => Err(RuntimeError::UnknownApproval {
                run_id: job.requested_run_id,
                request_id: job.request_id,
            }),
            Err(error) => Err(error.into()),
        };
    }
    let mut cancelled = job.active.cancellation.subscribe();
    let mut shutdown = job.active.shutdown.subscribe();
    if *cancelled.borrow() || *shutdown.borrow() || *done.borrow() {
        return Err(RuntimeError::OperationCancelled);
    }
    let resolution = match &job.response {
        ApprovalResponse::Approved => ApprovalResolution::Approved,
        ApprovalResponse::Denied => ApprovalResolution::Denied,
        ApprovalResponse::Answer(answer) => ApprovalResolution::Answer(answer.clone()),
        ApprovalResponse::Answers(answers) => ApprovalResolution::Answers(answers.clone()),
    };
    store
        .record_owned_response_intent(
            attempt_run_id,
            root_id,
            &job.request_id,
            resolution,
            &job.dispatch_owner_id,
        )
        .await?;
    if *cancelled.borrow() || *shutdown.borrow() || *done.borrow() {
        let rejection = store
            .reject_owned_response_intent(
                attempt_run_id,
                root_id,
                &job.request_id,
                DispatchCertainty::NotDispatched,
                &job.dispatch_owner_id,
            )
            .await;
        job.active.approval_changed.notify_one();
        rejection?;
        return Err(RuntimeError::OperationCancelled);
    }
    if !store
        .refresh_provider_dispatch_lease(
            attempt_run_id,
            &job.dispatch_owner_id,
            DISPATCH_LEASE_DURATION,
            DISPATCH_LEASE_STALE_GRACE,
        )
        .await?
    {
        store
            .reject_owned_response_intent(
                attempt_run_id,
                root_id,
                &job.request_id,
                DispatchCertainty::NotDispatched,
                &job.dispatch_owner_id,
            )
            .await?;
        job.active.approval_changed.notify_one();
        return Err(RuntimeError::DispatchLeaseLost(attempt_run_id));
    }
    #[cfg(test)]
    if let Some(barrier) = &job.intent_barrier {
        barrier.committed.notify_one();
        barrier.release.notified().await;
    }
    if !store
        .refresh_provider_dispatch_lease(
            attempt_run_id,
            &job.dispatch_owner_id,
            DISPATCH_LEASE_DURATION,
            DISPATCH_LEASE_STALE_GRACE,
        )
        .await?
    {
        return Err(RuntimeError::DispatchLeaseLost(attempt_run_id));
    }
    if let Some(attempt) = job
        .active
        .attempt
        .lock()
        .expect("active attempt mutex must not be poisoned")
        .as_mut()
    {
        attempt.response_in_flight = true;
    }
    drop(gate);

    let response_result = race_attempt_control(
        adapter.respond(&session, &job.request_id, job.response),
        &mut cancelled,
        &mut shutdown,
        &mut done,
    )
    .await;
    let _gate = job.active.attempt_gate.lock().await;
    match response_result {
        Err(RuntimeError::OperationCancelled) => {
            let rejection = store
                .reject_owned_response_intent(
                    attempt_run_id,
                    root_id,
                    &job.request_id,
                    DispatchCertainty::MayHaveDispatched,
                    &job.dispatch_owner_id,
                )
                .await;
            clear_response_state(&job.active, attempt_run_id, &job.request_id, false);
            match rejection {
                Ok(_) | Err(StoreError::InvalidApprovalResponseIntentState) => {
                    Err(RuntimeError::OperationCancelled)
                }
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error),
        Ok(Err(error)) => {
            let rejection = store
                .reject_owned_response_intent(
                    attempt_run_id,
                    root_id,
                    &job.request_id,
                    error.dispatch_certainty(),
                    &job.dispatch_owner_id,
                )
                .await;
            clear_response_state(&job.active, attempt_run_id, &job.request_id, false);
            rejection?;
            Err(error.into())
        }
        Ok(Ok(())) => {
            if *cancelled.borrow() || *shutdown.borrow() || *done.borrow() {
                let rejection = store
                    .reject_owned_response_intent(
                        attempt_run_id,
                        root_id,
                        &job.request_id,
                        DispatchCertainty::MayHaveDispatched,
                        &job.dispatch_owner_id,
                    )
                    .await;
                clear_response_state(&job.active, attempt_run_id, &job.request_id, false);
                match rejection {
                    Ok(_) | Err(StoreError::InvalidApprovalResponseIntentState) => {
                        return Err(RuntimeError::OperationCancelled);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            #[cfg(test)]
            if let Some(barrier) = &job.pre_acknowledgement_barrier {
                barrier.ready.notify_one();
                barrier.release.notified().await;
            }
            let acknowledgement = store
                .acknowledge_owned_response_intent(
                    attempt_run_id,
                    root_id,
                    &job.request_id,
                    &job.dispatch_owner_id,
                )
                .await;
            #[cfg(test)]
            if acknowledgement.is_ok()
                && let Some(barrier) = &job.acknowledgement_barrier
            {
                barrier.committed.notify_one();
                barrier.release.notified().await;
                assert!(
                    !barrier.panic_after_release,
                    "injected post-ack response panic"
                );
            }
            clear_response_state(
                &job.active,
                attempt_run_id,
                &job.request_id,
                acknowledgement.is_ok(),
            );
            acknowledgement?;
            set_status(&job.active, attempt_run_id, RunStatus::Running);
            Ok(())
        }
    }
}

fn clear_response_state(active: &ActiveRun, run_id: RunId, request_id: &str, acknowledged: bool) {
    let mut attempt = active
        .attempt
        .lock()
        .expect("active attempt mutex must not be poisoned");
    if let Some(attempt) = attempt.as_mut()
        && attempt.run_id == run_id
    {
        attempt.response_in_flight = false;
        if acknowledged && attempt.pending_request_id.as_deref() == Some(request_id) {
            attempt.pending_request_id = None;
        }
    }
    drop(attempt);
    active.approval_changed.notify_one();
}

fn clear_response_operation(active: &ActiveRun, operation_id: u64) {
    let mut attempt = active
        .attempt
        .lock()
        .expect("active attempt mutex must not be poisoned");
    if let Some(attempt) = attempt.as_mut()
        && attempt.response_operation_id == Some(operation_id)
    {
        attempt.response_operation_id = None;
    }
}

async fn reconcile_response_panic(
    store: &Store,
    active: &ActiveRun,
    request_id: &str,
    dispatch_owner_id: &str,
) {
    let gate = active.attempt_gate.lock().await;
    let (run_id, root_id) = *active
        .attempt_identity
        .lock()
        .expect("attempt identity mutex must not be poisoned");
    match (
        store.load_approval(run_id, request_id).await,
        store.load_run(run_id).await,
    ) {
        (Ok(approval), Ok(run)) => {
            let acknowledged = approval
                .response_intent
                .as_ref()
                .is_some_and(|intent| intent.status == ApprovalResponseIntentStatus::Acknowledged);
            if !acknowledged {
                let _ = store
                    .reject_owned_response_intent(
                        run_id,
                        root_id,
                        request_id,
                        DispatchCertainty::MayHaveDispatched,
                        dispatch_owner_id,
                    )
                    .await;
            }
            clear_response_state(active, run_id, request_id, acknowledged);
            set_status(active, run_id, run.status);
        }
        _ => {
            signal_attempt_done(active);
            active.shutdown.send_replace(true);
            mark_reconciliation_failed(active);
            drop(gate);
            let _ = shutdown_active_turn(active).await;
        }
    }
}

async fn reconcile_task(
    store: &Store,
    active_runs: &Mutex<HashMap<RunId, Arc<ActiveRun>>>,
    owners: &mut HashMap<Id, TaskOwner>,
    result: Result<(Id, Result<(), RuntimeError>), tokio::task::JoinError>,
) -> bool {
    let id = match &result {
        Ok((id, _)) => *id,
        Err(error) => error.id(),
    };
    let Some(owner) = owners.remove(&id) else {
        return true;
    };
    match owner {
        TaskOwner::Run {
            active,
            dispatch_owner_id,
            admission,
        } => {
            let failed = match result {
                Ok((_, result)) => result.is_err(),
                Err(_) => true,
            };
            if failed {
                signal_attempt_done(&active);
                let _ = shutdown_active_turn(&active).await;
                let (run_id, root_id) = *active
                    .attempt_identity
                    .lock()
                    .expect("attempt identity mutex must not be poisoned");
                let reconciled = store
                    .fail_owned_run_if_active(
                        run_id,
                        root_id,
                        ProviderErrorCategory::ContractViolation,
                        MutationState::Unknown,
                        DispatchCertainty::MayHaveDispatched,
                        &dispatch_owner_id,
                    )
                    .await;
                match reconciled {
                    Ok(true) => set_status(&active, run_id, RunStatus::Failed),
                    Ok(false) => match store.load_run(run_id).await {
                        Ok(run) if is_terminal(run.status) => {
                            set_status(&active, run_id, run.status);
                        }
                        _ => mark_reconciliation_failed(&active),
                    },
                    Err(_) => mark_reconciliation_failed(&active),
                }
            }
            remove_joined_run(active_runs, &active);
            drop(admission);
            failed
        }
        TaskOwner::Response {
            active,
            dispatch_owner_id,
            request_id,
            operation_id,
            admission,
            reply,
        } => match result {
            Ok((_, result)) => {
                clear_response_operation(&active, operation_id);
                let _ = reply.send(result);
                drop(admission);
                false
            }
            Err(_) => {
                reconcile_response_panic(store, &active, &request_id, &dispatch_owner_id).await;
                clear_response_operation(&active, operation_id);
                let _ = reply.send(Err(RuntimeError::OwnedTaskFailed));
                drop(admission);
                true
            }
        },
    }
}

fn signal_attempt_done(active: &ActiveRun) {
    if let Some(state) = active
        .attempt
        .lock()
        .expect("active attempt mutex must not be poisoned")
        .as_mut()
    {
        state.done.send_replace(true);
        state.response_in_flight = false;
    }
    active.approval_changed.notify_waiters();
}

async fn shutdown_active_turn(active: &ActiveRun) -> Result<(), ProviderError> {
    let mut turn = active.turn.lock().await;
    match turn.as_mut() {
        Some(turn) => shutdown_turn(turn).await,
        None => Ok(()),
    }
}

fn unique_active_runs(active: &Mutex<HashMap<RunId, Arc<ActiveRun>>>) -> Vec<Arc<ActiveRun>> {
    let mut unique = Vec::new();
    for run in active
        .lock()
        .expect("active run mutex must not be poisoned")
        .values()
    {
        if !unique.iter().any(|candidate| Arc::ptr_eq(candidate, run)) {
            unique.push(Arc::clone(run));
        }
    }
    unique
}

async fn execute_job(
    store: Store,
    adapters: Arc<HashMap<ProviderId, Arc<dyn ProviderAdapter>>>,
    active_runs: Arc<Mutex<HashMap<RunId, Arc<ActiveRun>>>>,
    job: Job,
) -> Result<(), RuntimeError> {
    let cancellation = job.active.cancellation.subscribe();
    let shutdown = job.active.shutdown.subscribe();
    if *cancellation.borrow() || *shutdown.borrow() {
        interrupt_queued(&store, &job).await?;
        return Ok(());
    }
    let primary = adapters
        .get(&job.request.provider)
        .cloned()
        .ok_or(RuntimeError::MissingAdapter(job.request.provider))?;
    let first = AttemptSpec {
        run_id: job.primary_run_id,
        root_id: job.primary_root_id,
        dispatch_owner_id: job.dispatch_owner_id.clone(),
        dispatch_claimed: job.primary_dispatch_claimed,
        adapter: primary,
        resume_native_id: job.request.native_session_id.clone(),
        fallback: job.request.fallback.clone(),
    };
    let first_result = execute_attempt(&store, &job, first).await?;
    match first_result {
        AttemptResult::FallbackCreated { run, root } => {
            let fallback_request = job
                .request
                .fallback
                .as_ref()
                .expect("a created fallback has its provider-specific request");
            #[cfg(test)]
            if let Some(barrier) = &job.active.fallback_creation_barrier {
                barrier.created.notify_one();
                barrier.release.notified().await;
            }
            *job.active
                .attempt_identity
                .lock()
                .expect("attempt identity mutex must not be poisoned") = (run.id, root.id);
            let adapter = adapters
                .get(&fallback_request.provider)
                .cloned()
                .ok_or(RuntimeError::MissingAdapter(fallback_request.provider))?;
            let mut state = *job.active.state.borrow();
            state.current_run_id = run.id;
            state.fallback_run_id = Some(run.id);
            state.status = RunStatus::Queued;
            let _ = job.active.state.send(state);
            active_runs
                .lock()
                .expect("active run mutex must not be poisoned")
                .insert(run.id, Arc::clone(&job.active));
            let fallback = AttemptSpec {
                run_id: run.id,
                root_id: root.id,
                dispatch_owner_id: job.dispatch_owner_id.clone(),
                dispatch_claimed: true,
                adapter,
                resume_native_id: fallback_request.native_session_id.clone(),
                fallback: None,
            };
            let fallback_job = Job {
                request: RunRequest {
                    turn: fallback_request.turn.clone(),
                    fallback: None,
                    ..job.request.clone()
                },
                primary_run_id: job.primary_run_id,
                primary_root_id: job.primary_root_id,
                primary_dispatch_claimed: job.primary_dispatch_claimed,
                dispatch_owner_id: job.dispatch_owner_id.clone(),
                active: Arc::clone(&job.active),
            };
            let result = execute_attempt(&store, &fallback_job, fallback).await?;
            set_status(&job.active, run.id, result.status());
        }
        result => set_status(&job.active, job.primary_run_id, result.status()),
    }
    Ok(())
}

async fn interrupt_queued(store: &Store, job: &Job) -> Result<(), RuntimeError> {
    #[cfg(test)]
    if let Some(barrier) = &job.active.queued_interrupt_barrier {
        barrier.ready.notify_one();
        barrier.release.notified().await;
    }
    let result = store
        .append_owned_run_event(
            job.primary_run_id,
            job.primary_root_id,
            &job.dispatch_owner_id,
            ProviderEventRecord::interrupted(),
        )
        .await;
    #[cfg(test)]
    if let Some(barrier) = &job.active.queued_interrupt_barrier {
        barrier.finished.notify_one();
    }
    result?;
    set_status(&job.active, job.primary_run_id, RunStatus::Interrupted);
    Ok(())
}

fn remove_joined_run(active: &Mutex<HashMap<RunId, Arc<ActiveRun>>>, completed: &Arc<ActiveRun>) {
    active
        .lock()
        .expect("active run mutex must not be poisoned")
        .retain(|_, candidate| !Arc::ptr_eq(candidate, completed));
}

struct AttemptSpec {
    run_id: RunId,
    root_id: AgentId,
    dispatch_owner_id: String,
    dispatch_claimed: bool,
    adapter: Arc<dyn ProviderAdapter>,
    resume_native_id: Option<String>,
    fallback: Option<FallbackRequest>,
}

#[derive(Clone, Debug)]
enum AttemptResult {
    Completed,
    Interrupted,
    FallbackCreated {
        run: crate::domain::ProviderRun,
        root: Box<crate::domain::AgentNode>,
    },
    Failed,
}

enum AttemptFinish {
    ProviderTerminal(RunStatus),
    Interrupted(MutationState),
    Failed {
        category: ProviderErrorCategory,
        mutation: MutationState,
        dispatch_certainty: DispatchCertainty,
    },
    RuntimeError(RuntimeError),
}

impl AttemptResult {
    fn status(&self) -> RunStatus {
        match self {
            Self::Completed => RunStatus::Completed,
            Self::Interrupted => RunStatus::Interrupted,
            Self::FallbackCreated { .. } | Self::Failed => RunStatus::Failed,
        }
    }
}

async fn execute_attempt(
    store: &Store,
    job: &Job,
    attempt: AttemptSpec,
) -> Result<AttemptResult, RuntimeError> {
    *job.active
        .attempt_identity
        .lock()
        .expect("attempt identity mutex must not be poisoned") = (attempt.run_id, attempt.root_id);
    let mut cancellation = job.active.cancellation.subscribe();
    let mut shutdown = job.active.shutdown.subscribe();
    if *cancellation.borrow() || *shutdown.borrow() {
        return interrupt_before_start(store, &job.active, &attempt).await;
    }
    if !attempt.dispatch_claimed
        && !store
            .claim_provider_dispatch(
                attempt.run_id,
                &attempt.dispatch_owner_id,
                DISPATCH_LEASE_DURATION,
            )
            .await?
    {
        return Err(RuntimeError::DispatchAlreadyClaimed(attempt.run_id));
    }
    if !store
        .refresh_provider_dispatch_lease(
            attempt.run_id,
            &attempt.dispatch_owner_id,
            DISPATCH_LEASE_DURATION,
            DISPATCH_LEASE_STALE_GRACE,
        )
        .await?
    {
        return Err(RuntimeError::DispatchLeaseLost(attempt.run_id));
    }
    let session_request = StartSession {
        conversation_id: job.request.conversation_id,
        working_directory: job.request.working_directory.clone(),
    };
    let session_call = async {
        match &attempt.resume_native_id {
            Some(native_id) => {
                attempt
                    .adapter
                    .resume_session(
                        native_id,
                        ResumeSession {
                            conversation_id: session_request.conversation_id,
                            working_directory: session_request.working_directory,
                        },
                    )
                    .await
            }
            None => attempt.adapter.start_session(session_request).await,
        }
    };
    let session = match race_control(session_call, &mut cancellation, &mut shutdown).await {
        Ok(Ok(session)) if session.provider == attempt.adapter.id() => session,
        Ok(Ok(_)) => {
            return fail_attempt(
                store,
                &job.active,
                &attempt,
                ProviderErrorCategory::ContractViolation,
                MutationState::Unknown,
                DispatchCertainty::MayHaveDispatched,
            )
            .await;
        }
        Ok(Err(error)) => {
            let certainty = error.dispatch_certainty();
            let mutation = if certainty == DispatchCertainty::NotDispatched {
                MutationState::NoneObserved
            } else {
                MutationState::Unknown
            };
            return fail_attempt(
                store,
                &job.active,
                &attempt,
                error.category(),
                mutation,
                certainty,
            )
            .await;
        }
        Err(RuntimeError::OperationCancelled) => {
            return interrupt_before_start(store, &job.active, &attempt).await;
        }
        Err(error) => return Err(error),
    };
    store
        .bind_owned_native_session_with_group(
            attempt.run_id,
            &session.native_id,
            session.native_group_id.as_deref(),
            &attempt.dispatch_owner_id,
        )
        .await?;
    let (done, _) = watch::channel(false);
    *job.active
        .attempt
        .lock()
        .expect("active attempt mutex must not be poisoned") = Some(ActiveAttempt {
        run_id: attempt.run_id,
        root_id: attempt.root_id,
        adapter: Arc::clone(&attempt.adapter),
        session: session.clone(),
        native_turn_id: None,
        pending_request_id: None,
        response_in_flight: false,
        response_operation_id: None,
        done,
    });
    if !store
        .refresh_provider_dispatch_lease(
            attempt.run_id,
            &attempt.dispatch_owner_id,
            DISPATCH_LEASE_DURATION,
            DISPATCH_LEASE_STALE_GRACE,
        )
        .await?
    {
        return Err(RuntimeError::DispatchLeaseLost(attempt.run_id));
    }
    let turn = race_control(
        attempt
            .adapter
            .start_turn(&session, job.request.turn.clone()),
        &mut cancellation,
        &mut shutdown,
    )
    .await;
    let provider_turn = match turn {
        Ok(Ok(turn)) => turn,
        Ok(Err(error)) => {
            let certainty = error.dispatch_certainty();
            let mutation = if certainty == DispatchCertainty::NotDispatched {
                MutationState::NoneObserved
            } else {
                MutationState::Unknown
            };
            return fail_attempt(
                store,
                &job.active,
                &attempt,
                error.category(),
                mutation,
                certainty,
            )
            .await;
        }
        Err(RuntimeError::OperationCancelled) => {
            return interrupt_before_start(store, &job.active, &attempt).await;
        }
        Err(error) => return Err(error),
    };
    let mut turn_slot = job.active.turn.lock().await;
    *turn_slot = Some(provider_turn);
    let turn = turn_slot
        .as_mut()
        .expect("provider turn was stored before processing");
    let mut started = false;
    let mut mutation = MutationState::NoneObserved;
    let mut buffered = VecDeque::new();
    let mut staged_buffered = 0_usize;
    let mut buffered_closed = false;
    loop {
        if approval_pending(&job.active) {
            if buffered_closed && !approval_responding(&job.active) {
                return finalize_attempt(
                    store,
                    &job.active,
                    &attempt,
                    turn,
                    &mut buffered,
                    active_failure(ProviderErrorCategory::StreamClosed, MutationState::Unknown),
                )
                .await;
            }
            let notified = job.active.approval_changed.notified();
            if approval_pending(&job.active) {
                if buffered_closed {
                    tokio::select! {
                        _ = notified => continue,
                        _ = cancellation.changed() => return interrupt_attempt(store, &job.active, &attempt, turn, &mut buffered, mutation).await,
                        _ = shutdown.changed() => return interrupt_attempt(store, &job.active, &attempt, turn, &mut buffered, mutation).await,
                        _ = tokio::time::sleep(RESPONSE_ACK_GRACE_TIMEOUT) => {
                            return finalize_attempt(
                                store,
                                &job.active,
                                &attempt,
                                turn,
                                &mut buffered,
                                active_failure(ProviderErrorCategory::StreamClosed, MutationState::Unknown),
                            ).await;
                        }
                    }
                }
                tokio::select! {
                    biased;
                    _ = notified => continue,
                    _ = cancellation.changed() => return interrupt_attempt(store, &job.active, &attempt, turn, &mut buffered, mutation).await,
                    _ = shutdown.changed() => return interrupt_attempt(store, &job.active, &attempt, turn, &mut buffered, mutation).await,
                    event = turn.recv() => {
                        let Some(event) = event else {
                            if approval_responding(&job.active) {
                                buffered_closed = true;
                                continue;
                            }
                            return finalize_attempt(store, &job.active, &attempt, turn, &mut buffered, active_failure(ProviderErrorCategory::StreamClosed, MutationState::Unknown)).await;
                        };
                        let event = match event {
                            Ok(event) => event,
                            Err(error) => {
                                return finalize_attempt(
                                    store,
                                    &job.active,
                                    &attempt,
                                    turn,
                                    &mut buffered,
                                    active_failure(error.category(), MutationState::Unknown),
                                ).await;
                            }
                        };
                        if event.is_terminal() {
                            signal_attempt_done(&job.active);
                            #[cfg(test)]
                            if let Some(barrier) = &job.active.terminal_receipt_barrier {
                                barrier.received.notify_one();
                                barrier.release.notified().await;
                            }
                            let status = match event {
                                ProviderEvent::TurnCompleted => RunStatus::Completed,
                                ProviderEvent::Interrupted => RunStatus::Interrupted,
                                _ => unreachable!("terminal event classification is exhaustive"),
                            };
                            buffered.clear();
                            return finalize_attempt(
                                store,
                                &job.active,
                                &attempt,
                                turn,
                                &mut buffered,
                                AttemptFinish::ProviderTerminal(status),
                            ).await;
                        }
                        let gate = job.active.attempt_gate.lock().await;
                        if !approval_pending(&job.active) {
                            drop(gate);
                            buffered.push_back(event);
                            continue;
                        }
                        let record = match &event {
                            ProviderEvent::AssistantMessage { content } => {
                                ProviderEventRecord::message(content.clone())
                            }
                            ProviderEvent::AssistantMessageDelta {
                                native_item_id,
                                content,
                            } => ProviderEventRecord::native_message(
                                content.clone(),
                                native_item_id.clone(),
                            ),
                            ProviderEvent::Progress { content } => {
                                ProviderEventRecord::progress(content.clone())
                            }
                            ProviderEvent::ToolActivity {
                                description,
                                mutation: observed,
                            } => ProviderEventRecord::tool(description.clone(), *observed),
                            ProviderEvent::NativeItemActivity {
                                native_item_id,
                                description,
                                mutation: observed,
                            } => ProviderEventRecord::native_item(
                                native_item_id.clone(),
                                description.clone(),
                                *observed,
                            ),
                            ProviderEvent::ChildAgentActivity {
                                native_item_id,
                                parent_native_thread_id,
                                child_native_thread_ids,
                                child_statuses,
                                operation,
                                status,
                            } => ProviderEventRecord::child_agent(
                                native_item_id.clone(),
                                parent_native_thread_id.clone(),
                                child_native_thread_ids.clone(),
                                child_statuses.clone(),
                                operation.clone(),
                                status.clone(),
                            ),
                            ProviderEvent::SubAgentActivity {
                                native_item_id,
                                agent_thread_id,
                                agent_path,
                                activity,
                            } => ProviderEventRecord::sub_agent(
                                native_item_id.clone(),
                                agent_thread_id.clone(),
                                agent_path.clone(),
                                *activity,
                            ),
                            ProviderEvent::Unrecognized { method } => {
                                ProviderEventRecord::unrecognized(method.clone())
                            }
                            _ => {
                                drop(gate);
                                return finalize_attempt(
                                    store,
                                    &job.active,
                                    &attempt,
                                    turn,
                                    &mut buffered,
                                    active_failure(ProviderErrorCategory::ContractViolation, MutationState::Unknown),
                                ).await;
                            }
                        };
                        let stage = store
                            .stage_owned_waiting_event(
                                attempt.run_id,
                                attempt.root_id,
                                &attempt.dispatch_owner_id,
                                record,
                            )
                            .await;
                        drop(gate);
                        match stage {
                            Err(error) => {
                                return finalize_attempt(
                                    store,
                                    &job.active,
                                    &attempt,
                                    turn,
                                    &mut buffered,
                                    AttemptFinish::RuntimeError(error.into()),
                                ).await;
                            }
                            Ok(StageWaitingEventOutcome::Overflowed(_)) => {
                                buffered.clear();
                                return finalize_attempt(
                                    store,
                                    &job.active,
                                    &attempt,
                                    turn,
                                    &mut buffered,
                                    active_failure(
                                        ProviderErrorCategory::ContractViolation,
                                        MutationState::Unknown,
                                    ),
                                ).await;
                            }
                            Ok(StageWaitingEventOutcome::Staged(_)) => {}
                        }
                        if let ProviderEvent::ToolActivity { mutation: observed, .. }
                        | ProviderEvent::NativeItemActivity { mutation: observed, .. } = &event
                        {
                            mutation = merge_mutation(mutation, *observed);
                        }
                        buffered.push_back(event);
                        staged_buffered += 1;
                        continue;
                    }
                }
            }
        }
        if staged_buffered != 0 {
            for _ in 0..staged_buffered {
                let removed = buffered.pop_front();
                debug_assert!(removed.is_some());
            }
            staged_buffered = 0;
        }
        let next = if let Some(event) = buffered.pop_front() {
            Some(Ok(event))
        } else if buffered_closed {
            None
        } else {
            tokio::select! {
                _ = cancellation.changed() => return interrupt_attempt(store, &job.active, &attempt, turn, &mut buffered, mutation).await,
                _ = shutdown.changed() => return interrupt_attempt(store, &job.active, &attempt, turn, &mut buffered, mutation).await,
                event = turn.recv() => event,
            }
        };
        let Some(event) = next else {
            return finalize_attempt(
                store,
                &job.active,
                &attempt,
                turn,
                &mut buffered,
                active_failure(
                    ProviderErrorCategory::StreamClosed,
                    merge_mutation(mutation, MutationState::Unknown),
                ),
            )
            .await;
        };
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                return finalize_attempt(
                    store,
                    &job.active,
                    &attempt,
                    turn,
                    &mut buffered,
                    active_failure(
                        error.category(),
                        merge_mutation(mutation, MutationState::Unknown),
                    ),
                )
                .await;
            }
        };
        match event {
            ProviderEvent::TurnStarted { native_turn_id } if !started => {
                started = true;
                if let Some(active) = job
                    .active
                    .attempt
                    .lock()
                    .expect("active attempt mutex must not be poisoned")
                    .as_mut()
                {
                    active.native_turn_id = Some(native_turn_id.clone());
                }
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::started_with_native_id(native_turn_id),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
                if let Err(error) = store
                    .advance_owned_provider_context(attempt.run_id, &attempt.dispatch_owner_id)
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
                set_status(&job.active, attempt.run_id, RunStatus::Running);
                #[cfg(test)]
                if let Some(barrier) = &job.active.active_turn_panic_barrier {
                    barrier.started.notify_one();
                    barrier.release.notified().await;
                    panic!("injected active-turn panic");
                }
            }
            ProviderEvent::AssistantMessage { content } if started => {
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::message(content),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
            }
            ProviderEvent::AssistantMessageDelta {
                native_item_id,
                content,
            } if started => {
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::native_message(content, native_item_id),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
            }
            ProviderEvent::Progress { content } if started => {
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::progress(content),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
            }
            ProviderEvent::ToolActivity {
                description,
                mutation: observed,
            } if started => {
                mutation = merge_mutation(mutation, observed);
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::tool(description, observed),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
            }
            ProviderEvent::NativeItemActivity {
                native_item_id,
                description,
                mutation: observed,
            } if started => {
                mutation = merge_mutation(mutation, observed);
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::native_item(native_item_id, description, observed),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
            }
            ProviderEvent::ChildAgentActivity {
                native_item_id,
                parent_native_thread_id,
                child_native_thread_ids,
                child_statuses,
                operation,
                status,
            } if started => {
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::child_agent(
                            native_item_id,
                            parent_native_thread_id,
                            child_native_thread_ids,
                            child_statuses,
                            operation,
                            status,
                        ),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
            }
            ProviderEvent::SubAgentActivity {
                native_item_id,
                agent_thread_id,
                agent_path,
                activity,
            } if started => {
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::sub_agent(
                            native_item_id,
                            agent_thread_id,
                            agent_path,
                            activity,
                        ),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
            }
            ProviderEvent::Unrecognized { method } if started => {
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::unrecognized(method),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
            }
            ProviderEvent::ApprovalRequested {
                request_id,
                operation,
                scope,
                details,
            } if started && !approval_pending(&job.active) => {
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::approval_requested_with_details(
                            attempt.adapter.id(),
                            request_id.clone(),
                            operation,
                            scope,
                            details,
                        ),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
                if let Some(active) = job
                    .active
                    .attempt
                    .lock()
                    .expect("active attempt mutex must not be poisoned")
                    .as_mut()
                {
                    active.pending_request_id = Some(request_id);
                }
                set_status(&job.active, attempt.run_id, RunStatus::Waiting);
            }
            ProviderEvent::UserInputRequested {
                request_id,
                questions,
                auto_resolution_ms,
            } if started && !approval_pending(&job.active) => {
                if let Err(error) = store
                    .append_owned_run_event(
                        attempt.run_id,
                        attempt.root_id,
                        &attempt.dispatch_owner_id,
                        ProviderEventRecord::user_input_requested(
                            attempt.adapter.id(),
                            request_id.clone(),
                            questions,
                            auto_resolution_ms,
                        ),
                    )
                    .await
                {
                    return finalize_attempt(
                        store,
                        &job.active,
                        &attempt,
                        turn,
                        &mut buffered,
                        AttemptFinish::RuntimeError(error.into()),
                    )
                    .await;
                }
                if let Some(active) = job
                    .active
                    .attempt
                    .lock()
                    .expect("active attempt mutex must not be poisoned")
                    .as_mut()
                {
                    active.pending_request_id = Some(request_id);
                }
                set_status(&job.active, attempt.run_id, RunStatus::Waiting);
            }
            ProviderEvent::TurnCompleted if started => {
                #[cfg(test)]
                if let Some(barrier) = &job.active.terminal_receipt_barrier {
                    barrier.received.notify_one();
                    barrier.release.notified().await;
                }
                return finalize_attempt(
                    store,
                    &job.active,
                    &attempt,
                    turn,
                    &mut buffered,
                    AttemptFinish::ProviderTerminal(RunStatus::Completed),
                )
                .await;
            }
            ProviderEvent::Interrupted if started => {
                return finalize_attempt(
                    store,
                    &job.active,
                    &attempt,
                    turn,
                    &mut buffered,
                    AttemptFinish::ProviderTerminal(RunStatus::Interrupted),
                )
                .await;
            }
            _ => {
                return finalize_attempt(
                    store,
                    &job.active,
                    &attempt,
                    turn,
                    &mut buffered,
                    active_failure(
                        ProviderErrorCategory::ContractViolation,
                        MutationState::Unknown,
                    ),
                )
                .await;
            }
        }
    }
}

async fn interrupt_before_start(
    store: &Store,
    active: &ActiveRun,
    attempt: &AttemptSpec,
) -> Result<AttemptResult, RuntimeError> {
    store
        .append_owned_run_event(
            attempt.run_id,
            attempt.root_id,
            &attempt.dispatch_owner_id,
            ProviderEventRecord::interrupted(),
        )
        .await?;
    set_status(active, attempt.run_id, RunStatus::Interrupted);
    Ok(AttemptResult::Interrupted)
}

async fn interrupt_attempt(
    store: &Store,
    active: &ActiveRun,
    attempt: &AttemptSpec,
    turn_owner: &mut ProviderTurn,
    buffered: &mut VecDeque<ProviderEvent>,
    mutation: MutationState,
) -> Result<AttemptResult, RuntimeError> {
    finalize_attempt(
        store,
        active,
        attempt,
        turn_owner,
        buffered,
        AttemptFinish::Interrupted(mutation),
    )
    .await
}

async fn fail_attempt(
    store: &Store,
    _active: &ActiveRun,
    attempt: &AttemptSpec,
    category: ProviderErrorCategory,
    mutation: MutationState,
    dispatch_certainty: DispatchCertainty,
) -> Result<AttemptResult, RuntimeError> {
    if mutation == MutationState::NoneObserved
        && dispatch_certainty == DispatchCertainty::NotDispatched
        && let Some(fallback) = &attempt.fallback
    {
        #[cfg(test)]
        if let Some(barrier) = &_active.fallback_transition_barrier {
            barrier.ready.notify_one();
            barrier.release.notified().await;
        }
        let (run, root) = store
            .fail_and_create_owned_fallback(
                attempt.run_id,
                attempt.root_id,
                &attempt.dispatch_owner_id,
                DISPATCH_LEASE_DURATION,
                category,
                NewFallbackAttempt {
                    provider: fallback.provider,
                    native_session_id: fallback.native_session_id.clone(),
                    turn_prompt: fallback.turn.prompt.clone(),
                    handoff_rendered: fallback.handoff_rendered.clone(),
                    handoff_hash: fallback.handoff_hash.clone(),
                    routing_decision: fallback.routing_decision.clone(),
                },
            )
            .await?;
        return Ok(AttemptResult::FallbackCreated {
            run,
            root: Box::new(root),
        });
    }
    store
        .append_owned_run_event(
            attempt.run_id,
            attempt.root_id,
            &attempt.dispatch_owner_id,
            ProviderEventRecord::provider_failed(category, mutation, dispatch_certainty),
        )
        .await?;
    Ok(AttemptResult::Failed)
}

fn active_failure(category: ProviderErrorCategory, mutation: MutationState) -> AttemptFinish {
    AttemptFinish::Failed {
        category,
        mutation,
        dispatch_certainty: DispatchCertainty::MayHaveDispatched,
    }
}

async fn finalize_attempt(
    store: &Store,
    active: &ActiveRun,
    attempt: &AttemptSpec,
    turn: &mut ProviderTurn,
    buffered: &mut VecDeque<ProviderEvent>,
    mut finish: AttemptFinish,
) -> Result<AttemptResult, RuntimeError> {
    signal_attempt_done(active);
    let _gate = active.attempt_gate.lock().await;

    if matches!(finish, AttemptFinish::ProviderTerminal(_)) {
        let pending_request_id = active
            .attempt
            .lock()
            .expect("active attempt mutex must not be poisoned")
            .as_ref()
            .and_then(|attempt| attempt.pending_request_id.clone());
        if let Some(request_id) = pending_request_id {
            match store.load_approval(attempt.run_id, &request_id).await {
                Ok(approval)
                    if approval.response_intent.as_ref().is_some_and(|intent| {
                        intent.status == ApprovalResponseIntentStatus::Acknowledged
                    }) =>
                {
                    clear_response_state(active, attempt.run_id, &request_id, true);
                    set_status(active, attempt.run_id, RunStatus::Running);
                }
                Ok(_) => {}
                Err(error) => finish = AttemptFinish::RuntimeError(error.into()),
            }
        }
    }

    if matches!(finish, AttemptFinish::ProviderTerminal(_)) {
        let stream_closed = if buffered.pop_front().is_some() {
            false
        } else {
            matches!(
                tokio::time::timeout(TERMINAL_CLOSE_TIMEOUT, turn.recv()).await,
                Ok(None)
            )
        };
        if !stream_closed || approval_pending(active) {
            finish = AttemptFinish::Failed {
                category: ProviderErrorCategory::ContractViolation,
                mutation: MutationState::Unknown,
                dispatch_certainty: DispatchCertainty::MayHaveDispatched,
            };
        }
    }

    if let Err(error) = shutdown_turn(turn).await
        && !matches!(finish, AttemptFinish::RuntimeError(_))
    {
        finish = AttemptFinish::Failed {
            category: error.category(),
            mutation: MutationState::Unknown,
            dispatch_certainty: DispatchCertainty::MayHaveDispatched,
        };
    }

    match finish {
        AttemptFinish::ProviderTerminal(RunStatus::Completed) => {
            store
                .append_owned_run_event(
                    attempt.run_id,
                    attempt.root_id,
                    &attempt.dispatch_owner_id,
                    ProviderEventRecord::completed(),
                )
                .await?;
            Ok(AttemptResult::Completed)
        }
        AttemptFinish::ProviderTerminal(RunStatus::Interrupted) => {
            store
                .append_owned_run_event(
                    attempt.run_id,
                    attempt.root_id,
                    &attempt.dispatch_owner_id,
                    ProviderEventRecord::interrupted(),
                )
                .await?;
            Ok(AttemptResult::Interrupted)
        }
        AttemptFinish::ProviderTerminal(_) => {
            unreachable!("provider terminal mapping is exhaustive")
        }
        AttemptFinish::Interrupted(mutation) => {
            let record = if mutation == MutationState::NoneObserved {
                ProviderEventRecord::interrupted()
            } else {
                ProviderEventRecord::interrupted_with_mutation(mutation)
            };
            store
                .append_owned_run_event(
                    attempt.run_id,
                    attempt.root_id,
                    &attempt.dispatch_owner_id,
                    record,
                )
                .await?;
            Ok(AttemptResult::Interrupted)
        }
        AttemptFinish::Failed {
            category,
            mutation,
            dispatch_certainty,
        } => {
            fail_attempt(
                store,
                active,
                attempt,
                category,
                mutation,
                dispatch_certainty,
            )
            .await
        }
        AttemptFinish::RuntimeError(error) => Err(error),
    }
}

async fn shutdown_turn(turn: &mut ProviderTurn) -> Result<(), ProviderError> {
    tokio::time::timeout(TURN_SHUTDOWN_TIMEOUT, turn.shutdown())
        .await
        .map_err(|_| ProviderError::Transport {
            category: "turn_shutdown_timeout".to_owned(),
        })?
}

async fn race_control<T>(
    future: impl Future<Output = T>,
    cancellation: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<T, RuntimeError> {
    if *cancellation.borrow() || *shutdown.borrow() {
        return Err(RuntimeError::OperationCancelled);
    }
    tokio::select! {
        result = future => Ok(result),
        _ = cancellation.changed() => Err(RuntimeError::OperationCancelled),
        _ = shutdown.changed() => Err(RuntimeError::OperationCancelled),
    }
}

async fn race_attempt_control<T>(
    future: impl Future<Output = T>,
    cancellation: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
    done: &mut watch::Receiver<bool>,
) -> Result<T, RuntimeError> {
    if *cancellation.borrow() || *shutdown.borrow() || *done.borrow() {
        return Err(RuntimeError::OperationCancelled);
    }
    tokio::select! {
        result = future => Ok(result),
        _ = cancellation.changed() => Err(RuntimeError::OperationCancelled),
        _ = shutdown.changed() => Err(RuntimeError::OperationCancelled),
        _ = done.changed() => Err(RuntimeError::OperationCancelled),
    }
}

fn approval_pending(active: &ActiveRun) -> bool {
    active
        .attempt
        .lock()
        .expect("active attempt mutex must not be poisoned")
        .as_ref()
        .is_some_and(|attempt| attempt.pending_request_id.is_some())
}

fn approval_responding(active: &ActiveRun) -> bool {
    active
        .attempt
        .lock()
        .expect("active attempt mutex must not be poisoned")
        .as_ref()
        .is_some_and(|attempt| attempt.response_in_flight)
}

fn set_status(active: &ActiveRun, run_id: RunId, status: RunStatus) {
    let mut state = *active.state.borrow();
    state.current_run_id = run_id;
    state.status = status;
    let _ = active.state.send(state);
}

fn mark_reconciliation_failed(active: &ActiveRun) {
    let mut state = *active.state.borrow();
    state.reconciliation_failed = true;
    let _ = active.state.send(state);
}

fn merge_mutation(current: MutationState, observed: MutationState) -> MutationState {
    match (current, observed) {
        (MutationState::Unknown, _) | (_, MutationState::Unknown) => MutationState::Unknown,
        (MutationState::Observed, _) | (_, MutationState::Observed) => MutationState::Observed,
        _ => MutationState::NoneObserved,
    }
}

fn is_terminal(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Completed | RunStatus::Interrupted | RunStatus::Failed
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use tokio::process::Command;

    use super::*;
    use crate::domain::{ApprovalResponseIntentStatus, ApprovalStatus, TimelineEventKind};
    use crate::providers::process::JsonLineProcess;
    use crate::providers::{ProviderCapabilities, ProviderHealth, ProviderTurnOwner};
    use crate::store::{NewConversation, StoreError};

    type HeldTurnSenders = Arc<Mutex<Vec<mpsc::Sender<Result<ProviderEvent, ProviderError>>>>>;

    struct ImmediateAdapter {
        provider: ProviderId,
        reject_before_dispatch: bool,
        start_calls: Option<Arc<AtomicUsize>>,
        held_turns: Option<HeldTurnSenders>,
    }

    struct HungShutdownAdapter(ImmediateAdapter);

    #[async_trait]
    impl ProviderAdapter for ImmediateAdapter {
        fn id(&self) -> ProviderId {
            self.provider
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        async fn health(&self) -> Result<ProviderHealth, ProviderError> {
            Ok(ProviderHealth::Healthy {
                version: "fixture".to_owned(),
            })
        }

        async fn start_session(
            &self,
            _request: StartSession,
        ) -> Result<ProviderSession, ProviderError> {
            if let Some(calls) = &self.start_calls {
                calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(ProviderSession {
                provider: self.provider,
                native_id: format!("{:?}-session", self.provider),
                native_group_id: None,
            })
        }

        async fn resume_session(
            &self,
            _native_id: &str,
            request: ResumeSession,
        ) -> Result<ProviderSession, ProviderError> {
            self.start_session(StartSession {
                conversation_id: request.conversation_id,
                working_directory: request.working_directory,
            })
            .await
        }

        async fn start_turn(
            &self,
            _session: &ProviderSession,
            _request: TurnRequest,
        ) -> Result<ProviderTurn, ProviderError> {
            if self.reject_before_dispatch {
                return Err(ProviderError::NotDispatched {
                    category: ProviderErrorCategory::Rejected,
                });
            }
            let (sender, receiver) = mpsc::channel(2);
            sender
                .send(Ok(ProviderEvent::TurnStarted {
                    native_turn_id: "fixture-turn".to_owned(),
                }))
                .await
                .unwrap();
            if let Some(held_turns) = &self.held_turns {
                held_turns.lock().unwrap().push(sender);
                return Ok(ProviderTurn::new(
                    receiver,
                    CountingOwner(Arc::new(AtomicUsize::new(0))),
                ));
            }
            sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
            drop(sender);
            Ok(ProviderTurn::new(
                receiver,
                CountingOwner(Arc::new(AtomicUsize::new(0))),
            ))
        }

        async fn steer(
            &self,
            _session: &ProviderSession,
            _active_turn: &str,
            _text: &str,
        ) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn respond(
            &self,
            _session: &ProviderSession,
            _request_id: &str,
            _response: ApprovalResponse,
        ) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn interrupt(
            &self,
            _session: &ProviderSession,
            _active_turn: &str,
        ) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ProviderAdapter for HungShutdownAdapter {
        fn id(&self) -> ProviderId {
            self.0.id()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            self.0.capabilities()
        }

        async fn health(&self) -> Result<ProviderHealth, ProviderError> {
            self.0.health().await
        }

        async fn start_session(
            &self,
            request: StartSession,
        ) -> Result<ProviderSession, ProviderError> {
            self.0.start_session(request).await
        }

        async fn resume_session(
            &self,
            native_id: &str,
            request: ResumeSession,
        ) -> Result<ProviderSession, ProviderError> {
            self.0.resume_session(native_id, request).await
        }

        async fn start_turn(
            &self,
            session: &ProviderSession,
            request: TurnRequest,
        ) -> Result<ProviderTurn, ProviderError> {
            self.0.start_turn(session, request).await
        }

        async fn steer(
            &self,
            session: &ProviderSession,
            active_turn: &str,
            text: &str,
        ) -> Result<(), ProviderError> {
            self.0.steer(session, active_turn, text).await
        }

        async fn respond(
            &self,
            session: &ProviderSession,
            request_id: &str,
            response: ApprovalResponse,
        ) -> Result<(), ProviderError> {
            self.0.respond(session, request_id, response).await
        }

        async fn interrupt(
            &self,
            session: &ProviderSession,
            active_turn: &str,
        ) -> Result<(), ProviderError> {
            self.0.interrupt(session, active_turn).await
        }

        async fn shutdown(&self) -> Result<(), ProviderError> {
            std::future::pending().await
        }
    }

    fn test_submission(
        command_id: &str,
        conversation_id: ConversationId,
        provider: ProviderId,
    ) -> NewSubmission {
        let decision = crate::router::Router::default()
            .route(
                crate::router::RouteRequest::builder("fixture")
                    .eligible([crate::router::ProviderRoutingState::available(
                        provider,
                        ProviderCapabilities::default(),
                    )])
                    .override_provider(provider)
                    .build(),
            )
            .unwrap();
        NewSubmission {
            command_id: command_id.to_owned(),
            request_hash: format!("hash-{command_id}"),
            conversation_id,
            provider,
            content: "fixture".to_owned(),
            routing_decision: decision,
            handoff_rendered: None,
            handoff_hash: None,
            turn_prompt: "fixture turn".to_owned(),
        }
    }

    #[tokio::test]
    async fn recovery_claim_precedes_admission_and_loser_never_dispatches() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("recovery ownership race"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, root } = store
            .prepare_submission(test_submission(
                "recovery-ownership-race",
                conversation.id,
                ProviderId::Codex,
            ))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };
        let starts = Arc::new(AtomicUsize::new(0));
        let supervisor = RunSupervisor::new(
            store.clone(),
            vec![Arc::new(ImmediateAdapter {
                provider: ProviderId::Codex,
                reject_before_dispatch: false,
                start_calls: Some(Arc::clone(&starts)),
                held_turns: None,
            })],
        )
        .unwrap();
        let claim_barrier = Arc::new(RecoveryClaimBarrier::new());
        supervisor.hold_recovery_claim_for_test(Arc::clone(&claim_barrier));
        let supervisor = Arc::new(supervisor);
        let run_id = run.id;
        let root_id = root.id;
        let recovery = {
            let supervisor = Arc::clone(&supervisor);
            tokio::spawn(async move {
                let Some(claim) = supervisor.claim_recovery_run(run.id, false).await? else {
                    return Err(RuntimeError::DispatchAlreadyClaimed(run.id));
                };
                supervisor
                    .recover_persisted(
                        RunRequest::new(
                            conversation.id,
                            PathBuf::from("/tmp/recovery-ownership-race"),
                            ProviderId::Codex,
                            TurnRequest::new("fixture"),
                        ),
                        run,
                        root,
                        &claim,
                    )
                    .await
            })
        };

        claim_barrier.ready.notified().await;
        let permits_before_release = supervisor.available_root_admission_for_test();
        assert!(
            store
                .claim_provider_dispatch(run_id, "recovery-owner-b", DISPATCH_LEASE_DURATION,)
                .await
                .unwrap()
        );
        store
            .append_run_event(run_id, root_id, ProviderEventRecord::interrupted())
            .await
            .unwrap();
        claim_barrier.release.notify_one();

        assert!(matches!(
            recovery.await.unwrap(),
            Err(RuntimeError::DispatchAlreadyClaimed(id)) if id == run_id
        ));
        assert_eq!(permits_before_release, MAX_ADMITTED_ROOT_RUNS);
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.load_run(run_id).await.unwrap().status,
            RunStatus::Interrupted
        );
        supervisor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn recovery_promotion_loser_releases_admission_and_never_dispatches() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("recovery promotion race"))
            .await
            .unwrap();
        let PreparedSubmission::Created { run, root } = store
            .prepare_submission(test_submission(
                "recovery-promotion-race",
                conversation.id,
                ProviderId::Codex,
            ))
            .await
            .unwrap()
        else {
            panic!("fixture must create a queued run");
        };
        let starts = Arc::new(AtomicUsize::new(0));
        let supervisor = Arc::new(
            RunSupervisor::new(
                store.clone(),
                vec![Arc::new(ImmediateAdapter {
                    provider: ProviderId::Codex,
                    reject_before_dispatch: false,
                    start_calls: Some(Arc::clone(&starts)),
                    held_turns: None,
                })],
            )
            .unwrap(),
        );
        let claim = supervisor
            .claim_recovery_run(run.id, false)
            .await
            .unwrap()
            .expect("the only reconciler must claim the run");
        let promotion = Arc::new(RecoveryPromotionBarrier::new());
        supervisor.hold_recovery_promotion_for_test(Arc::clone(&promotion));
        let run_id = run.id;
        let recovery = {
            let supervisor = Arc::clone(&supervisor);
            tokio::spawn(async move {
                supervisor
                    .recover_persisted(
                        RunRequest::new(
                            conversation.id,
                            PathBuf::from("/tmp/recovery-promotion-race"),
                            ProviderId::Codex,
                            TurnRequest::new("fixture"),
                        ),
                        run,
                        root,
                        &claim,
                    )
                    .await
            })
        };

        promotion.ready.notified().await;
        store
            .replace_dispatch_owner_for_test(run_id, "winning-reconciler")
            .await
            .unwrap();
        promotion.release.notify_one();

        assert!(matches!(
            recovery.await.unwrap(),
            Err(RuntimeError::DispatchAlreadyClaimed(id)) if id == run_id
        ));
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert_eq!(
            supervisor.available_root_admission_for_test(),
            MAX_ADMITTED_ROOT_RUNS
        );
        assert_eq!(
            store.load_run(run_id).await.unwrap().status,
            RunStatus::Queued
        );
        supervisor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn atomic_fallback_creation_blocks_concurrent_submit_and_archive() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("fallback reservation"))
            .await
            .unwrap();
        let mut supervisor = RunSupervisor::new(
            store.clone(),
            vec![
                Arc::new(ImmediateAdapter {
                    provider: ProviderId::Codex,
                    reject_before_dispatch: true,
                    start_calls: None,
                    held_turns: None,
                }),
                Arc::new(ImmediateAdapter {
                    provider: ProviderId::Claude,
                    reject_before_dispatch: false,
                    start_calls: None,
                    held_turns: None,
                }),
            ],
        )
        .unwrap();
        let barrier = Arc::new(FallbackCreationBarrier::new());
        supervisor.set_fallback_creation_barrier(Arc::clone(&barrier));
        let first = RunRequest::new(
            conversation.id,
            std::path::PathBuf::from("."),
            ProviderId::Codex,
            TurnRequest::new("fixture"),
        )
        .with_fallback(ProviderId::Claude);
        let handle = supervisor
            .submit_persisted(
                first,
                test_submission("first", conversation.id, ProviderId::Codex),
            )
            .await
            .unwrap()
            .handle;

        barrier.created.notified().await;
        let second = supervisor.submit_persisted(
            RunRequest::new(
                conversation.id,
                std::path::PathBuf::from("."),
                ProviderId::Codex,
                TurnRequest::new("second"),
            ),
            test_submission("second", conversation.id, ProviderId::Codex),
        );
        let archive = store.archive_conversation(conversation.id);
        let (second, archive) = tokio::join!(second, archive);
        let second_error = match second {
            Ok(_) => panic!("queued fallback must reject a concurrent submission"),
            Err(error) => error,
        };
        assert!(matches!(
            second_error,
            RuntimeError::Store(StoreError::ConversationBusy(id)) if id == conversation.id
        ));
        assert!(matches!(
            archive.unwrap_err(),
            StoreError::ConversationBusy(id) if id == conversation.id
        ));

        barrier.release.notify_one();
        let outcome = handle.wait().await.unwrap();
        assert_eq!(outcome.status, RunStatus::Completed);
        assert!(outcome.fallback_run_id.is_some());
        supervisor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn fallback_transition_stops_when_the_primary_lease_owner_changes() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("fallback owner fence"))
            .await
            .unwrap();
        let primary_starts = Arc::new(AtomicUsize::new(0));
        let fallback_starts = Arc::new(AtomicUsize::new(0));
        let mut supervisor = RunSupervisor::new(
            store.clone(),
            vec![
                Arc::new(ImmediateAdapter {
                    provider: ProviderId::Codex,
                    reject_before_dispatch: true,
                    start_calls: Some(Arc::clone(&primary_starts)),
                    held_turns: None,
                }),
                Arc::new(ImmediateAdapter {
                    provider: ProviderId::Claude,
                    reject_before_dispatch: false,
                    start_calls: Some(Arc::clone(&fallback_starts)),
                    held_turns: None,
                }),
            ],
        )
        .unwrap();
        let barrier = Arc::new(FallbackTransitionBarrier::new());
        supervisor.set_fallback_transition_barrier(Arc::clone(&barrier));
        let handle = supervisor
            .submit_persisted(
                RunRequest::new(
                    conversation.id,
                    std::path::PathBuf::from("."),
                    ProviderId::Codex,
                    TurnRequest::new("fixture"),
                )
                .with_fallback(ProviderId::Claude),
                test_submission("owner-fence", conversation.id, ProviderId::Codex),
            )
            .await
            .unwrap()
            .handle;

        barrier.ready.notified().await;
        store
            .replace_dispatch_owner_for_test(handle.run_id(), "other-recovery")
            .await
            .unwrap();
        barrier.release.notify_one();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), handle.wait())
                .await
                .expect("owner-fenced fallback task must terminate"),
            Err(RuntimeError::ReconciliationFailed)
        ));
        assert_eq!(primary_starts.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_starts.load(Ordering::SeqCst), 0);
        assert!(
            store
                .load_submission("owner-fence")
                .await
                .unwrap()
                .unwrap()
                .fallback_run
                .is_none()
        );
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn transferred_atomic_fallback_is_never_dispatched_by_its_former_owner() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("fallback child owner fence"))
            .await
            .unwrap();
        let fallback_starts = Arc::new(AtomicUsize::new(0));
        let mut supervisor = RunSupervisor::new(
            store.clone(),
            vec![
                Arc::new(ImmediateAdapter {
                    provider: ProviderId::Codex,
                    reject_before_dispatch: true,
                    start_calls: None,
                    held_turns: None,
                }),
                Arc::new(ImmediateAdapter {
                    provider: ProviderId::Claude,
                    reject_before_dispatch: false,
                    start_calls: Some(Arc::clone(&fallback_starts)),
                    held_turns: None,
                }),
            ],
        )
        .unwrap();
        let barrier = Arc::new(FallbackCreationBarrier::new());
        supervisor.set_fallback_creation_barrier(Arc::clone(&barrier));
        let handle = supervisor
            .submit_persisted(
                RunRequest::new(
                    conversation.id,
                    PathBuf::from("."),
                    ProviderId::Codex,
                    TurnRequest::new("fixture"),
                )
                .with_fallback(ProviderId::Claude),
                test_submission(
                    "fallback-child-owner-fence",
                    conversation.id,
                    ProviderId::Codex,
                ),
            )
            .await
            .unwrap()
            .handle;

        barrier.created.notified().await;
        let fallback_run_id = store
            .load_submission("fallback-child-owner-fence")
            .await
            .unwrap()
            .unwrap()
            .fallback_run
            .expect("fallback is committed before the barrier")
            .id;
        let fallback_before_transfer = store.load_run(fallback_run_id).await.unwrap();
        assert_eq!(fallback_before_transfer.status, RunStatus::Queued);
        assert_eq!(
            fallback_before_transfer.dispatch_certainty,
            Some(DispatchCertainty::MayHaveDispatched)
        );
        assert!(
            store
                .has_protected_provider_dispatch_lease(fallback_run_id, DISPATCH_LEASE_STALE_GRACE,)
                .await
                .unwrap()
        );
        store
            .replace_dispatch_owner_for_test(fallback_run_id, "winning-reconciler")
            .await
            .unwrap();
        barrier.release.notify_one();

        assert!(matches!(
            handle.wait().await,
            Err(RuntimeError::ReconciliationFailed)
        ));
        assert_eq!(fallback_starts.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.load_run(fallback_run_id).await.unwrap().status,
            RunStatus::Queued
        );
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    struct CountingOwner(Arc<AtomicUsize>);

    #[async_trait]
    impl ProviderTurnOwner for CountingOwner {
        async fn shutdown(self: Box<Self>) -> Result<(), ProviderError> {
            tokio::task::yield_now().await;
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct ProcessOwner(Option<JsonLineProcess>);

    #[async_trait]
    impl ProviderTurnOwner for ProcessOwner {
        async fn shutdown(mut self: Box<Self>) -> Result<(), ProviderError> {
            self.0
                .take()
                .expect("test process owner shuts down once")
                .shutdown()
                .await
        }
    }

    struct ApprovalAdapter {
        sender: Mutex<Option<mpsc::Sender<Result<ProviderEvent, ProviderError>>>>,
        responses: AtomicUsize,
        owner_shutdowns: Arc<AtomicUsize>,
        control_started: AtomicUsize,
        block_steer: bool,
        response_barrier: Option<Arc<ResponseControlBarrier>>,
        response_drops: Arc<AtomicUsize>,
        owned_pid: Option<Arc<AtomicUsize>>,
        panic_response: bool,
    }

    struct ResponseControlBarrier {
        started: Notify,
        release: Notify,
    }

    impl ResponseControlBarrier {
        fn new() -> Self {
            Self {
                started: Notify::new(),
                release: Notify::new(),
            }
        }
    }

    struct ResponseDropGuard(Arc<AtomicUsize>);

    impl Drop for ResponseDropGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl ProviderAdapter for ApprovalAdapter {
        fn id(&self) -> ProviderId {
            ProviderId::Codex
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        async fn health(&self) -> Result<ProviderHealth, ProviderError> {
            Ok(ProviderHealth::Healthy {
                version: "fixture".to_owned(),
            })
        }

        async fn start_session(
            &self,
            _request: StartSession,
        ) -> Result<ProviderSession, ProviderError> {
            Ok(ProviderSession {
                provider: ProviderId::Codex,
                native_id: "fixture-session".to_owned(),
                native_group_id: None,
            })
        }

        async fn resume_session(
            &self,
            _native_id: &str,
            _request: ResumeSession,
        ) -> Result<ProviderSession, ProviderError> {
            unreachable!()
        }

        async fn start_turn(
            &self,
            _session: &ProviderSession,
            _request: TurnRequest,
        ) -> Result<ProviderTurn, ProviderError> {
            let (sender, receiver) = mpsc::channel(4);
            sender
                .send(Ok(ProviderEvent::TurnStarted {
                    native_turn_id: "fixture-turn".to_owned(),
                }))
                .await
                .unwrap();
            sender
                .send(Ok(ProviderEvent::ApprovalRequested {
                    request_id: "fixture-approval".to_owned(),
                    operation: "write".to_owned(),
                    scope: "fixture.txt".to_owned(),
                    details: None,
                }))
                .await
                .unwrap();
            *self.sender.lock().unwrap() = Some(sender);
            if let Some(owned_pid) = &self.owned_pid {
                let mut command = Command::new("/bin/sh");
                command.arg("-c").arg("sleep 30");
                let process = JsonLineProcess::spawn(command)?;
                owned_pid.store(process.id() as usize, Ordering::SeqCst);
                Ok(ProviderTurn::new(receiver, ProcessOwner(Some(process))))
            } else {
                Ok(ProviderTurn::new(
                    receiver,
                    CountingOwner(Arc::clone(&self.owner_shutdowns)),
                ))
            }
        }

        async fn steer(
            &self,
            _session: &ProviderSession,
            _active_turn: &str,
            _text: &str,
        ) -> Result<(), ProviderError> {
            if self.block_steer {
                self.control_started.fetch_add(1, Ordering::SeqCst);
                std::future::pending().await
            }
            Ok(())
        }

        async fn respond(
            &self,
            _session: &ProviderSession,
            _request_id: &str,
            _response: ApprovalResponse,
        ) -> Result<(), ProviderError> {
            self.responses.fetch_add(1, Ordering::SeqCst);
            if let Some(barrier) = &self.response_barrier {
                let _drop_guard = ResponseDropGuard(Arc::clone(&self.response_drops));
                barrier.started.notify_one();
                barrier.release.notified().await;
            }
            assert!(!self.panic_response, "injected response panic");
            Ok(())
        }

        async fn interrupt(
            &self,
            _session: &ProviderSession,
            _active_turn: &str,
        ) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn terminal_after_durable_response_intent_never_dispatches_to_a_dead_attempt() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("intent race"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let barrier = Arc::new(ResponseIntentBarrier::new());
        let terminal = Arc::new(TerminalReceiptBarrier::new());
        let mut supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
        supervisor.set_response_intent_barrier(Arc::clone(&barrier));
        supervisor.set_terminal_receipt_barrier(Arc::clone(&terminal));
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/intent-race"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };

        barrier.committed.notified().await;
        let sender = adapter.sender.lock().unwrap().take().unwrap();
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        drop(sender);
        terminal.received.notified().await;
        barrier.release.notify_one();

        assert!(response.await.unwrap().is_err());
        assert_eq!(adapter.responses.load(Ordering::SeqCst), 0);
        terminal.release.notify_one();
        let outcome = tokio::time::timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("terminal finalization must wake the run handle")
            .unwrap();
        assert_eq!(outcome.status, RunStatus::Failed);
        supervisor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stale_owner_cannot_dispatch_or_acknowledge_an_approval_response() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("approval ownership fence"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let barrier = Arc::new(ResponseIntentBarrier::new());
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        supervisor.set_response_intent_barrier(Arc::clone(&barrier));
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/approval-ownership-fence"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };

        barrier.committed.notified().await;
        store
            .replace_dispatch_owner_for_test(handle.run_id(), "recovery-owner")
            .await
            .unwrap();
        barrier.release.notify_one();

        assert!(response.await.unwrap().is_err());
        assert_eq!(adapter.responses.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.load_run(handle.run_id()).await.unwrap().status,
            RunStatus::Waiting
        );
        let approval = store
            .load_approval(handle.run_id(), "fixture-approval")
            .await
            .unwrap();
        assert_eq!(approval.status, ApprovalStatus::Pending);
        assert!(
            approval
                .response_intent
                .as_ref()
                .is_some_and(|intent| { intent.status == ApprovalResponseIntentStatus::Recorded })
        );
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn owner_transfer_after_provider_response_dispatch_blocks_stale_acknowledgement() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("post-dispatch approval fence"))
            .await
            .unwrap();
        let pre_acknowledgement = Arc::new(ResponsePreAcknowledgementBarrier::new());
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        supervisor.set_response_pre_acknowledgement_barrier(Arc::clone(&pre_acknowledgement));
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/post-dispatch-approval-fence"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };

        pre_acknowledgement.ready.notified().await;
        store
            .replace_dispatch_owner_for_test(handle.run_id(), "current-owner")
            .await
            .unwrap();
        pre_acknowledgement.release.notify_one();

        assert!(matches!(
            response.await.unwrap(),
            Err(RuntimeError::Store(StoreError::DispatchOwnerMismatch(id))) if id == handle.run_id()
        ));
        assert_eq!(adapter.responses.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.load_run(handle.run_id()).await.unwrap().status,
            RunStatus::Waiting
        );
        let approval = store
            .load_approval(handle.run_id(), "fixture-approval")
            .await
            .unwrap();
        assert_eq!(approval.status, ApprovalStatus::Pending);
        assert!(
            approval
                .response_intent
                .as_ref()
                .is_some_and(|intent| { intent.status == ApprovalResponseIntentStatus::Recorded })
        );
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn terminal_received_before_response_forbids_provider_dispatch() {
        for close_stream in [false, true] {
            let store = Store::open_in_memory().await.unwrap();
            let conversation = store
                .create_conversation(NewConversation::projectless("terminal race"))
                .await
                .unwrap();
            let adapter = Arc::new(ApprovalAdapter {
                sender: Mutex::new(None),
                responses: AtomicUsize::new(0),
                owner_shutdowns: Arc::new(AtomicUsize::new(0)),
                control_started: AtomicUsize::new(0),
                block_steer: false,
                response_barrier: None,
                response_drops: Arc::new(AtomicUsize::new(0)),
                owned_pid: None,
                panic_response: false,
            });
            let barrier = Arc::new(TerminalReceiptBarrier::new());
            let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
            supervisor.set_terminal_receipt_barrier(Arc::clone(&barrier));
            let supervisor = Arc::new(supervisor);
            let handle = supervisor
                .submit(RunRequest::new(
                    conversation.id,
                    PathBuf::from("/tmp/terminal-race"),
                    ProviderId::Codex,
                    TurnRequest::new("fixture"),
                ))
                .await
                .unwrap();
            handle.wait_for(RunStatus::Waiting).await.unwrap();
            let sender = adapter.sender.lock().unwrap().as_ref().unwrap().clone();
            sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
            if close_stream {
                adapter.sender.lock().unwrap().take();
                drop(sender);
            }
            barrier.received.notified().await;

            let response = supervisor
                .respond(
                    handle.run_id(),
                    "fixture-approval",
                    ApprovalResponse::Approved,
                )
                .await;
            assert!(matches!(response, Err(RuntimeError::OperationCancelled)));
            assert_eq!(adapter.responses.load(Ordering::SeqCst), 0);
            barrier.release.notify_one();

            let outcome = tokio::time::timeout(Duration::from_secs(2), handle.wait())
                .await
                .expect("terminal receipt must bound finalization")
                .unwrap();
            assert_eq!(outcome.status, RunStatus::Failed);
            assert_eq!(adapter.owner_shutdowns.load(Ordering::SeqCst), 1);
            assert_eq!(
                store.load_run(handle.run_id()).await.unwrap().status,
                RunStatus::Failed
            );
            let _ = supervisor.shutdown().await;
        }
    }

    #[tokio::test]
    async fn stale_owner_cannot_finalize_a_received_provider_terminal() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("terminal ownership fence"))
            .await
            .unwrap();
        let terminal = Arc::new(TerminalReceiptBarrier::new());
        let completion = Arc::new(OwnedTaskCompletionBarrier::new());
        let mut supervisor = RunSupervisor::new(
            store.clone(),
            vec![Arc::new(ImmediateAdapter {
                provider: ProviderId::Codex,
                reject_before_dispatch: false,
                start_calls: None,
                held_turns: None,
            })],
        )
        .unwrap();
        supervisor.set_terminal_receipt_barrier(Arc::clone(&terminal));
        supervisor.set_root_task_completion_barrier(Arc::clone(&completion));
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/terminal-ownership-fence"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();

        terminal.received.notified().await;
        store
            .replace_dispatch_owner_for_test(handle.run_id(), "recovery-owner")
            .await
            .unwrap();
        terminal.release.notify_one();
        completion.completed.notified().await;

        assert_eq!(
            store.load_run(handle.run_id()).await.unwrap().status,
            RunStatus::Running
        );
        completion.release.notify_one();
        assert!(matches!(
            handle.wait().await,
            Err(RuntimeError::ReconciliationFailed)
        ));
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn active_turn_panic_cancels_controls_and_awaits_owner_shutdown() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("active panic"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: true,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let barrier = Arc::new(ActiveTurnPanicBarrier::new());
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        supervisor.set_active_turn_panic_barrier(Arc::clone(&barrier));
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/active-panic"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        barrier.started.notified().await;
        let control = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move { supervisor.steer(run_id, "blocked").await })
        };
        while adapter.control_started.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        barrier.release.notify_one();

        let outcome = tokio::time::timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("panic reconciliation must wake the run handle")
            .unwrap();
        assert_eq!(outcome.status, RunStatus::Failed);
        tokio::time::timeout(Duration::from_secs(2), control)
            .await
            .expect("panic reconciliation must wake attempt controls")
            .unwrap()
            .expect_err("control cannot outlive its attempt");
        assert_eq!(adapter.owner_shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.load_run(handle.run_id()).await.unwrap().status,
            RunStatus::Failed
        );
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn stale_owner_panic_reconciliation_cannot_fail_the_transferred_run() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("panic ownership fence"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let barrier = Arc::new(ActiveTurnPanicBarrier::new());
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
        supervisor.set_active_turn_panic_barrier(Arc::clone(&barrier));
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/panic-ownership-fence"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();

        barrier.started.notified().await;
        store
            .replace_dispatch_owner_for_test(handle.run_id(), "recovery-owner")
            .await
            .unwrap();
        barrier.release.notify_one();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), handle.wait())
                .await
                .expect("panic reconciliation must finish"),
            Err(RuntimeError::ReconciliationFailed)
        ));
        assert_eq!(
            store.load_run(handle.run_id()).await.unwrap().status,
            RunStatus::Running
        );
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn aborting_response_waiter_does_not_cancel_provider_acknowledgement() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("abandoned response"))
            .await
            .unwrap();
        let response_barrier = Arc::new(ResponseControlBarrier::new());
        let response_drops = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: Some(Arc::clone(&response_barrier)),
            response_drops: Arc::clone(&response_drops),
            owned_pid: None,
            panic_response: false,
        });
        let supervisor =
            Arc::new(RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap());
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/abandoned-response"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };
        response_barrier.started.notified().await;
        let sender = adapter.sender.lock().unwrap().as_ref().unwrap().clone();
        sender
            .send(Ok(ProviderEvent::Progress {
                content: "staged before acknowledgement".to_owned(),
            }))
            .await
            .unwrap();
        response.abort();
        assert!(response.await.unwrap_err().is_cancelled());
        assert_eq!(response_drops.load(Ordering::SeqCst), 0);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let staged = store
                    .pending_recovery()
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|run| run.run.id == handle.run_id())
                    .is_some_and(|run| {
                        run.staged_events
                            .iter()
                            .any(|event| event.content == "staged before acknowledgement")
                    });
                if staged {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("provider output must be durably staged before acknowledgement");
        response_barrier.release.notify_one();

        tokio::time::timeout(Duration::from_secs(2), handle.wait_for(RunStatus::Running))
            .await
            .expect("owned response must finish after its waiter is dropped")
            .unwrap();
        assert!(matches!(
            supervisor
                .respond(
                    handle.run_id(),
                    "fixture-approval",
                    ApprovalResponse::Approved,
                )
                .await,
            Err(RuntimeError::Store(
                StoreError::ApprovalResponseAlreadyAcknowledged
            ))
        ));
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        adapter.sender.lock().unwrap().take();
        drop(sender);
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
        let timeline = store
            .load_timeline(conversation.id, None, 20)
            .await
            .unwrap();
        assert_eq!(
            timeline
                .items
                .iter()
                .filter(|event| event.content == "staged before acknowledgement")
                .count(),
            1
        );
        supervisor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn aborting_response_waiter_after_ack_does_not_skip_runtime_cleanup() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("ack cleanup"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let acknowledgement = Arc::new(ResponseAcknowledgementBarrier::new());
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        supervisor.set_response_acknowledgement_barrier(Arc::clone(&acknowledgement));
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/ack-cleanup"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };
        acknowledgement.committed.notified().await;
        response.abort();
        assert!(response.await.unwrap_err().is_cancelled());
        acknowledgement.release.notify_one();

        tokio::time::timeout(Duration::from_secs(2), handle.wait_for(RunStatus::Running))
            .await
            .expect("owned response must publish its in-memory acknowledgement")
            .unwrap();
        let sender = adapter.sender.lock().unwrap().take().unwrap();
        sender
            .send(Ok(ProviderEvent::AssistantMessage {
                content: "after acknowledged cleanup".to_owned(),
            }))
            .await
            .unwrap();
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        drop(sender);
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
        let timeline = store
            .load_timeline(conversation.id, None, 20)
            .await
            .unwrap();
        assert_eq!(
            timeline
                .items
                .iter()
                .filter(|event| {
                    event.kind == TimelineEventKind::Message
                        && event.content == "after acknowledged cleanup"
                })
                .count(),
            1
        );
        supervisor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_cancels_an_owned_response_after_its_waiter_is_dropped() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("response shutdown"))
            .await
            .unwrap();
        let response_barrier = Arc::new(ResponseControlBarrier::new());
        let response_drops = Arc::new(AtomicUsize::new(0));
        let owned_pid = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: Some(Arc::clone(&response_barrier)),
            response_drops: Arc::clone(&response_drops),
            owned_pid: Some(Arc::clone(&owned_pid)),
            panic_response: false,
        });
        let supervisor =
            Arc::new(RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap());
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/response-shutdown"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };
        response_barrier.started.notified().await;
        response.abort();
        assert!(response.await.unwrap_err().is_cancelled());
        assert_eq!(response_drops.load(Ordering::SeqCst), 0);

        tokio::time::timeout(Duration::from_secs(2), supervisor.shutdown())
            .await
            .expect("shutdown must join the abandoned response operation")
            .unwrap();
        assert_eq!(response_drops.load(Ordering::SeqCst), 1);
        let pid = owned_pid.load(Ordering::SeqCst);
        assert_ne!(pid, 0);
        assert!(
            !std::process::Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success()),
            "supervisor shutdown must reap the turn owner's child process"
        );
        let approval = store
            .load_approval(handle.run_id(), "fixture-approval")
            .await
            .unwrap();
        assert_eq!(approval.status, ApprovalStatus::Cancelled);
        assert!(matches!(
            approval.response_intent.unwrap().status,
            ApprovalResponseIntentStatus::Recorded | ApprovalResponseIntentStatus::DispatchUnknown
        ));
    }

    #[tokio::test]
    async fn panicked_owned_response_wakes_waiter_and_reconciles_bookkeeping() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("response panic"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: true,
        });
        let supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/response-panic"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();

        assert!(matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                supervisor.respond(
                    handle.run_id(),
                    "fixture-approval",
                    ApprovalResponse::Approved,
                ),
            )
            .await
            .expect("manager reconciliation must wake the response waiter"),
            Err(RuntimeError::OwnedTaskFailed)
        ));
        let approval = store
            .load_approval(handle.run_id(), "fixture-approval")
            .await
            .unwrap();
        assert_eq!(approval.status, ApprovalStatus::Pending);
        assert_eq!(
            approval.response_intent.unwrap().status,
            ApprovalResponseIntentStatus::DispatchUnknown
        );
        supervisor.interrupt(handle.run_id()).await.unwrap();
        handle.wait_for(RunStatus::Interrupted).await.unwrap();
        assert_eq!(adapter.owner_shutdowns.load(Ordering::SeqCst), 1);
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn panic_after_durable_ack_clears_pending_state_before_stream_continues() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("post-ack panic"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let acknowledgement = Arc::new(ResponseAcknowledgementBarrier::panicking());
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        supervisor.set_response_acknowledgement_barrier(Arc::clone(&acknowledgement));
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/post-ack-panic"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();

        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };
        acknowledgement.committed.notified().await;
        acknowledgement.release.notify_one();
        assert!(matches!(
            response.await.unwrap(),
            Err(RuntimeError::OwnedTaskFailed)
        ));
        handle.wait_for(RunStatus::Running).await.unwrap();
        let sender = adapter.sender.lock().unwrap().take().unwrap();
        sender
            .send(Ok(ProviderEvent::AssistantMessage {
                content: "after post-ack panic".to_owned(),
            }))
            .await
            .unwrap();
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        drop(sender);

        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
        let timeline = store
            .load_timeline(conversation.id, None, 20)
            .await
            .unwrap();
        assert_eq!(
            timeline
                .items
                .iter()
                .filter(|event| event.content == "after post-ack panic")
                .count(),
            1
        );
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn terminal_queued_behind_post_ack_panic_remains_terminal() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("ack terminal race"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let acknowledgement = Arc::new(ResponseAcknowledgementBarrier::panicking());
        let terminal = Arc::new(TerminalReceiptBarrier::new());
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter.clone()]).unwrap();
        supervisor.set_response_acknowledgement_barrier(Arc::clone(&acknowledgement));
        supervisor.set_terminal_receipt_barrier(Arc::clone(&terminal));
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/ack-terminal-race"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };
        acknowledgement.committed.notified().await;
        let sender = adapter.sender.lock().unwrap().take().unwrap();
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        drop(sender);
        terminal.received.notified().await;
        terminal.release.notify_one();
        tokio::task::yield_now().await;
        acknowledgement.release.notify_one();

        assert!(matches!(
            response.await.unwrap(),
            Err(RuntimeError::OwnedTaskFailed)
        ));
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
        assert_eq!(
            store.load_run(handle.run_id()).await.unwrap().status,
            RunStatus::Completed
        );
        assert_eq!(adapter.owner_shutdowns.load(Ordering::SeqCst), 1);
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn response_panic_with_closed_store_does_not_deadlock_turn_cleanup() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("panic read failure"))
            .await
            .unwrap();
        let owner_shutdowns = Arc::new(AtomicUsize::new(0));
        let response_barrier = Arc::new(ResponseControlBarrier::new());
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::clone(&owner_shutdowns),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: Some(Arc::clone(&response_barrier)),
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: true,
        });
        let supervisor = Arc::new(RunSupervisor::new(store.clone(), vec![adapter]).unwrap());
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/panic-read-failure"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };
        response_barrier.started.notified().await;
        store.clone().close().await;
        response_barrier.release.notify_one();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), response)
                .await
                .expect("response panic reconciliation must not deadlock")
                .unwrap(),
            Err(RuntimeError::OwnedTaskFailed)
        ));
        assert!(matches!(
            handle.wait().await,
            Err(RuntimeError::ReconciliationFailed)
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), supervisor.shutdown())
                .await
                .expect("shutdown must join owner cleanup after Store read failure"),
            Err(RuntimeError::OwnedTaskFailed)
        ));
        assert_eq!(owner_shutdowns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn root_admission_is_released_only_after_manager_reconciliation() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("root permit ordering"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let completion = Arc::new(OwnedTaskCompletionBarrier::new());
        let mut supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
        supervisor.set_root_task_completion_barrier(Arc::clone(&completion));
        let admission = Arc::clone(&supervisor.root_admission);
        let held = (0..MAX_ADMITTED_ROOT_RUNS - 1)
            .map(|_| Arc::clone(&admission).try_acquire_owned().unwrap())
            .collect::<Vec<_>>();
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/root-permit-ordering"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        supervisor
            .respond(
                handle.run_id(),
                "fixture-approval",
                ApprovalResponse::Approved,
            )
            .await
            .unwrap();
        handle.wait_for(RunStatus::Running).await.unwrap();
        let sender = adapter.sender.lock().unwrap().take().unwrap();
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        drop(sender);
        completion.completed.notified().await;
        assert_eq!(handle.status(), RunStatus::Completed);
        assert_eq!(admission.available_permits(), 0);
        assert!(matches!(
            supervisor
                .submit(RunRequest::new(
                    conversation.id,
                    PathBuf::from("/tmp/root-permit-ordering"),
                    ProviderId::Codex,
                    TurnRequest::new("must remain bounded"),
                ))
                .await,
            Err(RuntimeError::RunQueueFull { .. })
        ));

        completion.release.notify_one();
        handle.wait().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while admission.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("manager reconciliation must release root admission");
        drop(held);
        supervisor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stale_owner_cannot_interrupt_a_queued_run_after_transfer() {
        let store = Store::open_in_memory().await.unwrap();
        let held_turns = Arc::new(Mutex::new(Vec::new()));
        let adapter = Arc::new(ImmediateAdapter {
            provider: ProviderId::Codex,
            reject_before_dispatch: false,
            start_calls: None,
            held_turns: Some(Arc::clone(&held_turns)),
        });
        let barrier = Arc::new(QueuedInterruptBarrier::new());
        let mut supervisor = RunSupervisor::new(store.clone(), vec![adapter]).unwrap();
        supervisor.set_queued_interrupt_barrier(Arc::clone(&barrier));
        let supervisor = Arc::new(supervisor);
        let mut handles = Vec::new();
        for index in 0..5 {
            let conversation = store
                .create_conversation(NewConversation::projectless(format!(
                    "queued ownership fence {index}"
                )))
                .await
                .unwrap();
            handles.push(
                supervisor
                    .submit(RunRequest::new(
                        conversation.id,
                        PathBuf::from("/tmp/queued-ownership-fence"),
                        ProviderId::Codex,
                        TurnRequest::new("fixture"),
                    ))
                    .await
                    .unwrap(),
            );
        }
        for handle in handles.iter().take(MAX_CONCURRENT_ROOT_RUNS) {
            handle.wait_for(RunStatus::Running).await.unwrap();
        }
        let queued = handles.last().unwrap();
        assert_eq!(queued.status(), RunStatus::Queued);

        supervisor.interrupt(queued.run_id()).await.unwrap();
        barrier.ready.notified().await;
        store
            .replace_dispatch_owner_for_test(queued.run_id(), "recovery-owner")
            .await
            .unwrap();
        barrier.release.notify_one();
        barrier.finished.notified().await;

        assert_eq!(
            store.load_run(queued.run_id()).await.unwrap().status,
            RunStatus::Queued
        );
        held_turns.lock().unwrap().clear();
        assert!(matches!(
            supervisor.shutdown().await,
            Err(RuntimeError::OwnedTaskFailed)
        ));
    }

    #[tokio::test]
    async fn bounded_shutdown_forces_and_awaits_a_stuck_owned_run_task() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("forced manager shutdown"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let completion = Arc::new(OwnedTaskCompletionBarrier::new());
        let mut supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
        supervisor.set_root_task_completion_barrier(Arc::clone(&completion));
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/forced-manager-shutdown"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        supervisor
            .respond(
                handle.run_id(),
                "fixture-approval",
                ApprovalResponse::Approved,
            )
            .await
            .unwrap();
        let sender = adapter.sender.lock().unwrap().take().unwrap();
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        drop(sender);
        completion.completed.notified().await;
        // Local test ownership, the supervisor fixture, the active run, and its task.
        assert_eq!(Arc::strong_count(&completion), 4);

        supervisor
            .shutdown_with_grace(Duration::from_millis(10))
            .await
            .unwrap();

        assert_eq!(Arc::strong_count(&completion), 2);
        assert!(supervisor.manager.lock().await.is_none());
    }

    #[tokio::test]
    async fn forced_shutdown_is_bounded_when_an_adapter_reaper_is_stuck() {
        let store = Store::open_in_memory().await.unwrap();
        let supervisor = RunSupervisor::new(
            store,
            vec![Arc::new(HungShutdownAdapter(ImmediateAdapter {
                provider: ProviderId::Codex,
                reject_before_dispatch: false,
                start_calls: None,
                held_turns: None,
            }))],
        )
        .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(3), supervisor.force_shutdown())
            .await
            .expect("forced shutdown must retain a hard deadline");

        assert!(matches!(result, Err(RuntimeError::AdapterShutdownTimedOut)));
        assert!(supervisor.manager.lock().await.is_none());
    }

    #[tokio::test]
    async fn response_admission_is_released_only_after_manager_reconciliation() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("response permit ordering"))
            .await
            .unwrap();
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: None,
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let completion = Arc::new(OwnedTaskCompletionBarrier::new());
        let mut supervisor = RunSupervisor::new(store, vec![adapter.clone()]).unwrap();
        supervisor.set_response_task_completion_barrier(Arc::clone(&completion));
        let admission = Arc::clone(&supervisor.response_admission);
        let held = (0..MAX_CONCURRENT_APPROVAL_RESPONSES - 1)
            .map(|_| Arc::clone(&admission).try_acquire_owned().unwrap())
            .collect::<Vec<_>>();
        let supervisor = Arc::new(supervisor);
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/response-permit-ordering"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let response = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(run_id, "fixture-approval", ApprovalResponse::Approved)
                    .await
            })
        };
        completion.completed.notified().await;
        assert_eq!(admission.available_permits(), 0);
        assert!(matches!(
            supervisor
                .respond(
                    handle.run_id(),
                    "fixture-approval",
                    ApprovalResponse::Approved,
                )
                .await,
            Err(RuntimeError::ApprovalResponseBusy { .. })
        ));

        completion.release.notify_one();
        response.await.unwrap().unwrap();
        assert_eq!(admission.available_permits(), 1);
        drop(held);
        let sender = adapter.sender.lock().unwrap().take().unwrap();
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        drop(sender);
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
        supervisor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_large_approval_responses_admit_only_one_owned_operation() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = store
            .create_conversation(NewConversation::projectless("bounded responses"))
            .await
            .unwrap();
        let response_barrier = Arc::new(ResponseControlBarrier::new());
        let adapter = Arc::new(ApprovalAdapter {
            sender: Mutex::new(None),
            responses: AtomicUsize::new(0),
            owner_shutdowns: Arc::new(AtomicUsize::new(0)),
            control_started: AtomicUsize::new(0),
            block_steer: false,
            response_barrier: Some(Arc::clone(&response_barrier)),
            response_drops: Arc::new(AtomicUsize::new(0)),
            owned_pid: None,
            panic_response: false,
        });
        let supervisor = Arc::new(RunSupervisor::new(store, vec![adapter.clone()]).unwrap());
        let handle = supervisor
            .submit(RunRequest::new(
                conversation.id,
                PathBuf::from("/tmp/bounded-responses"),
                ProviderId::Codex,
                TurnRequest::new("fixture"),
            ))
            .await
            .unwrap();
        handle.wait_for(RunStatus::Waiting).await.unwrap();
        let first = {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            tokio::spawn(async move {
                supervisor
                    .respond(
                        run_id,
                        "fixture-approval",
                        ApprovalResponse::Answer("x".repeat(MAX_APPROVAL_RESPONSE_BYTES)),
                    )
                    .await
            })
        };
        response_barrier.started.notified().await;

        let mut duplicates = Vec::new();
        for _ in 0..32 {
            let supervisor = Arc::clone(&supervisor);
            let run_id = handle.run_id();
            duplicates.push(tokio::spawn(async move {
                supervisor
                    .respond(
                        run_id,
                        "fixture-approval",
                        ApprovalResponse::Answer("y".repeat(MAX_APPROVAL_RESPONSE_BYTES)),
                    )
                    .await
            }));
        }
        for duplicate in duplicates {
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(1), duplicate)
                    .await
                    .expect("duplicate responses must be rejected without queueing")
                    .unwrap(),
                Err(RuntimeError::ApprovalResponseBusy { .. })
            ));
        }
        assert_eq!(adapter.responses.load(Ordering::SeqCst), 1);
        assert!(matches!(
            supervisor
                .respond(
                    handle.run_id(),
                    "fixture-approval",
                    ApprovalResponse::Answer("z".repeat(MAX_APPROVAL_RESPONSE_BYTES + 1)),
                )
                .await,
            Err(RuntimeError::ApprovalResponseTooLarge { limit })
                if limit == MAX_APPROVAL_RESPONSE_BYTES
        ));

        response_barrier.release.notify_one();
        first.await.unwrap().unwrap();
        let sender = adapter.sender.lock().unwrap().take().unwrap();
        sender.send(Ok(ProviderEvent::TurnCompleted)).await.unwrap();
        drop(sender);
        assert_eq!(handle.wait().await.unwrap().status, RunStatus::Completed);
        supervisor.shutdown().await.unwrap();
    }
}
