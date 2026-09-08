//! Metadata-only ancestry correlation for activities carried by an owned root turn.
//! This does not forward descendant-owned turn, transcript, or control events.

use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};
use tokio::time::Instant;

use super::{
    DispatcherState, JsonLineSender, MAX_PENDING_REQUESTS, PendingResponse, ProviderError,
    ProviderEvent, REQUEST_TIMEOUT, RpcId, deliver_to_turn, handle_server_message, protocol,
    required_string, required_write,
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
}

impl Children {
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
    pub root: String,
    pub registration: u64,
    pub deadline: Instant,
    requested: String,
    activity_id: String,
    activity_path: String,
    chain: Vec<Identity>,
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
            && state
                .turns
                .get(&lookup.root)
                .is_some_and(|turn| turn.registration_id == lookup.registration && !turn.cancelled)
        {
            if owner.as_ref().is_some_and(|root| root != &lookup.root) {
                return Err(protocol("child-root-conflict"));
            }
            owner = Some(lookup.root.clone());
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
        root: root.to_owned(),
        registration: turn.registration_id,
        deadline: Instant::now() + REQUEST_TIMEOUT,
        requested: agent_thread_id.clone(),
        activity_id: native_item_id.clone(),
        activity_path: agent_path.clone(),
        chain: Vec::new(),
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
    let Some(turn) = state
        .turns
        .get(&lookup.root)
        .filter(|turn| turn.registration_id == lookup.registration && !turn.cancelled)
    else {
        return Ok(());
    };
    if lookup.deadline <= Instant::now() {
        return Err(protocol("child-metadata-timeout"));
    }
    let identity = parse_metadata(&lookup.requested, &result)?;
    if identity.id == lookup.root
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
        && identity
            .path
            .as_ref()
            .is_some_and(|path| path != &lookup.activity_path)
    {
        return Err(protocol("child-path-conflict"));
    }
    let parent_depth = turn.children.depth(&lookup.root, &identity.parent);
    lookup.requested = identity.parent.clone();
    lookup.chain.push(identity);
    let Some(mut depth) = parent_depth else {
        return request_metadata(lookup, sender, state).await;
    };
    // Validate the entire chain before publishing any identity into the run.
    for identity in lookup.chain.iter().rev() {
        depth += 1;
        if identity.depth != depth {
            return Err(protocol("child-depth-conflict"));
        }
    }
    if turn.children.identities.len() + lookup.chain.len() > MAX_AGENTS {
        return Err(protocol("child-identity-capacity-exceeded"));
    }
    for identity in lookup.chain.into_iter().rev() {
        let event = ProviderEvent::ChildAgentActivity {
            // The real activity caused this metadata observation; retain its ID.
            native_item_id: lookup.activity_id.clone(),
            parent_native_thread_id: identity.parent.clone(),
            child_native_thread_ids: vec![identity.id.clone()],
            child_statuses: Vec::new(),
            operation: "resolveIdentity".to_owned(),
            status: "observed".to_owned(),
        };
        let turn = state
            .turns
            .get_mut(&lookup.root)
            .expect("root registration checked above");
        turn.children.insert(identity)?;
        if turn.native_turn_id.is_none() {
            if turn.provisional_events.len() >= super::PROVISIONAL_EVENT_CAPACITY {
                return Err(protocol("provisional-event-capacity-exceeded"));
            }
            turn.provisional_events.push_back(Ok(event));
        } else {
            deliver_to_turn(state, &lookup.root, Ok(event), sender).await?;
        }
    }
    let turn = state
        .turns
        .get_mut(&lookup.root)
        .expect("root registration checked above");
    turn.children.resolving = false;
    let buffered = std::mem::take(&mut turn.children.queued);
    turn.children.queued_bytes = 0;
    for message in buffered {
        Box::pin(handle_server_message(message, sender, state)).await?;
    }
    Ok(())
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

/// A cancelled or replaced root must not retain lookups or their event payloads.
pub(super) fn reap_lookups(state: &mut DispatcherState) -> Result<(), ProviderError> {
    let mut expired = false;
    state.pending.retain(|_, pending| {
        let PendingResponse::ChildMetadata(lookup) = pending else {
            return true;
        };
        let active = state
            .turns
            .get(&lookup.root)
            .is_some_and(|turn| turn.registration_id == lookup.registration && !turn.cancelled);
        expired |= active && lookup.deadline <= Instant::now();
        active
    });
    for turn in state.turns.values_mut().filter(|turn| turn.cancelled) {
        turn.children.queued.clear();
        turn.children.queued_bytes = 0;
        turn.children.resolving = false;
    }
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
