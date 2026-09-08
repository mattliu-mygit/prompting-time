//! Bounded metadata-only ancestry and native child turn/control ownership.
//! Child transcript content is never forwarded through this boundary.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::time::Instant;

use super::{
    DispatcherState, JsonLineSender, MAX_PENDING_REQUESTS, PendingResponse, ProviderError,
    ProviderEvent, REQUEST_TIMEOUT, RpcId, handle_server_message, protocol, required_string,
    required_write,
};

const MAX_AGENTS: usize = 128;
const MAX_DEPTH: u64 = 32;
const MAX_ACTIVITIES: usize = 1024;
const MAX_QUEUED_EVENTS: usize = 128;
const MAX_QUEUED_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct Identity {
    id: String,
    parent: String,
    path: Option<String>,
    depth: u64,
}

#[derive(Default)]
pub(super) struct Children {
    identities: HashMap<String, Identity>,
    activities: HashMap<String, ProviderEvent>,
    pub resolving: bool,
    pub terminal_received: bool,
    queued: VecDeque<Value>,
    queued_bytes: usize,
    pub native: Arc<NativeTurns>,
}

#[derive(Default)]
pub(super) struct NativeTurns {
    pub turns: Mutex<HashMap<String, ChildTurn>>,
    pub changed: tokio::sync::Notify,
}

impl NativeTurns {
    pub fn live(&self) -> Vec<super::NativeChildTurn> {
        self.turns
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, turn)| !turn.ended)
            .map(|(thread, turn)| super::NativeChildTurn {
                native_thread_id: thread.clone(),
                native_turn_id: turn.id.clone(),
            })
            .collect()
    }

    pub fn seal(&self, thread: &str, id: &str) {
        if let Some(turn) = self.turns.lock().unwrap().get_mut(thread)
            && turn.id == id
        {
            turn.terminal_received = true;
        }
    }
}

pub(super) struct ChildTurn {
    pub id: String,
    pub ended: bool,
    pub terminal_received: bool,
    pub history: HashSet<String>,
    pub file_changes: HashMap<String, Vec<super::FileChangeApprovalDetail>>,
}

impl Children {
    pub fn new(native: Arc<NativeTurns>) -> Self {
        Self {
            native,
            ..Self::default()
        }
    }
    pub fn start(&mut self, thread: &str, id: &str) -> Result<bool, ProviderError> {
        validate_id(id)?;
        let mut turns = self.native.turns.lock().unwrap();
        if let Some(turn) = turns.get_mut(thread) {
            if turn.id == id && !turn.ended {
                return Ok(false);
            }
            if !turn.ended || turn.history.contains(id) || turn.history.len() >= 1024 {
                return Err(protocol("child-turn-conflict"));
            }
            turn.id = id.to_owned();
            turn.ended = false;
            turn.terminal_received = false;
            turn.history.insert(id.to_owned());
            turn.file_changes.clear();
        } else {
            if !self.identities.contains_key(thread) {
                return Err(protocol("child-identity-unresolved"));
            }
            turns.insert(
                thread.to_owned(),
                ChildTurn {
                    id: id.to_owned(),
                    ended: false,
                    terminal_received: false,
                    history: HashSet::from([id.to_owned()]),
                    file_changes: HashMap::new(),
                },
            );
        }
        Ok(true)
    }
    pub fn queue(&mut self, message: Value) -> Result<(), ProviderError> {
        let bytes = serde_json::to_vec(&message)
            .map_err(|_| protocol("child-event-encoding"))?
            .len();
        if self.queued.len() >= MAX_QUEUED_EVENTS || bytes > MAX_QUEUED_BYTES - self.queued_bytes {
            return Err(protocol("child-event-capacity-exceeded"));
        }
        self.queued_bytes += bytes;
        self.queued.push_back(message);
        Ok(())
    }

    fn depth(&self, root: &str, id: &str) -> Option<u64> {
        if id == root {
            Some(0)
        } else {
            self.identities.get(id).map(|identity| identity.depth)
        }
    }

    fn insert(&mut self, identity: Identity) -> Result<(), ProviderError> {
        if let Some(existing) = self.identities.get_mut(&identity.id) {
            if existing.parent != identity.parent
                || existing.depth != identity.depth
                || existing
                    .path
                    .as_ref()
                    .zip(identity.path.as_ref())
                    .is_some_and(|(a, b)| a != b)
            {
                return Err(protocol("child-identity-conflict"));
            }
            if existing.path.is_none() {
                existing.path = identity.path;
            }
        } else {
            if self.identities.len() >= MAX_AGENTS {
                return Err(protocol("child-identity-capacity-exceeded"));
            }
            self.identities.insert(identity.id.clone(), identity);
        }
        Ok(())
    }

    /// Legacy spawn declarations already carry a direct parent. Other collab
    /// operations do not establish parentage for newly observed receivers.
    pub fn observe_spawn(
        &mut self,
        root: &str,
        event: &ProviderEvent,
    ) -> Result<(), ProviderError> {
        if let ProviderEvent::ChildAgentActivity {
            parent_native_thread_id,
            child_native_thread_ids,
            operation,
            ..
        } = event
            && operation == "spawnAgent"
        {
            let depth = self
                .depth(root, parent_native_thread_id)
                .ok_or_else(|| protocol("child-parent-unresolved"))?
                + 1;
            for id in child_native_thread_ids {
                validate_id(id)?;
                if id == root || id == parent_native_thread_id || depth > MAX_DEPTH {
                    return Err(protocol("child-ancestry-conflict"));
                }
                self.insert(Identity {
                    id: id.clone(),
                    parent: parent_native_thread_id.clone(),
                    path: None,
                    depth,
                })?;
            }
        }
        Ok(())
    }

    pub fn observe_activity(&mut self, event: &ProviderEvent) -> Result<bool, ProviderError> {
        let ProviderEvent::SubAgentActivity {
            native_item_id,
            agent_thread_id,
            agent_path,
            ..
        } = event
        else {
            return Ok(true);
        };
        validate_id(native_item_id)?;
        validate_id(agent_thread_id)?;
        validate_path(agent_path)?;
        let identity = self
            .identities
            .get_mut(agent_thread_id)
            .ok_or_else(|| protocol("child-identity-unresolved"))?;
        if identity
            .path
            .as_ref()
            .is_some_and(|path| path != agent_path)
        {
            return Err(protocol("child-path-conflict"));
        }
        identity.path = Some(agent_path.clone());
        if let Some(previous) = self.activities.get(native_item_id) {
            if previous != event {
                return Err(protocol("child-activity-conflict"));
            }
            return Ok(false);
        }
        if self.activities.len() >= MAX_ACTIVITIES {
            return Err(protocol("child-activity-capacity-exceeded"));
        }
        self.activities
            .insert(native_item_id.clone(), event.clone());
        Ok(true)
    }
}

pub(super) struct Lookup {
    roots: Vec<(String, u64)>,
    pub deadline: Instant,
    requested: String,
    activity: Option<(String, String)>,
    chain: Vec<Identity>,
    queued: VecDeque<Value>,
    queued_bytes: usize,
}

impl Lookup {
    pub fn is_discovery(&self) -> bool {
        self.activity.is_none()
    }
    pub fn is_active(&self, state: &DispatcherState) -> bool {
        self.roots.iter().any(|(root, registration)| {
            state
                .turns
                .get(root)
                .is_some_and(|turn| turn.registration_id == *registration && !turn.cancelled)
        })
    }
}

pub(super) fn activity_owner(
    method: &str,
    params: &Value,
    state: &DispatcherState,
) -> Result<Option<String>, ProviderError> {
    if !matches!(method, "item/started" | "item/completed")
        || params.pointer("/item/type").and_then(Value::as_str) != Some("subAgentActivity")
    {
        return Ok(None);
    }
    let Some(observer) = params.get("threadId").and_then(Value::as_str) else {
        return Ok(None);
    };
    // Only already verified ancestry associates an observer with a root.
    let mut owners = state
        .turns
        .iter()
        .filter(|(_, turn)| !turn.cancelled && turn.children.identities.contains_key(observer));
    let owner = owners.next().map(|(root, _)| root.clone());
    if owners.next().is_some() {
        return Err(protocol("child-root-conflict"));
    }
    Ok(owner)
}

pub(super) fn owner(
    state: &DispatcherState,
    thread: &str,
) -> Result<Option<String>, ProviderError> {
    let mut roots = state.turns.iter().filter(|(root, turn)| {
        root.as_str() == thread || turn.children.identities.contains_key(thread)
    });
    let root = roots.next().map(|(root, _)| root.clone());
    if roots.next().is_some() {
        return Err(protocol("child-root-conflict"));
    }
    Ok(root)
}

pub(super) fn active(
    state: &DispatcherState,
    thread: &str,
    id: &str,
) -> Result<Option<String>, ProviderError> {
    let Some(root) = owner(state, thread)? else {
        return Ok(None);
    };
    let turn = &state.turns[&root];
    if turn.cancelled
        || turn.interrupt_pending
        || turn.children.terminal_received
        || turn.provisional_terminal
    {
        return Ok(None);
    }
    let matches = if root == thread {
        super::active_or_announced_turn_id(turn) == Some(id)
    } else {
        turn.children
            .native
            .turns
            .lock()
            .unwrap()
            .get(thread)
            .is_some_and(|turn| turn.id == id && !turn.ended && !turn.terminal_received)
    };
    Ok(matches.then_some(root))
}

pub(super) async fn announce_thread(
    params: &Value,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let id = required_string(params, &["thread", "id"], "thread-started-id")?;
    if state.turns.contains_key(&id)
        || params
            .pointer("/thread/parentThreadId")
            .is_none_or(Value::is_null)
    {
        return Ok(());
    }
    let identity = parse_metadata(&id, params)?;
    if let Some(root) = owner(state, &identity.parent)? {
        let turn = &state.turns[&root];
        if turn.cancelled || turn.children.terminal_received {
            return Ok(());
        }
        if turn
            .children
            .depth(&root, &identity.parent)
            .map(|depth| depth + 1)
            != Some(identity.depth)
        {
            return Err(protocol("child-depth-conflict"));
        }
        publish_identity(&root, identity, None, sender, state).await?;
    }
    Ok(())
}

async fn publish_identity(
    root: &str,
    identity: Identity,
    activity_id: Option<String>,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let event = match activity_id {
        Some(native_item_id) => ProviderEvent::ChildAgentActivity {
            native_item_id,
            parent_native_thread_id: identity.parent.clone(),
            child_native_thread_ids: vec![identity.id.clone()],
            child_statuses: Vec::new(),
            operation: "resolveIdentity".into(),
            status: "observed".into(),
        },
        None => ProviderEvent::NativeChildIdentity {
            parent_native_thread_id: identity.parent.clone(),
            native_thread_id: identity.id.clone(),
        },
    };
    state
        .turns
        .get_mut(root)
        .expect("registered root")
        .children
        .insert(identity)?;
    super::deliver_or_buffer_turn_event(state, root, Ok(event), false, sender).await
}

/// Hold observations from a lookup target without trusting that target yet.
/// They are replayed only after the complete chain reaches the registered root.
pub(super) fn pending_activity_owner(
    method: &str,
    params: &Value,
    state: &DispatcherState,
) -> Result<Option<String>, ProviderError> {
    if !matches!(method, "item/started" | "item/completed")
        || params.pointer("/item/type").and_then(Value::as_str) != Some("subAgentActivity")
    {
        return Ok(None);
    }
    let Some(observer) = params.get("threadId").and_then(Value::as_str) else {
        return Ok(None);
    };
    let mut owner = None;
    for pending in state.pending.values() {
        if let PendingResponse::ChildMetadata(lookup) = pending
            && (lookup.requested == observer
                || lookup.chain.iter().any(|identity| identity.id == observer))
            && lookup.activity.is_some()
            && lookup.is_active(state)
        {
            let root = &lookup.roots[0].0;
            if owner.as_ref().is_some_and(|previous| previous != root) {
                return Err(protocol("child-root-conflict"));
            }
            owner = Some(root.clone());
        }
    }
    Ok(owner)
}

pub(super) async fn resolve_activity(
    root: &str,
    method: &str,
    params: &Value,
    event: &ProviderEvent,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<bool, ProviderError> {
    let ProviderEvent::SubAgentActivity {
        native_item_id,
        agent_thread_id,
        agent_path,
        ..
    } = event
    else {
        return Ok(false);
    };
    validate_id(agent_thread_id)?;
    validate_id(native_item_id)?;
    validate_path(agent_path)?;
    if agent_thread_id == root {
        return Err(protocol("child-is-root"));
    }
    let turn = state
        .turns
        .get_mut(root)
        .ok_or_else(|| protocol("child-root-not-active"))?;
    if turn.cancelled || turn.interrupt_pending {
        return Ok(true);
    }
    if turn.children.identities.contains_key(agent_thread_id) {
        return Ok(false);
    }
    turn.children
        .queue(json!({"method": method, "params": params}))?;
    turn.children.resolving = true;
    let lookup = Lookup {
        roots: vec![(root.to_owned(), turn.registration_id)],
        deadline: Instant::now() + REQUEST_TIMEOUT,
        requested: agent_thread_id.clone(),
        activity: Some((native_item_id.clone(), agent_path.clone())),
        chain: Vec::new(),
        queued: VecDeque::new(),
        queued_bytes: 0,
    };
    request_metadata(lookup, sender, state).await?;
    Ok(true)
}

async fn request_metadata(
    lookup: Lookup,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    if state.pending.len() >= MAX_PENDING_REQUESTS {
        return Err(protocol("too-many-pending-requests"));
    }
    let id = RpcId::Number(state.next_id);
    state.next_id = state
        .next_id
        .checked_add(1)
        .ok_or_else(|| protocol("request-id-exhausted"))?;
    let request = json!({"id":id, "method":"thread/read", "params":{"threadId":lookup.requested, "includeTurns":false}});
    state
        .pending
        .insert(id, PendingResponse::ChildMetadata(lookup));
    required_write(sender, &state.process_shutdown, request).await
}

pub(super) async fn accept_metadata(
    mut lookup: Lookup,
    result: Value,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    if !lookup.is_active(state) {
        return Ok(());
    }
    if lookup.deadline <= Instant::now() {
        return Err(protocol("child-metadata-timeout"));
    }
    if lookup.is_discovery()
        && result.pointer("/thread/id").and_then(Value::as_str) == Some(lookup.requested.as_str())
        && result
            .pointer("/thread/parentThreadId")
            .is_none_or(Value::is_null)
    {
        return reject_discovery(lookup, sender, state).await;
    }
    let identity = parse_metadata(&lookup.requested, &result)?;
    if lookup.roots.iter().any(|(root, _)| root == &identity.id)
        || identity.parent == identity.id
        || lookup
            .chain
            .iter()
            .any(|ancestor| ancestor.id == identity.id || ancestor.id == identity.parent)
        || lookup.chain.len() >= MAX_DEPTH as usize
    {
        return Err(protocol("child-ancestry-conflict"));
    }
    if lookup.chain.is_empty()
        && lookup.activity.as_ref().is_some_and(|(_, activity_path)| {
            identity
                .path
                .as_ref()
                .is_some_and(|path| path != activity_path)
        })
    {
        return Err(protocol("child-path-conflict"));
    }
    let mut owners = lookup.roots.iter().filter_map(|(root, registration)| {
        state
            .turns
            .get(root)
            .filter(|turn| turn.registration_id == *registration && !turn.cancelled)
            .and_then(|turn| turn.children.depth(root, &identity.parent))
            .map(|depth| (root.clone(), depth))
    });
    let parent = owners.next();
    if owners.next().is_some() {
        return Err(protocol("child-root-conflict"));
    }
    lookup.requested = identity.parent.clone();
    lookup.chain.push(identity);
    let Some((root, mut depth)) = parent else {
        return request_metadata(lookup, sender, state).await;
    };
    // Validate the entire chain before publishing any identity into the run.
    for identity in lookup.chain.iter().rev() {
        depth += 1;
        if identity.depth != depth {
            return Err(protocol("child-depth-conflict"));
        }
    }
    if state.turns[&root].children.identities.len() + lookup.chain.len() > MAX_AGENTS {
        return Err(protocol("child-identity-capacity-exceeded"));
    }
    for identity in lookup.chain.into_iter().rev() {
        publish_identity(
            &root,
            identity,
            lookup.activity.as_ref().map(|(id, _)| id.clone()),
            sender,
            state,
        )
        .await?;
    }
    // Replay the discovered child's own start/control first; root output and terminal
    // were fenced behind discovery. No candidate was trusted before this point.
    let target = &mut state.turns.get_mut(&root).expect("verified root").children;
    for message in lookup.queued.into_iter().rev() {
        let bytes = serde_json::to_vec(&message)
            .map_err(|_| protocol("child-event-encoding"))?
            .len();
        if target.queued.len() >= MAX_QUEUED_EVENTS
            || bytes > MAX_QUEUED_BYTES - target.queued_bytes
        {
            return Err(protocol("child-event-capacity-exceeded"));
        }
        target.queued_bytes += bytes;
        target.queued.push_front(message);
    }
    flush_resolved_roots(&lookup.roots, sender, state).await?;
    Ok(())
}

async fn flush_resolved_roots(
    roots: &[(String, u64)],
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    for (root, registration) in roots {
        let resolving = state.pending.values().any(|pending| matches!(pending, PendingResponse::ChildMetadata(lookup) if lookup.roots.contains(&(root.clone(), *registration))));
        let Some(turn) = state
            .turns
            .get_mut(root)
            .filter(|turn| turn.registration_id == *registration && !turn.cancelled)
        else {
            continue;
        };
        turn.children.resolving = resolving;
        if resolving {
            continue;
        }
        let queued = std::mem::take(&mut turn.children.queued);
        turn.children.queued_bytes = 0;
        for message in queued {
            Box::pin(handle_server_message(message, sender, state)).await?;
        }
    }
    Ok(())
}

pub(super) async fn reject_discovery(
    lookup: Lookup,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    for message in lookup.queued {
        if let Some(id) = message.get("id").and_then(super::parse_rpc_id) {
            required_write(sender, &state.process_shutdown, json!({"id":id,"error":{"code":-32003,"message":"Request owner could not be verified"}})).await?;
            super::remember_server_request_tombstone(state, id);
        }
    }
    flush_resolved_roots(&lookup.roots, sender, state).await
}

pub(super) async fn expire_discoveries(
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let expired: Vec<_> = state.pending.iter().filter_map(|(id, pending)| {
        matches!(pending, PendingResponse::ChildMetadata(lookup) if lookup.is_discovery() && (!lookup.is_active(state) || lookup.deadline <= Instant::now())).then_some(id.clone())
    }).collect();
    for id in expired {
        if let Some(PendingResponse::ChildMetadata(lookup)) = state.pending.remove(&id) {
            reject_discovery(lookup, sender, state).await?;
        }
    }
    Ok(())
}

/// Unknown starts trigger metadata-only discovery against a snapshot of owned
/// registrations. The candidate list is a fence, never evidence of parentage.
pub(super) async fn correlate(
    message: &Value,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<bool, ProviderError> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(false);
    };
    let Some(params) = message.get("params") else {
        return Ok(false);
    };
    let thread = if method == "thread/started" {
        params.pointer("/thread/id")
    } else {
        params.get("threadId")
    };
    let Some(thread) = thread.and_then(Value::as_str) else {
        return Ok(false);
    };
    if let Some(root) = owner(state, thread)? {
        // Receipt seals the exact child before delayed lineage/output replay.
        if method == "turn/completed" && root != thread {
            let id = params.pointer("/turn/id").and_then(Value::as_str);
            if let Some(id) = id {
                state.turns[&root].children.native.seal(thread, id);
            }
        }
        if root != thread && state.turns[&root].children.resolving && !state.turns[&root].cancelled
        {
            state
                .turns
                .get_mut(&root)
                .unwrap()
                .children
                .queue(message.clone())?;
            return Ok(true);
        }
        return Ok(false);
    }
    let (pending_rows, pending_bytes) =
        state
            .pending
            .values()
            .fold((0usize, 0usize), |(rows, bytes), pending| match pending {
                PendingResponse::ChildMetadata(lookup) if lookup.is_discovery() => {
                    (rows + lookup.queued.len(), bytes + lookup.queued_bytes)
                }
                _ => (rows, bytes),
            });
    let bytes = serde_json::to_vec(message)
        .map_err(|_| protocol("child-event-encoding"))?
        .len();
    for pending in state.pending.values_mut() {
        if let PendingResponse::ChildMetadata(lookup) = pending
            && lookup.activity.is_none()
            && (lookup.requested == thread
                || lookup.chain.iter().any(|identity| identity.id == thread))
        {
            if pending_rows >= MAX_QUEUED_EVENTS
                || bytes > MAX_QUEUED_BYTES.saturating_sub(pending_bytes)
            {
                return Err(protocol("child-event-capacity-exceeded"));
            }
            lookup.queued_bytes += bytes;
            lookup.queued.push_back(message.clone());
            return Ok(true);
        }
    }
    if !matches!(method, "thread/started" | "turn/started") {
        return Ok(false);
    }
    if method == "thread/started"
        && params
            .pointer("/thread/parentThreadId")
            .is_none_or(Value::is_null)
    {
        return Ok(false);
    }
    validate_id(thread)?;
    let roots: Vec<_> = state
        .turns
        .iter()
        .filter(|(_, turn)| !turn.cancelled && !turn.children.terminal_received)
        .map(|(root, turn)| (root.clone(), turn.registration_id))
        .collect();
    if roots.is_empty() {
        return Ok(false);
    }
    if pending_rows >= MAX_QUEUED_EVENTS || bytes > MAX_QUEUED_BYTES.saturating_sub(pending_bytes) {
        return Err(protocol("child-event-capacity-exceeded"));
    }
    let mut lookup = Lookup {
        roots,
        deadline: Instant::now() + REQUEST_TIMEOUT,
        requested: thread.into(),
        activity: None,
        chain: Vec::new(),
        queued: VecDeque::new(),
        queued_bytes: 0,
    };
    for (root, _) in &lookup.roots {
        state.turns.get_mut(root).unwrap().children.resolving = true;
    }
    if method == "thread/started" {
        Box::pin(accept_metadata(lookup, params.clone(), sender, state)).await?;
    } else {
        lookup.queued_bytes = serde_json::to_vec(message)
            .map_err(|_| protocol("child-event-encoding"))?
            .len();
        if lookup.queued_bytes > MAX_QUEUED_BYTES {
            return Err(protocol("child-event-capacity-exceeded"));
        }
        lookup.queued.push_back(message.clone());
        request_metadata(lookup, sender, state).await?;
    }
    Ok(true)
}

fn parse_metadata(requested: &str, result: &Value) -> Result<Identity, ProviderError> {
    let thread = result
        .get("thread")
        .ok_or_else(|| protocol("child-metadata-thread"))?;
    let id = required_string(thread, &["id"], "child-metadata-id")?;
    let parent = required_string(thread, &["parentThreadId"], "child-metadata-parent")?;
    let source = thread
        .pointer("/source/subAgent/thread_spawn")
        .ok_or_else(|| protocol("child-metadata-source"))?;
    let source_parent = required_string(
        source,
        &["parent_thread_id"],
        "child-metadata-source-parent",
    )?;
    let depth = source
        .get("depth")
        .and_then(Value::as_u64)
        .filter(|depth| *depth > 0 && *depth <= MAX_DEPTH)
        .ok_or_else(|| protocol("child-metadata-depth"))?;
    validate_id(&id)?;
    validate_id(&parent)?;
    if id != requested || parent != source_parent {
        return Err(protocol("child-metadata-identity-conflict"));
    }
    let path = match source.get("agent_path") {
        None | Some(Value::Null) => None,
        Some(Value::String(path)) => {
            validate_path(path)?;
            Some(path.clone())
        }
        _ => return Err(protocol("child-metadata-path")),
    };
    Ok(Identity {
        id,
        parent,
        path,
        depth,
    })
}

fn validate_id(id: &str) -> Result<(), ProviderError> {
    if id.is_empty() || id.len() > 256 {
        Err(protocol("child-identity-bound"))
    } else {
        Ok(())
    }
}

fn validate_path(path: &str) -> Result<(), ProviderError> {
    if path.is_empty() || path.len() > 1024 {
        Err(protocol("child-path-bound"))
    } else {
        Ok(())
    }
}

/// Cancellation drops correlation payloads, but received terminals still own
/// cleanup state and buffered server requests still require a response.
pub(super) async fn discard_cancelled_queue(
    root: &str,
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let Some(turn) = state.turns.get_mut(root).filter(|turn| turn.cancelled) else {
        return Ok(());
    };
    let queued = std::mem::take(&mut turn.children.queued);
    turn.children.queued_bytes = 0;
    turn.children.resolving = false;
    for message in queued {
        if message.get("id").is_some()
            || super::is_known_server_request(
                message
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        {
            let Some(id) = message.get("id").and_then(super::parse_rpc_id) else {
                super::send_invalid_request(sender, state).await?;
                continue;
            };
            // Registered requests are rejected by their existing owner. A request
            // already answered (including another buffered copy) needs no second write.
            if !state.server_request_tombstones.contains(&id)
                && !state.server_requests.contains_key(&id.external())
            {
                required_write(
                    sender,
                    &state.process_shutdown,
                    json!({"id":id,"error":{"code":-32003,"message":"Owning root was cancelled"}}),
                )
                .await?;
                super::remember_server_request_tombstone(state, id);
            }
        } else if message.get("method").and_then(Value::as_str) == Some("turn/completed")
            && let Some(params) = message.get("params")
            && let Some(thread) = params.get("threadId").and_then(Value::as_str)
            && thread != root
            && owner(state, thread)?.as_deref() == Some(root)
        {
            super::handle_child_notification(root, thread, "turn/completed", params, sender, state)
                .await?;
        }
    }
    Ok(())
}

/// A cancelled or replaced root must not retain lookups or their event payloads.
pub(super) async fn reap_lookups(
    sender: &JsonLineSender,
    state: &mut DispatcherState,
) -> Result<(), ProviderError> {
    let cancelled: Vec<_> = state
        .turns
        .iter()
        .filter(|(_, turn)| turn.cancelled)
        .map(|(root, _)| root.clone())
        .collect();
    for root in cancelled {
        discard_cancelled_queue(&root, sender, state).await?;
    }
    let mut expired = false;
    state.pending.retain(|_, pending| {
        let PendingResponse::ChildMetadata(lookup) = pending else {
            return true;
        };
        let active = lookup.roots.iter().any(|(root, registration)| {
            state
                .turns
                .get(root)
                .is_some_and(|turn| turn.registration_id == *registration && !turn.cancelled)
        });
        expired |= active && lookup.deadline <= Instant::now();
        active
    });
    if expired {
        Err(protocol("child-metadata-timeout"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::NativeSubAgentActivityKind;

    #[test]
    fn queued_lineage_work_is_bounded_by_rows_and_bytes() {
        let mut children = Children::default();
        for _ in 0..MAX_QUEUED_EVENTS {
            children.queue(json!({})).unwrap();
        }
        assert!(children.queue(json!({})).is_err());
        let mut children = Children::default();
        assert!(
            children
                .queue(json!({"data":"x".repeat(MAX_QUEUED_BYTES)}))
                .is_err()
        );
        assert!(children.queued.is_empty());
    }

    #[test]
    fn repeated_activity_identity_is_idempotent_but_changed_content_is_rejected() {
        let mut children = Children::default();
        children
            .insert(Identity {
                id: "child".into(),
                parent: "root".into(),
                path: Some("path/child".into()),
                depth: 1,
            })
            .unwrap();
        let event = ProviderEvent::SubAgentActivity {
            native_item_id: "activity".into(),
            agent_thread_id: "child".into(),
            agent_path: "path/child".into(),
            activity: NativeSubAgentActivityKind::Started,
        };
        assert!(children.observe_activity(&event).unwrap());
        assert!(!children.observe_activity(&event).unwrap());
        let mut changed = event.clone();
        if let ProviderEvent::SubAgentActivity { activity, .. } = &mut changed {
            *activity = NativeSubAgentActivityKind::Completed;
        }
        assert!(children.observe_activity(&changed).is_err());
        let mut changed = event;
        if let ProviderEvent::SubAgentActivity { agent_path, .. } = &mut changed {
            *agent_path = "changed/path".into();
        }
        assert!(children.observe_activity(&changed).is_err());
    }
}
