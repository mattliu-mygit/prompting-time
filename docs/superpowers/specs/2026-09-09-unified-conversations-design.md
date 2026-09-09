# Unified conversations and nested navigation

Status: proposed implementation scope; direction approved, written review pending.

## Intent

One durable Conversation represents every chat shown in the app. A conversation
may have a parent and may have children, including children that orchestrate further
descendants. Root, child, and orchestrator are relationships or observed behavior,
not separate conversation classes or separate chat implementations.

The sidebar renders these relationships visibly. Selecting any node opens that
conversation's own recorded activity in the same chat surface. This replaces the
current split between selecting a conversation and merely inspecting an agent.

## Identity and execution boundaries

Conversation identity and its optional parent relationship are authoritative for
product navigation. Children are obtained from that relationship, not a second
independently maintained tree. Cycles, missing parents, and conflicting parentage
are rejected. Verified provider ancestry is required before publishing a child;
unresolved ownership is never presented as root activity.

Provider runs and observed agent executions remain internal records: they describe
execution attempts, native correlation, permissions, and lifecycle, not another
user-facing conversation type. Each execution maps to exactly one Conversation.
Root executions map to their existing conversation; observed child executions map
to durable child conversations beneath their verified parent.

For this slice, observed child identity retains the existing provider-run scope.
Repeated observations of one recorded child reuse its conversation; labels or raw
native IDs alone never merge children across unrelated runs. A child conversation
remains discoverable after its run finishes and after the parent starts a later
turn. Cross-run native-session identity consolidation is not part of this change.
Consequently, the same raw native child ID observed in a later parent run creates
a separate child Conversation rather than guessing continuity. This limitation
must remain explicit until provider-specific cross-run identity is established.
Each node's own status comes from its associated execution, not the parent's latest
run. Historical children do not overwrite the current parent run's status.

The same conversation summary, selected-conversation state, timeline, and composer
components serve every node. Remove the parallel product-facing agent-selection
and agent-chat state after migration; do not leave a second renderer behind a type
alias. Internal execution metadata may still supply attribution and approvals.

## History, controls, and workspace

Activity reads resolve the selected Conversation to its recorded execution owners
before applying indexed, bounded pagination. They must not filter only the newest
parent page in the browser, which could hide all older child activity. Existing
event identity, order, operation revisions, and lazy bounded details are preserved.
No event bodies are copied into a second transcript store. Parent navigation keeps
descendant status and attention visible without treating child text as parent text.

Only captured messages and activity are displayed. Missing provider child text is
explained explicitly; a lifecycle-only child is not represented as a complete chat.
No historical transcript import, hidden reasoning capture, or fabricated messages.

Actions depend on the execution binding and verified provider capabilities, not
whether the Conversation happens to have a parent. Existing independently managed
conversations retain their current controls. Provider-observed children initially
offer recorded activity and existing supported approval handling; unsupported send,
resume, routing, interruption, or archive operations are unavailable with a clear
explanation and are also rejected by the service. This does not add new native
child control protocols or independently dispatch a provider-observed child.
Native task or child IDs are not inserted into independently resumable provider
session records merely because they now have a Conversation in the product model.

Selecting a child never silently sends a message to its parent. The shared composer
targets the selected Conversation or explains that messaging is unavailable.
Drafts, reading anchors, previews, and detail disclosures are scoped to conversation
identity; parent drafts survive visiting a child and returning. An explicit parent
navigation action changes the target back.

Observed children reuse the owning execution's workspace context. Creating a child
conversation does not allocate a new worktree, transfer cleanup ownership, or grant
broader permissions. Resolve inherited context through its actual workspace owner;
do not copy the owner's exclusive workspace association into a child. Existing
dispatch, approval, mutation, and shutdown fences
remain tied to the original execution owner even when viewed from a child.

## Sidebar behavior

The top-level list includes root conversations only; children appear beneath their
parents, never a second time as independent roots. Existing root archive behavior
hides the subtree from navigation without deleting children or transferring ownership.

The selected conversation reveals its first child level when children first become
available, unless the user explicitly collapsed it. Each expandable node has a
chevron; deeper branches expand independently. Explicit navigation to a descendant
reveals its ancestors. Disclosure choices survive conversation switches and sidebar
hide/show within the session; they are not a durable bookmark guarantee.

Each generation has visible indentation and a parent guide. Indentation is bounded
in deep trees so names and 32-pixel controls remain usable; the selected ancestry
path still exposes exact semantic depth. Rows reuse the existing compact title,
provider, and status presentation. Keyboard navigation follows visible tree order.

Children load through bounded, paged reads on disclosure. Closed branches do not
hydrate entire execution histories. Refresh updates an already-open branch when
new children arrive; it must not require collapsing and reopening to load them.
Completed children remain available. Failed reads preserve the visible state and
offer retry; evicted windows clearly offer a way to load more history.
Live execution updates refresh the affected conversation and ancestor summaries;
viewing a child must not lose updates merely because its provider runs under a
parent-owned execution scope.

## Migration and scope

Keep existing root conversation IDs and all historical run, event, approval, and
workspace identities. Backfill child conversations from recorded execution ancestry
and add durable bindings transactionally. Preserve original execution ownership
rather than rewriting permission or run relationships just to fit the UI. Reopening
an upgraded database or replaying observations must not duplicate children.

A shared UI wrapper over two separate chat models would leave the identity/history
problem intact. Conversely, collapsing provider execution attempts into Conversation
would erase necessary runtime boundaries. The selected design unifies conversations
while retaining those internal execution records.

This is one focused model-and-navigation change. It does not add manual child
creation, cross-provider child handoff, reparenting, conversation deletion, a new
database, or a new provider dependency. Preserve the existing running application
and local data during development; restart, live-provider tests, and publication
are separate actions requiring authorization.

## Acceptance and completion

- Root, child, and grandchild use one public conversation model and one chat path.
- Recursive indentation is checked in a real browser, not only through aria-level.
- Each selection shows only correctly attributed captured history, including older
  pages; missing transcripts remain explicit and parent drafts remain unchanged.
- Child hierarchy survives completion, subsequent parent turns, and database reopen.
- Duplicate observations, conflicting ancestry, paging, stale refreshes, and absent
  child detail are covered with focused regression tests.
- Unsupported controls cause no provider dispatch, parent submission, or workspace
  mutation; approval and runtime ownership tests remain valid.
- Browser checks cover narrow/wide windows, keyboard selection, disclosure state,
  deep names, loading/retry, and bounded overflow using invented data.
- Consolidate this accepted behavior into the canonical product specification when
  implemented; retire this temporary change specification and superseded UI paths.
