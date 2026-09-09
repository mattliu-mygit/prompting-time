# Compact tool operation history

Status: recommended direction approved; written specification awaiting review.

## Goal and scope

Make activity answer what the agent is doing, what it acted on, and whether it
succeeded. Cover newly observed Codex and Claude tool operations, their durable
history, and the conversation UI. Keep messages readable and the transcript compact.

The approved direction is compact operation rows with meaningful details. Merely
shrinking the existing status cards would preserve missing information; moving all
activity into the inspector would conceal useful progress. Neither is sufficient.

This does not add providers, change execution or permissions, replay commands,
import provider transcripts, expose hidden reasoning, or restart the desktop app.
No new UI dependency or separate activity database is required.

## Observable behavior

- One row represents one tool invocation. Starting, completing, and receiving more
  supported detail update that row without moving its position in the transcript.
  Identical commands with different invocation identities remain distinct.
- Rows show an evidence-backed action and target, such as `Read src/main.rs`,
  `Search "routing"`, or `Run cargo test`. Use provider-supplied action metadata or
  known tool inputs; do not parse arbitrary shell commands to invent a description.
  A generic tool name is the honest fallback when the provider supplies less detail.
- Status distinguishes running, succeeded, failed, interrupted, and unknown.
  Duration and exit information appear only when actually reported. A completed
  provider run does not prove that every tool succeeded.
- Consecutive completed, ordinary operations may collapse into a concise summary.
  Its count describes operations in the loaded history, not the entire run. Running
  work and failures remain directly visible. Groups never combine different runs,
  providers, agents, or activity separated by a message.
- Expanded groups contain compact rows, not repeated provider/category cards.
  Attribution stays visible at the homogeneous group level and on standalone rows.
- A row offers `Show details` only when there is additional captured information.
  Details identify command/input, output, error, and execution context when those
  fields are available. Empty output, unavailable output, and truncated output are
  different states; no control promises output that was not captured.
- Output may become available only on completion. This first slice does not require
  a live terminal, per-byte output streaming, or periodic elapsed-time animation.
- Content-free reasoning and routine protocol observations stay out of chat.
  Existing diagnostics remain the place for protocol metadata; hidden reasoning
  content is neither stored nor displayed. Approval and user-input controls remain
  in their existing actionable surface.

Keep the existing message typography and minimum 32-pixel interactive targets.
Ordinary operation summaries fit one compact line when space permits, wrap or
truncate safely on narrow windows, and never force horizontal page scrolling.
Details use bounded internal scrolling. Disclosures are keyboard accessible with
expanded state, explicit loading/error/retry feedback, and no live announcement for
every streaming update. Streaming and pagination preserve reading position, draft
text, and an already-open disclosure wherever its operation remains loaded.

## Data ownership and architecture

Adapters normalize supported provider fields into one provider-neutral operation
record. Rust owns identity, status transitions, retention limits, and persistence;
React renders the typed display projection without interpreting protocol strings.
Mutation certainty and permission handling remain independent of display status.
A failed command may already have changed files and must not enable unsafe retry.

Extend the existing canonical event store, which already updates streamed messages
in place. An operation has a stable app-owned identity and initial ordering position.
Native correlation includes its owning run, agent, and native turn where applicable;
native identifiers stay internal. Updates must not cross conversation or child-agent
ownership boundaries. Duplicate observations are idempotent. A delayed start cannot
regress an observed terminal state. Missing starts permit a completion-only row;
missing completion remains unknown rather than fabricated success. Interruption or
restart ends a stale running indicator without claiming a tool result was received.
Conflicting terminal observations surface an unknown outcome with a visible notice
rather than silently replacing failure evidence with success.

Use the same transactional ingress for normal delivery, approval-time staging, and
staged-event publication. Preserve the existing staging limits and combined content
plus payload byte accounting. Refreshes reconcile visible operations in older loaded
pages as well as the newest page, using bounded reads and stable cursors. Do not load
the complete conversation or merge identities from display text in the browser.

## Retention and privacy

Capture selected display fields for known command, file, search, and tool-call
events: action/tool label, relevant target or command, status, and supported textual
result/error plus reported duration/exit information. Known file tools expose their
target and supported change/output details, not unrelated files or extra filesystem
reads. General tool calls may expose their name and supported textual result; do not
persist arbitrary argument objects, protocol envelopes, or authentication metadata.
Unknown kinds retain a conservative label and available status, without raw payloads.

All retained detail stays in local application state. It must not enter diagnostic
logs, analytics, public fixtures, or provider handoff context through this change.
Tool output can itself contain sensitive text; no automatic secret-scrubbing guarantee
is made. Tests use invented data only. No private screenshots are committed.

Bound each operation's combined retained display detail to 256 KiB and its ordinary
timeline text preview to 1 KiB, reusing current limits. These are byte bounds with
UTF-8-safe truncation, not character counts. Truncation remains visible and copy
controls identify previews. Bound parsing, collection lengths, and aggregate buffers
as well as storage; new fields cannot bypass existing queue or frame limits.

## Existing conversations

Historical status-only records remain readable and compact, with
`Details weren't captured` when appropriate. They do not get an output disclosure
unless additional content really exists. Do not reconstruct missing output, guess
success from free text, or merge old invocations by command/status text. Preserve
legacy detail that actually exists and the existing bounded-detail behavior.

Do not hide ambiguous old records through a text-matching rule. Newly normalized
structured classifications control protocol-noise filtering. The upgrade preserves
all existing messages, approvals, agent ancestry, and conversation ordering.

## Verification and delivery

Use synthetic provider-shaped tests for both adapters, including complete input
arriving after an initially empty streamed tool block. Verify start/completion
pairing, repeated identical commands, duplicate and out-of-order observations,
completion without start, failures, absent/empty output, unknown tools, byte bounds,
and exclusion of reasoning/authentication envelopes.

Store/runtime coverage must include isolation between owners, approval staging and
replay, restart/interruption, migration of existing data, bounded detail, and updates
to an operation outside the newest page. UI tests cover meaningful summaries,
failure/running visibility, truthful disclosure, retry/stale-result handling,
keyboard use, narrow layouts, loaded-history counts, and scroll/draft preservation.

Run focused checks per boundary, then proportionate integration, frontend/build,
privacy, and independent code review. Browser fixture checks and a macOS bundle
build are not native WebView or live-provider evidence; report those limits unless
separately exercised. Do not restart an existing app or publish without authorization.

Before delivery, reconcile this approved behavior into the canonical product
specification, remove superseded temporary plans, and retain only useful regression
fixtures and current documentation. This feature specification should not become a
second conflicting description of the implemented product.
