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

- The left pane groups root conversations by project and status. Expanding any conversation reveals its child conversations, recursively. Each node shows its own provider and current status.
- The center pane prioritizes the selected conversation: readable assistant responses, compact user messages, expandable tool activity, approvals, and the composer. Provider identity stays visible without dominating the text.
- The right inspector shows routing rationale, workspace and worktree state, changed files, active agents, and explicitly disclosed diagnostics. It is collapsible for focus and smaller windows.

One 48–56px application header holds the selected conversation title, actual
provider and run status, pane toggles, and a More menu for archival. There is no
second workspace title row or visible Timeline heading. The sidebar heading shares
a row with Search, New conversation, and a status filter; Search and New move to
the header when the sidebar is hidden. The filter retains its current selection
while toggling the sidebar. Menu cancellation restores the trigger, and menu-to-
dialog or palette transitions restore focus only to controls that remain mounted.
Shared dropdown primitives and icons avoid separate custom menu machinery.

The inspector starts closed, leaving the conversation primary. Explicitly opening
or closing it retains that choice while switching conversations. At smaller widths,
it overlays the workspace instead of squeezing the reading column; its background
is noninteractive, keyboard focus stays inside, and closing restores the trigger.
New windows default to 1440 × 900, with a minimum of 960 × 600. This does not reset
the size of an already open window.

Readable defaults use 17px chat, preview and composer text, 14px controls/sidebar
labels and code, and at least 13px secondary text at standard root scaling. Existing
larger headings retain their hierarchy. Typed text uses the normal foreground color;
placeholder and metadata remain secondary. Shared rem-based sizes preserve scaling
without global zoom. Controls have at least 32px height, icon targets are 32px square,
and conversation rows have at least 36px height. Long text wraps or scrolls within
its own region rather than creating horizontal window overflow.
Conversation names occupy their own row, with provider/status metadata
below, so badges do not crowd out nested names.

Users can scale the entire native webview with Command-plus (including unshifted
Command-equals), Command-minus, and Command-zero to reset. The display preference
starts at 100%, advances in 25-percentage-point steps between 75% and 200%, and is
remembered locally after successful application. It applies across conversations
and while editing, without changing draft contents. Missing or invalid storage
falls back to 100%; storage failures do not prevent using zoom. This explicit user
zoom is separate from the default typography sizes above.

Unsent composer text, provider choice, and Markdown preview belong to their
conversation rather than its mounted view. Switching conversations and refreshing
their snapshots preserves separate drafts within the app session. Drafts, composer
view state, and pending-send bookkeeping stay in memory only;
app shutdown discards them. Successful send or steering clears the submitted
version, never a newer edit or another conversation's draft. Switching away and
back during a request must not bypass in-flight or ambiguous-retry protection.

A shared searchable palette opens from Search or Command-K (Control-K also works).
It searches active conversation titles and project paths and exposes existing
new-conversation, pane-toggle, and focus-message actions. It is not message-content
search. Conversation selection focuses its composer when messaging is available;
read-only views retain an accessible reading or navigation target. Cancellation
restores the prior focus. Existing blocking dialogs and narrow inspector overlays retain
keyboard priority. The palette uses cmdk for filtering and selection, Radix Dialog
for dialog and focus management, and react-hotkeys-hook for shortcut registration,
with our existing styling and store actions; no parallel action framework or
additional state library.

Reading context is session-local. Returning to a recently visited conversation
restores its visible message and expanded activity, including explicitly loaded
older history. Readers stay at their place when newer output arrives; views that
were following latest continue to follow. Retention is bounded to ten recent
conversation views and the existing per-view history limits. Evicted context is
not a durable bookmark. Cached reading content does not make cached approvals
actionable; current approval state must still be read from the service.

Roots and descendants use one conversation summary, selection state, timeline,
composer, and tree-row implementation. Selecting a child opens its own captured
activity through the ordinary conversation read path. A compact, horizontally
bounded ancestor path provides explicit parent navigation; the root needs no
duplicate breadcrumb. The palette finds loaded child conversations. Missing or
bounded ancestry is marked incomplete, never invented.

Actions depend on the verified execution binding, not the presence of a parent.
Provider-observed children expose recorded activity and supported approval handling,
but do not imply independently resumable or controllable sessions. Unsupported
send, routing, interruption, and archive controls are absent and service-rejected.
The shared composer targets the selected conversation or explains why messaging is
unavailable; it never silently submits to the parent. Parent drafts, preview,
reading anchors, and disclosures remain scoped to the parent's conversation identity.

The shell is constrained to the window height. Long timelines, conversation lists, and inspector
content scroll within their panes instead of pushing the composer below the window. Selecting a
child preserves usable navigation and the conversation workspace. An active interruptible turn
has its own Interrupt action even when steering is available or no alternative provider is
available; confirmation remains bound to the original run.

The UI must not invent child identity or transcripts. Verified provider child
ancestry creates an internal execution node bound to a durable child Conversation.
An anonymous tool operation remains an event under its actual owner. A child with
only lifecycle or tool evidence explicitly explains that message history may be
incomplete; no greeting, hidden reasoning, or historical transcript is fabricated.

The center timeline keeps the newest 80 events plus at most four explicitly loaded
80-event history pages. Coalesced live refresh reads the newest window and those
retained older windows using their original request cursors, so an older operation
can update without loading the full conversation. Durable event identity and first
sequence preserve ordering; overlapping refresh, paging, or conversation changes
cannot reintroduce stale rows. Reading anchors and disclosed operations are retained,
including on return to a recently visited view. Genuine gaps or history eviction are
visible and offer a way back to the newest window. Detail reads stay lazy, refresh by
revision only while disclosed, and never fan out over hidden rows. Every truncated
event kind remains visibly truncated with separately bounded detail.

Child conversations belong in sidebar navigation, not a duplicate agent-card list
inside the transcript. Pending approvals load 30 summaries initially, page
independently on explicit request, and never fetch operation or question detail
merely because a card is mounted. The approval view retains at most four pages:
loading farther back evicts the newest retained page but preserves the returned
cursor until every pending request is reachable, and an explicit control restores
the newest page. Live invalidation rebases that bounded window onto the newest
approvals by following the refreshed cursor chain, so insertion-driven page shifts
cannot skip requests and terminal or missing requests disappear without unbounded
fan-out. Explicit approval paging owns an independent busy token so overlapping
refresh success or failure cannot strand its retry controls.

### Reading and participating in a conversation

Assistant responses use an unboxed reading column with Markdown paragraphs, headings,
lists, links, block quotes, tables, and fenced code. User messages render Markdown
in subtly filled bubbles. Code can be copied independently and is syntax-highlighted
within a bounded budget; message copy preserves underlying text and distinguishes a
preview from an undisclosed full response. Long code and tables scroll internally.
Message copy sits beside the sender in a visible, keyboard-accessible icon control
with the same minimum target size and explicit success/failure feedback. Compact
bubble padding and inter-message gaps reduce vertical overhead without changing
message typography or internal Markdown paragraph, code, and table spacing.
Untrusted output cannot execute HTML or code, embed applications, load remote images
automatically, or activate unsafe URL protocols. Ordinary HTTP(S) links open in the
default browser through a main-window capability restricted to those two schemes.
Local fragments remain in the document; other external URL schemes are inert.

Newly captured tool invocations have one compact row with an evidence-backed action
and target. Starting, completing, and enriching an operation update its existing
app-owned identity without moving its first sequence. Identical commands remain
distinct invocations. Status distinguishes running, succeeded, failed, interrupted,
and unknown; duration and exit information appear only when reported. A completed
run does not establish success for a tool whose result was never observed.

Consecutive successful ordinary tools and progress entries collapse into compact
activity, without merging across messages, runs, providers, or agents. Summary counts
describe loaded operations, not the entire run. Expanded groups show compact rows
with attribution once at the group level. Running, failed, interrupted, unknown,
and conflicting tools, and approval requests remain directly visible. Show details appears only
for additional captured information and lazily reads selected input, output, error,
and context. Missing, empty, and truncated output remain distinguishable; copied
bounded content is labeled as a preview. Details stay open across updates while
their operation remains loaded. Legacy status-only rows keep their literal text and
explain unavailable details; neither output nor identity is guessed from that text.
Message typography and minimum 32-pixel controls are unchanged. Tool details scroll
internally and never execute content or force horizontal page scrolling.

Routine lifecycle updates are quiet status rows, and current
run status comes from authoritative conversation state rather than the first root in
an arbitrary agent page. Ordinary, nontruncated lifecycle entries use one muted
wrapping row with their canonical content and provider/agent attribution. Notices,
failures, and truncated lifecycle entries retain the full visible detail layout.
This presentation does not filter events, change grouping, or move timeline anchors.

The Rust read projection separates routine protocol notifications from conversation
events using stored structured evidence before pagination. Both views read the same
durable history; the UI neither deletes notifications nor copies them into another
log. Failure evidence takes precedence over routine-notification metadata. Legacy
diagnostics without enough evidence stay visible as neutral notices instead of being
silently hidden or classified by a text regex. Message previews have a larger bounded
budget than routine activity so ordinary coding answers remain readable.

Inspector diagnostics load only on explicit disclosure, paging, or refresh, independently
of workspace inspection and streaming invalidations. Their bounded history window and
any eviction are explicit; details remain lazy. Every request remains tied to its
conversation, with stale responses discarded on switching or closing.

Streaming follows the newest reply while the reader stays at the bottom. Scrolling up
pauses following; Jump to latest restores it. Older-page loading and activity disclosure
preserve a stable reading position. The composer remains visible and explains its
existing Send/Steer/Interrupt behavior. Enter submits the current Send or Steer action;
Shift+Enter inserts a newline. Cmd/Ctrl+Enter also submits, but Shift takes precedence.
Composition confirmation and held-key repeats do not submit. An unavailable action
does not silently queue or interrupt an active run.

The composer edits raw Markdown by default, with an explicit Preview/Edit toggle
using the same safe rendering as the transcript. Returning to edit preserves the
draft and caret. Nonblank text is submitted without stripping Markdown indentation
or trailing line breaks; failed submission retains it, while successful submission
clears it and returns focus to the input. The input grows with content and wrapping,
then scrolls internally at a bounded height; preview is similarly bounded. A message
queue, message branch editing, attachment support, and artifact or diff viewers are
separate work.

At rest the composer contains the message field and one footer with provider,
help, preview and submission controls, plus interruption when supported. Routine
routing guidance and keyboard hints appear below the footer only when help is
expanded. Failures and blocking explanations remain visible independently. This
consolidation preserves text sizes, minimum control targets, and the readable
message-column width; it does not change routing or submission semantics.

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

The UI submits question answers only from complete canonical question data supplied by a bounded question page or complete approval detail. A complete question whose `options` are null is a canonical free-text question; `isOther` adds free text alongside enumerated choices. Missing or truncated question data blocks the response with an actionable explanation. Approval detail also supplies a bounded canonical requesting-agent label path, independently of which child-conversation pages the user has disclosed; paths beyond the bound are explicitly marked truncated.

The sidebar distinguishes queued, running, waiting for approval or input,
completed, interrupted, and failed work. Child pages use stable newest-first
creation order, independently of the parent's latest run. The sidebar retains at
most 80 descendant summaries across branches, including its pinned selected path.
Ancestry reads return the nearest 64 contiguous conversations including the selected
one, with explicit truncation and independent execution-owner provenance. Disclosure
never loads full execution history.

macOS notifications fire only while the main window is unfocused and only when a
root conversation becomes completed, failed, or needs attention. Repeated
observations of the same state are deduplicated for every active conversation
observed during the process lifetime. Startup primes that state without notifying.
If the compact store-change channel lags, notification state is rebuilt from
active roots in bounded 200-row pages so a dropped terminal or needs-attention
change is still observed; after a complete scan, entries absent from the active
set are pruned. The notification contains only the fixed app title and a fixed
status label; conversation titles, prompt text, output, tool details, paths, file
names, and provider-native identity are excluded. Permission is requested lazily
on the first eligible notification.

### Recursive conversation hierarchy

One durable Conversation represents every navigable chat. Its optional parent is
the authority for hierarchy; a child may itself orchestrate further children.
Provider execution nodes remain internal correlation and lifecycle records, each
immutably bound to one Conversation. Root executions reuse their existing root
conversation, while verified nonroot executions get children beneath their bound
parent. Missing parents, cycles, and conflicting or cross-owner bindings fail closed.

Observed child identity retains the existing provider-run scope. Replaying one
recorded execution reuses its conversation, including after database reopen; the
same raw native child ID observed in a later parent run creates a separate child
conversation. Labels do not establish continuity. Completed children remain
discoverable after later parent turns, and each child's own status comes from its
bound execution rather than the parent's newest run. Current descendant activity
and pending attention remain visible from the parent.

Root listing excludes children. Archiving a root hides its subtree without deleting
its records. Children inherit the actual execution owner's workspace context, not
its exclusive workspace association, cleanup rights, or provider session. Creating
a child allocates no worktree and transfers no control or permission ownership.

Each generation has visible indentation and a parent guide. Deep visual indentation
is capped to preserve readable names and usable controls; semantic ancestry remains
explicit. The selected root reveals its first child level when children
first appear unless explicitly collapsed. Deeper disclosure is explicit, and
explicit descendant navigation reveals ancestors. Choices survive switches and
sidebar hide/show within the session, not app restarts. Bounded child paging keeps
closed branches lazy, refreshes open branches when children arrive, and offers
retry or reload after failures and eviction. A complete contiguous reload clears
the eviction notice unless the retention budget evicts records again. Root-owned execution invalidation
also refreshes an affected visible child; selection is not discarded on a new
parent run.

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

- Conversation: one reusable root or descendant chat with an optional parent, a normalized nonblank title of at most 256 UTF-8 bytes, optional owned project/workspace, routing mode, and lifecycle metadata. Legacy titles are defensively bounded when read, and whitespace-only legacy values display as `Untitled conversation`.
- Message: ordered user or assistant content displayed in the shared timeline.
- Provider run: one provider's execution of a turn, including routing decision, native session identity, status, and context boundary.
- Agent node: an internal execution node with verified ancestry, provider identity, status, and an immutable conversation binding; not a second product-facing chat model.
- Event: ordered message, tool, progress, diagnostic, and lifecycle activity attached to a run and agent node.
- Approval: a durable pending or resolved provider request with the exact decision supplied by the user.
- Routing decision: eligible providers, chosen provider, profile, reason, and override state.
- Workspace: project identity, execution directory, worktree ownership, Git state, and cleanup eligibility.

Provider-native identifiers and narrowly selected protocol fields are retained alongside normalized records where needed for resumption and diagnosis. Raw payload retention is opt-in at each adapter boundary and must exclude hidden reasoning, authentication material, and unrecognized payload content. Provider-native data never replaces the canonical model.

Hierarchy upgrades backfill verified execution ancestry transactionally while
preserving existing root, run, agent, event, approval, and workspace identities.
Original event ownership, sequences, operation revisions, and runtime fences do
not change. Conversation reads resolve their bound execution owners before indexed
pagination; child history is never a filter over the parent's newest page. Only
read projections use the selected logical conversation ID. Event bodies are not
copied into another transcript store. Child selection does not expose the parent's
handoff or routing audit, and steering/interruption validate both the selected
conversation and the exact managed run before dispatch.

SQLite is the local source of truth. It uses WAL mode, versioned migrations, foreign-key enforcement, bounded queries, and indexes that support status/project filtering and timeline pagination. Ordinary sidebar synchronization filters archived conversations at this query boundary; archived history remains available only through an explicit future archive surface. Durable state transitions are transactional. User and assistant messages occupy the same role-bearing event sequence; the separate message projection exists only for provider handoff context. Upgrades reconstruct historical user events from that projection, replace transitional user-event copies, and deterministically place a user turn before provider events when old independent sequences share a timestamp. Desktop timeline pages expose UTF-8-safe bounded previews and fetch a separately bounded event detail by the app-owned event ID. Child conversations, provider-run audits, and pending or historical approvals use stable independent cursors, so a sidebar cutoff never makes descendants, historical routing, or actionable requests inaccessible. Provider-run audit summaries expose only app-owned run identity, canonical provider and routing reason, and explicit truncation state; a conversation-scoped detail read returns the bounded routing evaluation and UTF-8-safe bounded handoff without exposing native session identity. Approval rows carry their canonical conversation ownership and use conversation-first partial indexes for pending and historical pages. Approval question previews are normalized once in the approval transaction and paged through an indexed app-owned ordinal; ordinary snapshots contain only app-owned IDs and bounded canonical fields, while provider-native request and question identities remain internal and are mapped at provider dispatch. Application startup pages queued roots and processes ambiguous unfinished agents in indexed, deepest-first batches under a fixed deadline rather than hydrating every conversation or active graph.

Provider output received while an approval is pending is durably staged in receipt order without changing the Waiting lifecycle. Assistant deltas with the same native item ID aggregate into one durable message before, during, and after staging, including after restart, so a streamed message never becomes a row per token. Each run's staged queue accepts up to 256 complete provider events, with one additional physical row reserved for an overflow marker; the full queue, including that marker reserve, is limited to 8 MiB of content. Events within that capacity retain their full content. The first event that would exceed either limit is replaced by one compact diagnostic marker that records the omitted event kind and makes mutation certainty Unknown; later staged ingress is rejected. Recovery returns at most the 257-row physical limit together with explicit overflow and truncation flags. Once an approval response is accepted by the supervisor, the supervisor owns its complete intent, provider-dispatch, acknowledgement, publication, and cleanup lifecycle independently of the requesting UI future; interruption and shutdown cancel and join that operation. Approval acknowledgement atomically publishes the resumed lifecycle, any privacy-filtered accepted-answer message, and each staged event once, then clears the queue; interruption, failure, or crash performs the same bounded publish before the terminal diagnostic, so restart recovery retains bounded evidence of already-observed activity.

Runtime data is stored under the user's macOS Application Support directory. Conversations, prompts, tool output, provider payloads, machine paths, imported resources, and local configuration never enter the public repository.

Tool operations use the same canonical event store and transactional staging path,
not a separate activity log. Correlation includes the owning run, materialized agent,
native turn where available, and invocation identity. Duplicate snapshots are
idempotent; delayed starts cannot regress terminal evidence. Conflicting terminal
observations become an explicit unknown outcome without erasing failure evidence.
Ended runs, agents, or child turns cannot leave a stale running indicator. Unresolved
child ownership never becomes root activity. Pending child tools are published only
after verified owner registration and before its terminal state. Display status and
optional detail retention cannot change mutation certainty or permission decisions.

Adapters retain selected display fields from supported command, file, search, and
tool calls. Unknown tools retain a conservative label and supported textual result,
not arbitrary arguments, protocol envelopes, credentials, or hidden reasoning.
Combined operation detail is limited to 256 KiB with UTF-8-safe truncation; ordinary
timeline titles are limited to 1 KiB. Summary reads expose neither native identities
nor full input/output. Details are fetched only by app-owned event ID on disclosure,
with revision-based refresh. Parsing and pending buffers remain bounded as well as
stored data. Claude's per-turn display cache caps retained content at 8 MiB; pressure
truncates optional details explicitly instead of failing an otherwise valid run.
Existing protocol frame and identity limits remain independent. This change does
not feed tool detail into provider handoff context or
promise automatic secret removal from locally retained output.

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
