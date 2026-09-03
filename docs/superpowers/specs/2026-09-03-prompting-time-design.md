# Prompting Time Design

## Product intent

Prompting Time is a local macOS desktop application for managing coding-agent conversations across multiple installed CLI harnesses. The first release integrates Codex and Claude Code. It gives the user one place to create, monitor, steer, interrupt, resume, and switch providers across concurrent conversations without giving up either harness's native tools, authentication, approvals, or session persistence.

The application owns the user-facing conversation and execution hierarchy. Each provider continues to own its native session. This separation lets Prompting Time present consistent behavior without reducing Codex and Claude to a lowest-common-denominator API.

## Release boundaries

### Milestone 1: desktop multiplexer

Milestone 1 delivers:

- A locally installable macOS application named Prompting Time.
- Codex and Claude Code integrations through their installed local CLIs.
- Project-backed and projectless conversations.
- Multiple conversations executing concurrently.
- A recursive execution tree in which any agent may create child agents and any child may itself orchestrate descendants.
- Automatic provider routing with a visible manual override.
- Provider switching between turns with explicit context handoff.
- In-app approvals, questions, interruption, recovery, completion notifications, and diagnostics.
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

The UI must not invent child-agent identity. If a provider exposes a child agent, Prompting Time creates an agent node. If it exposes only an anonymous tool operation, the UI displays a tool event under the current agent.

### Conversation creation

A new conversation may target a local directory or have no project. For a Git project, isolated worktree execution is selected by default. The user may explicitly choose the current checkout. For a non-Git directory or projectless conversation, worktree controls are unavailable.

Before the first message, the user may choose automatic routing or pin Codex or Claude. Automatic routing remains the default. The selected provider and reason are visible on every provider run.

### Concurrent work

Conversations execute independently. The initial default limit is four active root runs across the application; the user may configure it. Additional work is queued visibly. A single conversation serializes normal turns, while child agents may run concurrently when the provider supports them.

The sidebar distinguishes queued, running, waiting for approval or input, completed, interrupted, and failed work. macOS notifications fire only when background work completes, fails, or needs the user.

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

Codex and Claude implement one capability-aware adapter contract. The contract covers provider discovery and health, session start and resume, message submission, active-turn steering when supported, interruption, approval or user-input responses, normalized event streaming, and capability reporting.

Capabilities are explicit rather than assumed. The UI disables or explains unsupported operations. Provider-specific options and native payloads remain isolated within the corresponding adapter.

The Codex adapter uses `codex app-server`, which exposes JSON-RPC threads, turns, streaming items, approvals, skill discovery, interruption, and session resumption. The Claude adapter uses Claude Code's structured streaming process interface, session identifiers, and approval hooks. Implementation must validate Claude's long-lived bidirectional flow and deferred approval behavior before building the full adapter.

Provider protocol versions are detected at startup. Missing, unauthenticated, unhealthy, or unsupported CLIs remain visible with actionable diagnostics but are excluded from automatic routing.

## Authoritative data model

Prompting Time stores one canonical representation of:

- Conversation: title, optional project/workspace, routing mode, and lifecycle metadata.
- Message: ordered user or assistant content displayed in the shared timeline.
- Provider run: one provider's execution of a turn, including routing decision, native session identity, status, and context boundary.
- Agent node: a self-referential execution node with an optional parent, provider identity, and rolled-up status.
- Event: ordered message, tool, progress, diagnostic, and lifecycle activity attached to a run and agent node.
- Approval: a durable pending or resolved provider request with the exact decision supplied by the user.
- Routing decision: eligible providers, chosen provider, profile, reason, and override state.
- Workspace: project identity, execution directory, worktree ownership, Git state, and cleanup eligibility.

Provider-native identifiers and raw protocol payloads are retained alongside normalized records where needed for resumption and diagnosis. They never replace the canonical model.

SQLite is the local source of truth. It uses WAL mode, versioned migrations, foreign-key enforcement, bounded queries, and indexes that support status/project filtering and timeline pagination. Durable state transitions are transactional. Large transcripts and event streams are paginated; application startup does not hydrate every conversation.

Runtime data is stored under the user's macOS Application Support directory. Conversations, prompts, tool output, provider payloads, machine paths, imported resources, and local configuration never enter the public repository.

## Routing

Automatic routing is deterministic and explainable in Milestone 1. Selection uses this precedence:

1. An explicit per-message or conversation provider override.
2. Provider eligibility: installed, supported, authenticated, healthy, and not known to be quota-blocked.
3. Continuity with the provider already handling the current line of work.
4. Task signals available from the message and conversation type.
5. Usage balancing among otherwise suitable providers.

The default profile is Balanced. Settings also expose Best fit and Usage balance. Profiles adjust the relative weight of task suitability and distribution; they do not bypass eligibility or manual overrides.

Every run records and displays the selected provider and a concise reason. Prompt classification runs locally in Rust and does not add a separate model call. The application records manual overrides and outcomes locally so a learned router can be evaluated in later work instead of being guessed into Milestone 1.

## Provider switching and context handoff

Switching providers occurs only at a turn boundary. Steering an active turn stays with its current provider. To switch during active work, the user first interrupts the run.

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

Approval decisions are scoped to the provider operation and persistence scope offered to the user. Destructive actions remain explicit. The application does not copy provider credentials; Codex and Claude continue using their existing authentication stores.

Diagnostic logs exclude prompt text, assistant content, tool output, file contents, credentials, and imported resources by default. Logs may contain timestamps, opaque local identifiers, provider/version information, state transitions, and sanitized error categories. Telemetry is disabled by default.

## Failure handling and recovery

The run supervisor records incoming events incrementally and treats provider streams as fallible. A process exit, malformed event, protocol incompatibility, or application restart cannot silently turn a partial run into a completed run.

Automatic fallback to the alternate provider occurs at most once and only when the first provider fails before any mutating tool activity. If mutation may have occurred, Prompting Time preserves the partial run, inspects current workspace state, and asks the user whether to continue, switch, or stop. It never replays a possibly mutating turn automatically.

On application restart, Prompting Time reconciles durable in-progress runs with operating-system process state and provider-native session state. Recoverable sessions may be resumed. Orphaned or ambiguous work is marked interrupted with a diagnostic and remains inspectable.

Per-conversation serialization prevents interleaved user turns. Bounded channels and pagination prevent unbounded memory growth. Cancellation propagates from the root run to owned processes, while provider-created descendants are reconciled from provider events.

## Workspace and worktree lifecycle

For a project-backed Git conversation, Prompting Time creates and records an isolated worktree before provider execution. All providers and descendants in that conversation share the recorded execution directory unless a later explicit feature introduces child isolation.

Using the current checkout is an explicit opt-out of isolation. The UI warns when another active conversation already uses that checkout.

Archiving a conversation does not imply worktree deletion. A worktree is automatically removable only when Prompting Time owns it and it has no uncommitted changes, untracked files, unique local commits, or active process. Otherwise the UI reports the exact blocking state and offers safe next actions. Dirty, divergent, or ambiguous worktrees are never deleted automatically.

## Public repository boundary

The public repository is named `prompting-time`. It contains source code, database migrations, generic fixtures, tests, CI configuration, and durable product and architecture documentation. Temporary brainstorming artifacts are ignored. The repository contains no company-specific skills, memories, conversations, machine paths, credentials, provider session data, or copied configuration.

Examples and test fixtures use invented projects, prompts, users, paths, and provider payloads. A pre-publication scan checks for secrets, machine-specific paths, and organization-specific terms.

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

Both adapters run through a shared behavioral contract suite. Fake child processes exercise streaming, malformed events, cancellation, process crashes, delayed approvals, backpressure, and resume flows without consuming provider usage.

Integration tests use temporary SQLite databases and temporary Git repositories to verify migrations, transactional recovery, concurrent writers, worktree creation, dirty-state protection, divergence detection, and cleanup. Recorded sanitized fixtures validate the supported Codex and Claude protocol shapes.

### UI tests

React component tests cover the recursive tree, status rollup display, timeline pagination, provider labels and overrides, routing explanations, approval prompts, inspector collapse behavior, and queued work. End-to-end tests cover critical Tauri commands and state/event synchronization where the macOS test environment supports them.

### Live smoke tests

Focused opt-in smoke tests validate discovery, authentication status, one projectless conversation, one isolated-worktree conversation, streaming, interruption, approval, resumption, and provider switching against installed Codex and Claude CLIs. They are reported separately from hermetic automated tests.

CI runs formatting, Rust and TypeScript linting, unit and integration tests, dependency audits, fixture compatibility checks, and an unsigned macOS release build. Expensive or account-backed smoke tests do not run in public CI.

## Milestone 1 acceptance criteria

Milestone 1 is complete when:

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

The highest implementation risk is Claude's direct interactive integration from a Rust-owned process. Official documentation establishes the required primitives, but the first implementation task must run focused experiments for long-lived streaming input, approval deferral/resume, interruption, and child-agent event identity. If direct CLI control cannot meet the contract reliably, the approved fallback is a narrowly scoped Claude Agent SDK sidecar behind the same Rust adapter boundary. That fallback changes packaging, not the domain model or UI.
