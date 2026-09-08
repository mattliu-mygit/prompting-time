# Prompting Time Design

## Product intent

Prompting Time is a local macOS desktop application for managing coding-agent conversations across multiple installed CLI harnesses. The current release candidate integrates Codex App Server and Claude Code when their installed CLIs pass compatibility and authentication checks. It gives the user one place to create, monitor, steer where supported, interrupt, resume, and switch providers across concurrent conversations while each harness retains its native authentication and session persistence.

The application owns the user-facing conversation and execution hierarchy. Each provider continues to own its native session. This separation lets Prompting Time present consistent behavior without reducing Codex and Claude to a lowest-common-denominator API.

## Release boundaries

### Milestone 1: desktop multiplexer

Milestone 1 targets:

- A locally installable macOS application named Prompting Time.
- Codex and Claude Code integration through supported, authenticated local CLIs, with actionable diagnostics for unavailable providers.
- Project-backed and projectless conversations.
- Multiple conversations executing concurrently.
- A recursive execution tree in which any agent may create child agents and any child may itself orchestrate descendants.
- Automatic provider routing with a visible manual override.
- Provider switching between turns with explicit context handoff. Hermetic tests cover the shared machinery; separate opt-in application smokes verify live continuity through both adapters.
- In-app approvals, questions, interruption, recovery, background notifications for root completion, failure, and needs-attention transitions, and diagnostics.
- Isolated Git worktrees by default for project-backed conversations, with an explicit option to use the current checkout.
- A public repository containing application code and generic project material only.

### Milestone 2: private resource library

Milestone 2 imports Codex and Claude skills, memories, and the files they reference into a private, provider-neutral local library. It preserves original content and source metadata, deduplicates identical resources, records provider compatibility, and stages selected resources in each provider's native format.

Milestone 2 is a separate specification and implementation plan. Milestone 1 establishes only the adapter and storage boundaries needed to add it cleanly.

### Later work

Additional providers, remote execution, learned routing, scheduled tasks, mobile control, cross-device synchronization, existing-session import, and signed/notarized public releases are outside Milestone 1. Public signing and notarization require Apple Developer credentials and a separate release decision.

## User experience

### Application layout

The main window uses a three-pane command-center layout:

- The left pane lists conversations by project and status. Expanding a conversation reveals its recursive agent tree. Each node shows its provider and current status.
- The center pane shows the selected conversation timeline, provider-labeled messages, tool activity, child-agent cards, approvals, and the composer.
- The right inspector shows routing rationale, workspace and worktree state, changed files, and active agents. It is collapsible for focus and smaller windows.

The shell is constrained to the window height. Long timelines, conversation lists, and inspector
content scroll within their panes instead of pushing the composer below the window. Selecting a
child preserves usable navigation and the conversation workspace. An active interruptible turn
has its own Interrupt action even when steering is available or no alternative provider is
available; confirmation remains bound to the original run.

The UI must not invent child-agent identity. If a provider exposes a child agent, Prompting Time creates an agent node. If it exposes only an anonymous tool operation, the UI displays a tool event under the current agent.

The center timeline keeps the newest 80 events plus at most four explicitly loaded 80-event history pages. Live invalidations are coalesced behind one read per selected conversation; the latest dirty state receives one follow-up read without overlapping an older-page request. Live refresh replaces the newest window by durable event identity while retaining loaded history; any eviction is visible and offers a way back to the newest window. Every truncated event kind remains visibly truncated and fetches its separately bounded detail only after explicit disclosure. Agent activity is collapsed by default and independently requests the selected run's bounded agent pages when opened; it pages the expanded depth-first view 20 cards at a time so a wide or deep provider tree cannot crowd out approvals or the composer. Agent restarts are single-flight with one latest coalesced restart. Pending approvals load 30 summaries initially, page independently on explicit request, and never fetch operation or question detail merely because a card is mounted. The approval view retains at most four pages: loading farther back evicts the newest retained page but preserves the returned cursor until every pending request is reachable, and an explicit control restores the newest page. Live invalidation rebases that bounded window onto the newest approvals by following the refreshed cursor chain, so insertion-driven page shifts cannot skip requests and terminal or missing requests disappear without unbounded fan-out. Explicit approval paging owns an independent busy token so overlapping refresh success or failure cannot strand its retry controls.

### Conversation creation

A fresh installation offers a compact new-conversation chooser with **Choose folder**,
**Without a folder**, and collapsed **Advanced** options. Choose folder opens the native
macOS single-directory picker. Selection automatically validates and classifies the
directory, prepares the workspace, creates and selects an empty conversation, and
focuses the composer without another form or manual inspection action. Picker cancellation
creates nothing and returns to the chooser. Creation does not start a provider or submit
a prompt.

The folder name supplies the initial title, with **New conversation** as the fallback
and projectless title. No title or objective is required. The initial objective is empty;
ordinary message history carries the first user request without inferred instructions,
an extra naming model call, or duplicated metadata. Advanced allows routing and Git
execution overrides before selection. Git folders default to the existing isolated
worktree behavior; non-Git folders use the selected directory directly. Isolation does
not copy uncommitted edits. The user may explicitly choose the current Git checkout,
but failed isolation never silently falls back to it.

Only one picker/check/create operation is admitted at a time. Closing or unmounting the
chooser invalidates pending pre-creation results; committed creation cannot be cancelled
ambiguously. Busy controls prevent duplicates, errors permit another selection or retry,
and existing preparation rollback remains authoritative. Keyboard focus remains inside
the chooser until cancellation or successful creation.

Archiving requires deliberate confirmation, retains durable history, removes the
conversation from active navigation, and selects another active conversation or clears
the workspace. The same reconciliation occurs when an external change reports that the
selected conversation was archived.

Before the first message, the user may choose automatic routing or pin Codex or Claude. Automatic routing remains the default. The selected provider and reason are visible on every provider run.

Startup diagnostics remain visible with their recovery instruction and Retry even when
conversation loading fails. Bootstrap and ordinary conversation reads remain concurrent; stale
or disposed refreshes cannot replace current startup state.

The inspector keeps a bounded, cursor-paged audit of provider runs. Each marker exposes the canonical provider and routing reason. Selecting a marker performs a conversation-scoped detail read for that app-owned run identifier and reveals the exact bounded handoff sent for that run; historical handoffs are never fetched merely by opening the inspector. Git/worktree inspection is globally single-flight across conversation selection and component lifetimes, with at most one coalesced follow-up for the latest selection, and is not repeated for each streaming delta. Streaming changes mark the inspector stale, and the user explicitly refreshes the potentially expensive workspace snapshot.

### Concurrent work

Conversations execute independently. The runtime executes at most four root runs and retains at most 64 additional root runs in its FIFO admission queue. A 69th root submission is rejected before any run is persisted; completing or interrupting an admitted run releases one slot. The active-run limit may become user-configurable without weakening the fixed admission bound. A single conversation serializes normal turns, while child agents may run concurrently when the provider supports them.

Approval responses and steering share bounded application-wide control admission: at most four operations, with separate single response and steering slots per active attempt. Both may coexist when the provider supports them. Steering and answer payloads are bounded to 64 KiB of UTF-8 text. Excess or duplicate work is rejected before another task or payload is retained. Interrupt and shutdown control paths cannot be starved by ordinary work admission. An admitted operation has an owned execution lifetime independent of its caller; pending work can be cancelled before dispatch, and executing work must settle its outcome before terminal publication allows the next user turn.

The UI submits question answers only from complete canonical question data supplied by a bounded question page or complete approval detail. A complete question whose `options` are null is a canonical free-text question; `isOther` adds free text alongside enumerated choices. Missing or truncated question data blocks the response with an actionable explanation. Approval detail also supplies a bounded canonical requesting-agent label path, independently of which agent-tree pages the user has disclosed; paths beyond the bound are explicitly marked truncated.

The sidebar distinguishes queued, running, waiting for approval or input, completed, interrupted, and failed work. Conversation summaries keep only their bounded root preview until the user expands one conversation. The sidebar then loads 20 agents per explicit page, retains at most four pages for that one conversation/run, preserves the selected agent's loaded ancestry, and exposes restart or retry controls when history is evicted or loading fails. An already disclosed window refreshes its first bounded page when run state changes so displayed status does not become stale; unopened conversations remain lazy. macOS notifications fire only while the main window is unfocused and only when a root conversation becomes completed, failed, or needs attention. Repeated observations of the same state are deduplicated for every active conversation observed during the process lifetime. Startup primes that state without notifying. If the compact store-change channel lags, notification state is rebuilt from active conversations in bounded 200-row pages so a dropped terminal or needs-attention change is still observed; after a complete scan, entries absent from the active set are pruned. The notification contains only the fixed app title and a fixed status label; conversation titles, prompt text, output, tool details, paths, file names, and provider-native identity are excluded. Permission is requested lazily on the first eligible notification.

### Recursive agent hierarchy

An agent node may emit messages and tool events, request approval or input, and create child nodes. Descendant state rolls up to ancestors: a root conversation remains active while any descendant is running and shows that attention is required when any descendant is waiting on the user.

The hierarchy is provider-neutral in the UI but retains provider-native identifiers and event payloads for diagnosis. Arbitrary depth is supported by the data model; the UI progressively collapses deeper levels rather than imposing a semantic depth limit.

## Architecture

Prompting Time uses Tauri 2 for the macOS application shell, React and TypeScript for presentation, and Rust for all orchestration, persistence, routing, process supervision, and provider integration. The webview never starts or communicates with provider processes directly.

The Rust core is organized around these responsibilities:

- Conversation service: owns canonical conversations, messages, and timeline events.
- Run supervisor: starts provider processes, consumes streams, applies backpressure, tracks liveness, and coordinates cancellation and recovery.
- Router: selects an eligible provider and records an explainable decision.
- Approval broker: normalizes provider approval and question requests without weakening provider policy.
- Workspace manager: creates, validates, inventories, and safely cleans up worktrees.
- Notification service: emits macOS notifications for actionable background transitions.
- Resource boundary: provides the extension point for Milestone 2 without importing resources in Milestone 1.

Tauri commands expose typed application operations. Tauri events publish typed state changes. React owns transient presentation state only; orchestration policy and durable truth remain in Rust.

### Provider adapter boundary

The provider boundary defines one capability-aware adapter contract covering discovery and health, session start and resume, message submission, active-turn steering when supported, interruption, approval or user-input responses, normalized event streaming, and capability reporting. Both Codex and Claude implement that contract. Desktop registration reuses each adapter's eligibility checks so displayed availability agrees with routing eligibility.

Capabilities are explicit rather than assumed. The UI disables or explains unsupported operations. Provider-specific options and native payloads remain isolated within the corresponding adapter.

The Codex adapter uses the newline-delimited JSON-RPC protocol exposed by `codex app-server`, with current live evidence for CLI 0.153.4. It owns the shared process and separates client requests from provider-originated requests. Typed request identity, bounded correlation, registration generations, and native thread/turn identity prevent stale output or responses from affecting a replacement run. Provisional starts buffer only bounded events until the native start response confirms identity. Malformed ownership, conflicting identities, duplicate requests, and unsupported contracts fail closed.

Codex child identity comes from verified native metadata, not display paths or an assumption that the only open root owns an event. Bounded metadata-only discovery resolves missing ancestors before publishing children, including nested starts received before collaboration activity. Activity observations are deduplicated and do not replace authoritative native turn lifecycle once available. A child can run another native turn while keeping its canonical identity; ended turn IDs cannot be reused. Exact native child ownership follows each permission or question through admission, response, acknowledgement, and termination. Root, siblings, and independent conversations cannot share request or file-item details accidentally.

A child terminal observed by the dispatcher before a response write seals that response target. If the write precedes terminal receipt, the matching terminal waits for durable response reconciliation; unrelated children continue. This establishes transport ordering, not proof that the provider acted on the answer. Cancellation rejects outstanding controls before interrupting known owned turns. Cleanup requires terminal evidence rather than treating an interrupt acknowledgement as completion; missing evidence fails within bounded shutdown. Native root completion with active descendants remains a conservative failure-and-cleanup case, not an unbounded background drain.

Codex permission and question payloads are validated before UI admission. Invalid, stale, or cancelled requests are explicitly rejected; bounded response-write failure tears down the connection. Approval never broadens the requested permission profile. Unknown notifications retain only a safe method diagnostic, not arbitrary payloads. Child lifecycle and controls are supported, but child transcripts and arbitrary child output are not forwarded into root conversation history. Version-specific protocol fields and test evidence belong in [provider protocol evidence](../../provider-protocols.md).

The first provisional terminal seals its buffer, so later activity is ignored and later requests are rejected rather than replayed. Each turn registration has a unique generation shared by its start request, owner, and cleanup guard. Request state distinguishes queued, writing, awaiting response, caller-consumed, and caller-abandoned work; only the caller consuming its response marks a request finished. Dropping a response-ready `turn/start` therefore still cleans up or interrupts its exact generation. If both caller and event receivers are already closed when a successful start response activates the turn, that exact cancelling generation remains registered until native interrupt confirmation or process teardown; no replacement can enter while its fatal interrupt is pending. Explicit interruption validates the active generation and turn ID before changing ownership or writing to Codex and enters cancelling only after its request is admitted and written; capacity rejection leaves the turn active and retryable. Terminal completion can internally confirm and tombstone an admitted interrupt before its late response, but it resolves primary and coalesced callers and releases the turn only after every outstanding server request has been rejected through writer-acknowledged responses. Cleanup failure instead fails the interrupt callers and tears down the connection. Dropping the interrupt caller after write does not abandon the native confirmation. If natural terminal completion wins a race with an already queued owner shutdown, an exact `NotDispatched` cancellation is reconciled against the shared completed flag and treated as successful without tearing down the healthy shared process. Stale cleanup cannot remove or interrupt a replacement turn. Cancelling a queued, undispatched `turn/start` unregisters only that exact provisional generation, and pending-capacity rejection does the same while reporting that nothing was dispatched. An existing registration is never replaced merely because its consumer closed. Each bounded turn stream reserves one slot for terminal evidence, and overflow interruption is dispatched outside the shared reader so a slow consumer cannot hide completion or stall other sessions.

Codex approval details are typed and durable across restart. Command approvals expose the requested command and working directory. File-change approvals require a bounded, same-generation item correlation and expose the grant root, reason, and affected paths with change kinds and move targets, but never retain patch bodies; unseen item IDs fail closed. Permission approvals expose the exact typed filesystem/network profile and working directory Codex requested.

Provider protocol versions are detected at startup. Missing, unauthenticated, unhealthy, or unsupported CLIs remain visible with actionable diagnostics but are excluded from automatic routing. Provider initialization has an adapter-owned deadline: timeout and error paths stop, kill when necessary, and await the provisional process owner before returning.

Claude uses one owned CLI process per active turn and resumes the native conversation across later turns and app restarts. Its health check inspects version and authentication under bounded deadlines without retaining account data. Compatibility currently requires major version 2 at or above 2.1.205; live evidence is specific to 2.1.205, with fail-closed protocol validation on every allowed version. The CLI chooses its default model. Default permissions travel through stdio, with empty settings sources and an empty strict MCP configuration. Imported hooks and private resources remain Milestone 2 work.

Claude exposes streaming, permission and single-select question responses, interruption, resume, and recursive child hierarchy. It does not expose steering; unsupported multi-select questions are safely declined. Native task identity establishes parentage and terminal status without promising independently resumable child sessions or complete grandchild text. Root completion requires observed descendant termination. The known-failing legacy hook-defer/restart probe is not the runtime approval transport.

Initialization before the first prompt does not establish a resumable Claude transcript. In-process cancellation preserves the fresh-session state, but app restart in that narrow window can require a new conversation; a missing transcript fails closed without replay. Completed and interrupted prompt sessions have resumed successfully in focused live probes. Version-specific protocol shapes and test evidence belong in [provider protocol evidence](../../provider-protocols.md).

## Authoritative data model

Prompting Time stores one canonical representation of:

- Conversation: a normalized nonblank title of at most 256 UTF-8 bytes, optional project/workspace, routing mode, and lifecycle metadata. Legacy titles are defensively bounded when read, and whitespace-only legacy values display as `Untitled conversation`.
- Message: ordered user or assistant content displayed in the shared timeline.
- Provider run: one provider's execution of a turn, including routing decision, native session identity, status, and context boundary.
- Agent node: a self-referential execution node with an optional parent, provider identity, and rolled-up status.
- Event: ordered message, tool, progress, diagnostic, and lifecycle activity attached to a run and agent node.
- Approval: a durable pending or resolved provider request with the exact decision supplied by the user.
- Routing decision: eligible providers, chosen provider, profile, reason, and override state.
- Workspace: project identity, execution directory, worktree ownership, Git state, and cleanup eligibility.

Provider-native identifiers and narrowly selected protocol fields are retained alongside normalized records where needed for resumption and diagnosis. Raw payload retention is opt-in at each adapter boundary and must exclude hidden reasoning, authentication material, and unrecognized payload content. Provider-native data never replaces the canonical model.

SQLite is the local source of truth. It uses WAL mode, versioned migrations, foreign-key enforcement, bounded queries, and indexes that support status/project filtering and timeline pagination. Ordinary sidebar synchronization filters archived conversations at this query boundary; archived history remains available only through an explicit future archive surface. Durable state transitions are transactional. User and assistant messages occupy the same role-bearing event sequence; the separate message projection exists only for provider handoff context. Upgrades reconstruct historical user events from that projection, replace transitional user-event copies, and deterministically place a user turn before provider events when old independent sequences share a timestamp. Desktop timeline pages expose UTF-8-safe bounded previews and fetch a separately bounded event detail by the app-owned event ID. Agent trees, provider-run audits, and pending or historical approvals use stable independent cursors, so a sidebar cutoff never makes descendants, historical routing, or actionable requests inaccessible. Provider-run audit summaries expose only app-owned run identity, canonical provider and routing reason, and explicit truncation state; a conversation-scoped detail read returns the bounded routing evaluation and UTF-8-safe bounded handoff without exposing native session identity. Approval rows carry their canonical conversation ownership and use conversation-first partial indexes for pending and historical pages. Approval question previews are normalized once in the approval transaction and paged through an indexed app-owned ordinal; ordinary snapshots contain only app-owned IDs and bounded canonical fields, while provider-native request and question identities remain internal and are mapped at provider dispatch. Application startup pages queued roots and processes ambiguous unfinished agents in indexed, deepest-first batches under a fixed deadline rather than hydrating every conversation or active graph.

Provider output received while an approval is pending is durably staged in receipt order without changing the Waiting lifecycle. Assistant deltas with the same native item ID aggregate into one durable message before, during, and after staging, including after restart, so a streamed message never becomes a row per token. Each run's staged queue accepts up to 256 complete provider events, with one additional physical row reserved for an overflow marker; the full queue, including that marker reserve, is limited to 8 MiB of content. Events within that capacity retain their full content. The first event that would exceed either limit is replaced by one compact diagnostic marker that records the omitted event kind and makes mutation certainty Unknown; later staged ingress is rejected. Recovery returns at most the 257-row physical limit together with explicit overflow and truncation flags. Once an approval response is accepted by the supervisor, the supervisor owns its complete intent, provider-dispatch, acknowledgement, publication, and cleanup lifecycle independently of the requesting UI future; interruption and shutdown cancel and join that operation. Approval acknowledgement atomically publishes the resumed lifecycle, any privacy-filtered accepted-answer message, and each staged event once, then clears the queue; interruption, failure, or crash performs the same bounded publish before the terminal diagnostic, so restart recovery retains bounded evidence of already-observed activity.

Runtime data is stored under the user's macOS Application Support directory. Conversations, prompts, tool output, provider payloads, machine paths, imported resources, and local configuration never enter the public repository.

Confirmed steering and acknowledged non-secret question answers enter the same authoritative
user-message/event projection used by timeline and handoff. Persistence is owner-fenced and
settles before terminal publication permits a new turn. Definitive rejection or uncertain
dispatch never becomes an accepted message. Process loss without durable confirmation remains
unconfirmed and is never replayed; this change adds no historical backfill.

Question context preserves provider and agent provenance and pairs answers using the stored
native question identities. Secret fields are filtered before bounded formatting, with explicit
truncation when the projection cannot fit. Exact approval response data remains authoritative in
the private approval record. Child answers advance the active provider's native session-tree
context boundary, not a claim of direct delivery to its root thread. Other providers can receive
that canonical context on handoff; no unsolicited steering delivers it back to the native root.

Only the active provider session's consumed-message boundary advances for accepted input; a
run's original dispatch context cut is immutable. Native assistant-item aggregation remains
unchanged, so a later chunk can update an older message without lowering that boundary. Handoffs
retain bounded recent whole messages, not a guarantee that all old input remains in every prompt.

## Routing

Automatic routing is deterministic and explainable in Milestone 1. Selection uses this precedence:

1. An explicit per-message or conversation provider override.
2. Provider eligibility: installed, supported, authenticated, healthy, and not known to be quota-blocked.
3. Continuity with the provider already handling the current line of work.
4. Task signals available from the message and conversation type.
5. Usage balancing among otherwise suitable providers.

New conversations default to Best fit. Advanced options also expose Balanced and Usage balance.
Existing conversations retain their saved profile. Profiles adjust the relative weight of task
suitability and distribution; they do not bypass eligibility or manual overrides.

Every run records and displays the selected provider and a concise reason. Prompt classification runs locally in Rust and does not add a separate model call. The application records manual overrides and outcomes locally so a learned router can be evaluated in later work instead of being guessed into Milestone 1.

## Provider switching and context handoff

Switching providers occurs only at a turn boundary. Steering an active turn stays with its current provider. To switch during active work, the user first interrupts the run.

Interrupt and provider-switch confirmation is a portaled modal surface. While it is open, the complete application background is inert to pointer, keyboard, and assistive-technology navigation; interruption failures remain inside the active dialog, and closing it restores normal interaction and focus.

The canonical conversation remains continuous, but each provider has its own native session. On a provider's first turn in a conversation, Prompting Time creates a native session and supplies a handoff capsule. When returning to an existing provider session, Prompting Time resumes it and supplies only canonical activity that session has not seen.

A handoff capsule contains:

- The current objective and current user request.
- Durable decisions and constraints.
- Recent user and assistant messages verbatim within a bounded budget.
- Relevant child-agent outcomes.
- Changed-file and workspace state for project-backed work.
- A clear boundary identifying imported context rather than native provider history.

Small conversations may transfer their complete visible transcript. Larger conversations use the durable capsule plus recent messages. The exact context supplied to each provider run is recorded. Hidden reasoning is neither requested nor fabricated. Detailed tool logs are summarized unless they are required to understand an unresolved failure. For project-backed work, current filesystem and Git state remain authoritative.

## Approvals and safety

Prompting Time preserves each provider's permission model. It normalizes the presentation of approval and user-input requests but does not silently broaden access, bypass safeguards, or treat one provider's approval as authorization for another.

Each active attempt exposes one canonical pending permission or input request at a time. Additional requests wait in an attempt-local FIFO bounded to 16 requests and 256 KiB of serialized event payload, excluding the currently visible request. The next request becomes visible only after the preceding response is durably acknowledged and its pending state clears; a retryable response rejection leaves the current request pending. Queued requests with empty identities, identities matching a current or queued request, or payloads exceeding queue limits fail the attempt safely. Ordinary provider progress continues through the separate durable staging path and retains its order and mutation evidence; staged progress may become visible before a later queued permission. Observed stream closure, interruption, terminal result, attempt failure, or ownership loss prevents further queue publication and invokes owned turn cleanup. Queued requests are not replayed after restart, which interrupts ambiguous active attempts.

Approval decisions are scoped to the provider operation and persistence scope offered to the user. Destructive actions remain explicit. The application does not copy provider credentials; Codex and Claude continue using their existing authentication stores.

Diagnostic logs exclude prompt text, assistant content, tool output, file contents, credentials, and imported resources by default. Logs may contain timestamps, opaque local identifiers, provider/version information, state transitions, and sanitized error categories. Telemetry is disabled by default.

## Failure handling and recovery

The run supervisor records incoming events incrementally and treats provider streams as fallible. A process exit, malformed event, protocol incompatibility, or application restart cannot silently turn a partial run into a completed run.

Automatic fallback to the alternate provider occurs at most once and only when the first provider fails before any mutating tool activity. If mutation may have occurred, Prompting Time preserves the partial run, inspects current workspace state, and asks the user whether to continue, switch, or stop. It never replays a possibly mutating turn automatically.

Each logical message submission has one command UUID. A retry reuses it only when the bridge explicitly reports `outcome-unknown`, meaning transport failed without establishing whether the command was accepted. Structured semantic failures, including generic internal failures, end that logical attempt so the next submission receives a new UUID.

Before any provider session or turn call, the supervisor atomically claims the queued run in SQLite, records that dispatch may have occurred, and attaches a renewable lease owned by a unique supervisor instance. New-message persistence and the first claim share one transaction. Provider start, resume, turn, steering, and approval-response calls each require a successful durable ownership fence immediately before dispatch. A provider call never precedes its durable claim. Once claimed, every supervisor-owned durable mutation—including provider events, native-session binding, context advancement, staged waiting output, approval response state, fallback, interruption, failure reconciliation, and terminalization—checks the expected owner inside the same write transaction. A former owner that wakes after a transfer cannot mutate the run. The lease lasts 120 seconds and is renewed every 15 seconds for executing and capacity-queued work. Stale recovery waits an additional five-minute margin, preserving a live owner across delayed scheduling or ordinary macOS sleep.

Concurrent reconcilers may admit or mutate only the single successful CAS winner. Each attempt first acquires a unique temporary recovery owner before reserving supervisor capacity, enqueueing, or interrupting; a loser leaves the row unchanged. Safe queued re-entry promotes that temporary owner to the stable supervisor owner in a second CAS immediately before enqueue. Startup leaves every claimed run with its current owner even when its wall-clock lease appears expired, giving a process waking from macOS sleep a full live scheduling window to renew. After two minutes, and every two minutes thereafter, bounded background recovery atomically transfers only a stale claim held by a different supervisor instance to a unique recovery-attempt owner before interrupting it; that transfer prevents a late former owner from racing the interruption or dispatching more provider work. A supervisor never reaps its own execution claim. If a recovery attempt times out, fails, or its process crashes, its transferred lease eventually becomes stale and a later recovery attempt, including one in the same healthy app process, can finish reconciliation. On application restart, Prompting Time re-enters only queued roots whose durable intent, mutation state, dispatch certainty, and absence of a retained native session prove they are unclaimed and undispatched. An abandoned claimed run is ambiguous and is never reopened; queued, running, and waiting ambiguity is marked interrupted child-first with an actionable diagnostic. A crash after claiming but before the provider call therefore fails closed rather than risking a duplicate side effect. Startup recovery is paged through the normal bounded supervisor admission limits under a ten-second deadline; stale follow-up runs outside that startup critical path. If startup recovery cannot establish coherent durable state in time, startup fails closed and performs owned shutdown.

On normal shutdown the app closes admission, cancels owned run and approval-response tasks, awaits graceful completion for five seconds, then requests forced adapter/process termination and awaits the remaining owners. No provider process or event-forwarding task is intentionally detached from its recorded owner.

Per-conversation serialization prevents interleaved user turns. Bounded channels and pagination prevent unbounded memory growth. Cancellation propagates from the root run to owned processes. On cancellation, successful owned shutdown makes unresolved descendants locally Interrupted, deepest first, before the root becomes terminal; provider failures or unsuccessful shutdown instead mark them Failed. These local outcomes carry Unknown mutation certainty and preserve already terminal descendants. Provider completion with active descendants is a contract failure. Every cleanup write remains fenced by durable run ownership, including children materialized while approval output is staged.

## Workspace and worktree lifecycle

For a project-backed Git conversation, Prompting Time creates and records an isolated worktree before provider execution. All providers and descendants in that conversation share the recorded execution directory unless a later explicit feature introduces child isolation.

Using the current checkout is an explicit opt-out of isolation. The UI warns when another active conversation already uses that checkout.

Archiving a conversation does not imply worktree deletion. A worktree is automatically removable only when Prompting Time owns it and it has no uncommitted changes, untracked files, unique local commits, or active process. Otherwise the UI reports the exact blocking state and offers safe next actions. Dirty, divergent, or ambiguous worktrees are never deleted automatically.

Every Git subprocess used by automatic inspection, workspace creation, cleanup eligibility, and shutdown has explicit input, output, and execution-time bounds appropriate to the operation. Exceeding a bound, failing to write bounded input, or timing out terminates and reaps the owned subprocess and becomes an explicit diagnostic or safe cleanup blocker; Prompting Time never treats incomplete Git inspection as evidence that deletion is safe.

Git commands and provider transports own an isolated subprocess group. Cleanup signals that
group before reaping the leader, including on natural leader exit, so ordinary inheriting hooks
and workers are stopped before rollback or ownership release. The guarantee covers signalable
members that remain in the group, not processes that deliberately escape into another group,
session, or credential boundary. Explicit cleanup reports bounded failures; Drop provides a
last-resort signal/reap path without a caller to receive errors.

## Public repository boundary

The public repository is named `prompting-time`. It contains source code, database migrations, generic fixtures, tests, CI configuration, and durable product and architecture documentation. Temporary brainstorming artifacts are ignored. The repository contains no company-specific skills, memories, conversations, machine paths, credentials, provider session data, or copied configuration.

Examples and test fixtures use invented projects, prompts, users, paths, and provider payloads. The tracked-files-only privacy scan checks private absolute home paths, known credential formats, provider authentication/state directories, and runtime or credential artifact extensions without printing matched values. A newline-delimited private literal denylist may be supplied only from outside the repository; its terms are never echoed. Public CI runs the generic scanner and its temporary-repository regression test, while publication additionally uses the external private denylist.

## Engineering constraints

- Prefer small modules with one responsibility and explicit ownership.
- Keep domain types independent of Tauri, SQLite, React, and provider protocols.
- Use exhaustive Rust enums for lifecycle states and typed conversion at protocol boundaries.
- Use one canonical decision path for routing, status rollup, approvals, and cleanup eligibility.
- Avoid speculative interfaces beyond the provider adapter and resource boundary already justified by the second provider and Milestone 2.
- Preserve errors with context and user-actionable categories; do not collapse protocol failures into strings at internal boundaries.
- Keep asynchronous tasks owned, cancellable, and observable. No detached background work may outlive its recorded owner silently.
- Treat generated bindings, schemas, and database migrations as generated or append-only artifacts where applicable; do not hand-edit generated output.
- Pin dependency ranges and protocol fixtures sufficiently for reproducible builds while allowing deliberate upgrades.

## Verification strategy

### Unit tests

Unit tests cover routing precedence and profiles, provider eligibility, recursive status rollup, lifecycle transitions, handoff construction and budgets, fallback safety, cleanup eligibility, redaction, and protocol-to-domain conversion.

### Contract and integration tests

Both adapters have hermetic process/contract coverage. Fake adapters and child processes exercise switching and handoff, streaming, malformed events, cancellation, process crashes, delayed approvals, backpressure, and resume boundaries without consuming provider usage. Real Claude adapter composition tests exercise projectless routing, persisted session resume, canonical approval/question responses, stale actions, and recursive sidebar ancestry through the application service.

Integration tests use temporary SQLite databases and temporary Git repositories to verify migrations, transactional recovery, concurrent dispatch claims, worktree creation, dirty-state protection, divergence detection, and cleanup. Invented protocol fixtures validate the supported adapter boundaries without retaining private provider content.

### UI tests

React component tests cover the recursive tree, status rollup display, timeline pagination, provider labels and overrides, routing explanations, approval prompts, inspector collapse behavior, and queued work. Rust tests cover Tauri command/state boundaries, notification policy, and event synchronization. No real native Tauri E2E or visual smoke test has been run for this release candidate.

### Live smoke tests

Focused opt-in smoke tests validate installed provider CLIs separately from hermetic checks. Claude protocol probes established streaming, approvals, interruption/resume, and depth-two task identity/lifecycle; actual-adapter live tests passed streaming/context recall and exact temporary Write denial/approval. Application smokes separately cover repeated projectless turns, cross-provider continuity, canonical approvals, and recursive completion. Their live results must be recorded before claiming application acceptance. None of these core application tests establishes a native visual walkthrough or notification delivery.

CI runs on macOS with pinned Node, pnpm, and Rust toolchains. It checks Rust formatting and linting, hermetic Rust and TypeScript tests, the frontend build, the privacy scanner and its regression test, and an unsigned macOS application bundle. Expensive or account-backed smoke tests do not run in public CI.

## Remaining Milestone 1 acceptance criteria

These are target acceptance criteria, not claims about the current release candidate. Milestone 1 is complete when:

- A user can install and launch Prompting Time locally on macOS.
- The app diagnoses the installed Codex and Claude CLIs without copying credentials.
- The user can create projectless and project-backed conversations.
- A Git-backed conversation defaults to an owned isolated worktree.
- Four root conversations can execute concurrently with accurate independent state, and a fifth is queued visibly by default.
- Automatic routing selects an eligible provider, shows its reason, and respects manual overrides.
- The user can switch providers between turns and inspect the context supplied to the new provider.
- Messages, streaming events, approvals, provider runs, and recursive agent nodes remain coherent across restart.
- The user can approve, deny, steer where supported, interrupt, resume, and inspect failures from the app.
- Fallback never automatically replays a turn after mutating activity.
- Unsafe worktree cleanup is blocked with an exact reason.
- The public repository passes its privacy scan and contains no private runtime data.
- Hermetic checks pass, and opt-in live smoke results are reported accurately rather than represented as hermetic coverage.

## Evidence and known risks

The design is grounded in current official interfaces:

- Codex App Server is intended for rich client integrations and exposes authentication, conversation history, approvals, streamed events, thread lifecycle, skills, and interruption: <https://learn.chatgpt.com/docs/app-server>.
- Codex and Claude Desktop use worktrees to isolate parallel project sessions: <https://learn.chatgpt.com/docs/environments/git-worktrees> and <https://code.claude.com/docs/en/worktrees>.
- Claude Code exposes structured streaming output, session identifiers, permission hooks, and deferred approval behavior: <https://code.claude.com/docs/en/headless>, <https://code.claude.com/docs/en/agent-sdk/user-input>, and <https://code.claude.com/docs/en/hooks>.
- Provider-neutral products expose sessions, status, permissions, diffs, and providers as separate concepts: <https://dev.opencode.ai/docs/server/>.
- Current automatic routers favor visible selection, task and health signals, session stickiness, optimization profiles, and deterministic fallback: <https://prod.cursor.com/help/models-and-usage/cursor-router>, <https://docs.github.com/en/copilot/concepts/models/auto-model-selection>, and <https://openrouter.ai/docs/guides/routing/routers/auto-router>.

Claude's direct integration now uses the validated Rust-owned stdio transport. Remaining risks include protocol drift across allowed CLI releases, absent grandchild text, and missing transcripts after cancellation before the first prompt followed by app restart. These boundaries fail closed and remain explicit in diagnostics and acceptance evidence; no sidecar or alternate transcript model is introduced.
