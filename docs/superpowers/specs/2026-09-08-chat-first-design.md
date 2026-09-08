# Chat-first conversation experience

Status: proposed implementation contract following approval of the chat-first direction.

## Goal and scope

Make Prompting Time useful for reading and participating in a conversation while an
agent works. Preserve the recursive conversation/agent sidebar, collapsible right
inspector, automatic routing, and existing provider execution and approval semantics.
This change concerns the selected conversation surface and its bounded data projection,
not a new chat runtime or application-wide redesign.

The accepted approach is a chat-first hybrid. Merely shrinking existing event cards
would preserve their excessive prominence; hiding all activity would remove useful
visibility into agent work.

## Observed problems

The current message renderer emits plain paragraphs, not Markdown. Messages and
non-message events share bordered, padded cards. Every diagnostic gets a red border.
The durable event model uses diagnostics for both unrecognized provider notifications
and failures; the desktop projection currently omits the metadata that distinguishes
them. Timeline previews also truncate content after 1,024 characters, which can make
ordinary coding answers unnecessarily fragmented.

These are confirmed implementation facts, not conclusions about provider reliability.

## Conversation presentation

- User messages appear in compact, subtly filled bubbles aligned toward the right.
  Assistant replies use an unboxed reading column with a quiet provider label. User
  messages are labeled You rather than attributed to the selected provider.
- Assistant text supports headings, paragraphs, lists, emphasis, links, block quotes,
  tables, and fenced code. Code includes a language label when supplied, syntax
  highlighting, and a copy button. Message copy preserves the underlying text.
  User text preserves literal formatting. Copy failure is reported without claiming
  success or changing the message.
- Use a readable, centered column, comfortable line spacing, and smaller secondary
  metadata. Long code and tables scroll within their own containers without causing
  horizontal overflow of the window. The composer remains visible.
- Markdown is untrusted content: no executable HTML, embedded applications, automatic
  remote image fetching, or unsafe links. Rendering never executes code. Prefer a
  maintained Markdown renderer over a handwritten parser or wholesale UI framework.
- Ordinary replies should be readable without repeated disclosures. Give messages a
  larger but still bounded preview budget than telemetry; unusually large messages
  retain explicit truncation and on-demand bounded detail. Copying a preview must not
  pretend to copy the undisclosed full response.

## Activity and attention

- Consecutive tool/progress activity becomes a compact collapsed group between
  messages. Groups never cross a user/assistant message or run boundary. Provider and
  agent attribution remain available, including for nested agents.
- A summary describes only what canonical data supports. When the event provides no
  useful operation name or result, show a neutral label such as Tool activity rather
  than inventing Searching files, success, elapsed time, or a tool count.
- Expansion reveals individual bounded entries and explicit controls for longer
  output. Streaming updates preserve disclosure state by durable identity. A group
  containing only a loaded history fragment does not imply a complete turn history.
- Routine raw provider notifications belong in a collapsed Diagnostics section of
  the right inspector, not the chat transcript. Diagnostics are conversation-scoped,
  independently paged on explicit opening, and never fetched by workspace inspection
  or solely because streaming text changed. No notification is deleted from history.
- Classify presentation at the Rust projection boundary using stored structured
  evidence. Known terminal failures stay visible with error styling; interruption is
  a neutral status, not success or failure. Unrecognized protocol notifications use
  their stored method metadata. Unclassified legacy diagnostics remain visible as
  neutral notices rather than being silently hidden or labeled errors by a text regex.
- Pending approvals/questions and real failures never disappear inside collapsed
  activity or diagnostics. Preserve existing approval paging, owner binding, and
  focus behavior. A compact current-run status communicates working, waiting,
  interrupted, or failed using authoritative run state, without fabricating progress.

## Streaming and input

Follow incoming content while the reader is near the bottom. Scrolling up suspends
following; a keyboard-accessible Jump to latest control restores it. Loading older
history and expanding activity must preserve a stable reading position. Conversation
switches reset scroll/disclosure state appropriately and discard stale async results.

Keep the existing Send, Steer, and Interrupt behavior. Make the current action and
keyboard shortcut understandable near the composer; never imply a message was queued
when the runtime does not support queuing. Adding a queue, edit/retry branches,
attachments, artifact previews, and a new diff viewer are outside this pass.

## Architecture and invariants

SQLite remains the authoritative history. Extend the existing bounded Rust timeline
projection only enough to distinguish conversation-visible events from diagnostics.
Apply visibility filtering before pagination so a burst of hidden notifications cannot
consume a chat page and evict every reply. Diagnostics use the same underlying records
and detail ownership checks, not a duplicate log or a second canonical model. Older
records remain readable; do not rewrite conversation history to implement presentation.

Keep transport, permissions, provider routing, staging/recovery, and native sessions
unchanged. In React, separate message rendering and compact activity presentation from
the existing timeline's pagination and request coordination. Share presentation logic
where the inspector displays the same event detail. Do not replace the existing store
with another framework runtime or add automatic unbounded history/detail loading.

## Verification and delivery

Use focused regression tests for classification, filtered pagination across noisy
pages, unknown legacy diagnostics, Unicode truncation, and conversation ownership.
Component tests cover Markdown safety/formatting, copy feedback, grouping boundaries,
visible failures and approvals, on-demand diagnostics, stale responses, and disclosure
state. Existing timeline pagination and approval race tests must continue to pass.

Browser checks use synthetic data with long replies, tables/code, notification bursts,
nested agents, pending approvals, failure, and streaming. Verify wide and narrow windows,
keyboard access, scroll anchoring, Jump to latest, and no overflow. A browser fixture is
not native application acceptance. Run relevant Rust/frontend tests, lint/type checks,
formatting, independent review, privacy scan, and rebuild the macOS bundle before handoff.

Keep commits reviewable and local. Do not publish, restart the user's application,
import private data, or invoke account-backed provider tests as part of this UX change.
At completion, consolidate lasting behavior into the canonical product specification
and README and remove this superseded change spec and temporary implementation plans.

## Research informing the direction

- [assistant-ui Markdown](https://www.assistant-ui.com/elements/markdown-text):
  formatted responses and code-copy controls.
- [assistant-ui tool presentation](https://www.assistant-ui.com/docs/tools):
  compact expandable tool output and consecutive tool groups.
- [assistant-ui thread](https://www.assistant-ui.com/docs/primitives/thread):
  explicit chat viewport and scrolling behavior.
- [Claude Code Desktop](https://code.claude.com/docs/en/desktop):
  detailed work review separated from conversational interaction.
- [Cursor Agent](https://cursor.com/docs/agent/overview):
  distinguish queued instructions from steering an active turn; queuing is not adopted here.
- [Open WebUI chat features](https://docs.openwebui.com/features/chat-conversations/chat-features/):
  structured output and message-level actions.

These references informed interaction choices, not a commitment to adopt their
dependencies or reproduce their full feature sets.
