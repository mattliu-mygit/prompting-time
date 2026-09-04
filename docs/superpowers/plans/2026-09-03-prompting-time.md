# Prompting Time Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a local macOS desktop application that manages concurrent Codex and Claude Code conversations through their installed CLI harnesses, with automatic routing, provider switching, approvals, recursive agent trees, and safe Git worktree isolation.

**Architecture:** Tauri 2 hosts a React/TypeScript interface and a Rust application core. The Rust core owns the canonical conversation model, SQLite persistence, process supervision, routing, handoff, and worktrees; Codex App Server and Claude Code streaming JSON are isolated behind one capability-aware adapter contract.

**Tech Stack:** Rust 1.98.1, Cargo workspace, Tokio, SQLx 0.9 with bundled SQLite, Tauri 2, React 19, TypeScript, Vite, Vitest, Testing Library, pnpm 11, macOS.

## Global Constraints

- The public application and repository name is `Prompting Time` / `prompting-time`.
- Milestone 1 supports macOS only and produces a locally installable unsigned `.app`.
- Use installed local Codex and Claude Code authentication; never copy provider credentials.
- Automatic routing is the default, remains explainable, and always permits a visible manual override.
- Provider switching occurs only at turn boundaries; active work must be interrupted before switching.
- Projectless conversations are supported; Git project conversations default to an owned isolated worktree.
- Four root runs execute concurrently by default and a fifth queues visibly.
- Any agent node may have child agents; never invent child identity when a provider does not expose it.
- Imported skills, memories, and referenced files are Milestone 2; Milestone 1 contains only the resource boundary.
- Private runtime content lives under macOS Application Support and must never enter Git or default diagnostics.
- Never auto-retry a provider turn after mutating activity may have occurred.
- Dirty, divergent, ambiguous, or active worktrees are never deleted automatically.
- Keep domain code independent of Tauri, SQLx, React, and provider wire formats.
- Use direct, explicit control flow and add abstractions only where the two providers or Milestone 2 justify them.

---

## Planned file structure

```text
prompting-time/
├── Cargo.toml                         # Rust workspace
├── rust-toolchain.toml                # Pinned Rust toolchain
├── package.json                       # Frontend and Tauri commands
├── pnpm-lock.yaml                     # Reproducible JavaScript dependencies
├── vite.config.ts                     # Vite and test configuration
├── tsconfig.json
├── tsconfig.node.json
├── eslint.config.js
├── index.html
├── assets/app-icon.svg                # Source icon for generated Tauri icons
├── crates/prompting-time-core/
│   ├── Cargo.toml
│   ├── migrations/0001_initial.sql
│   ├── src/
│   │   ├── lib.rs
│   │   ├── domain.rs                  # Canonical IDs, states, records, status rollup
│   │   ├── error.rs                   # Typed internal errors and user-action categories
│   │   ├── store.rs                   # SQLite persistence
│   │   ├── router.rs                  # Deterministic provider selection
│   │   ├── handoff.rs                 # Bounded cross-provider context capsules
│   │   ├── workspace.rs               # Git worktree preparation and cleanup eligibility
│   │   ├── runtime.rs                 # Owned concurrency, cancellation, event ingestion
│   │   ├── app.rs                     # Application use cases
│   │   └── providers/
│   │       ├── mod.rs                 # ProviderAdapter and normalized provider events
│   │       ├── process.rs             # JSON-line subprocess transport
│   │       ├── codex.rs               # Codex App Server adapter
│   │       └── claude.rs              # Claude Code adapter
│   └── tests/
│       ├── fixtures/codex/*.jsonl
│       ├── fixtures/claude/*.jsonl
│       ├── adapter_contract.rs
│       └── recovery.rs
├── src-tauri/
│   ├── Cargo.toml
│   ├── build.rs
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   └── src/{lib.rs,main.rs,commands.rs,state.rs}
├── src/
│   ├── main.tsx
│   ├── test/setup.ts
│   ├── app/App.tsx
│   ├── app/store.ts
│   ├── bridge/{api.ts,types.ts}
│   ├── features/conversations/{ConversationTree.tsx,ConversationTree.test.tsx}
│   ├── features/timeline/{Timeline.tsx,Timeline.test.tsx,Composer.tsx}
│   ├── features/inspector/{Inspector.tsx,ApprovalCard.tsx,Inspector.test.tsx}
│   └── styles/{tokens.css,app.css}
├── scripts/privacy-scan.sh
├── .github/workflows/ci.yml
└── README.md
```

The core remains one crate until compile times, reuse, or ownership justify another crate. Files split by responsibility; do not create repository/service interfaces for modules that have only one implementation.

---

### Task 1: Bootstrap a runnable Tauri app with provider diagnostics

**Files:**
- Create: `rust-toolchain.toml`
- Create: `Cargo.toml`
- Create: `crates/prompting-time-core/Cargo.toml`
- Create: `crates/prompting-time-core/src/lib.rs`
- Create: `crates/prompting-time-core/src/providers/mod.rs`
- Create: `src-tauri/Cargo.toml`
- Create: `src-tauri/build.rs`
- Create: `src-tauri/tauri.conf.json`
- Create: `src-tauri/capabilities/default.json`
- Create: `src-tauri/src/{main.rs,lib.rs,commands.rs,state.rs}`
- Create: `package.json`, `pnpm-lock.yaml`, `vite.config.ts`, `tsconfig.json`, `index.html`
- Create: `tsconfig.node.json`, `eslint.config.js`, `src/test/setup.ts`
- Create: `assets/app-icon.svg`, generated `src-tauri/icons/*`
- Create: `src/main.tsx`, `src/app/App.tsx`, `src/app/App.test.tsx`, `src/bridge/{api.ts,types.ts}`
- Modify: `.gitignore`

**Interfaces:**
- Produces: `ProviderInstallation`, `ProviderId`, `discover_provider(binary: &str, id: ProviderId) -> Result<ProviderInstallation, ProviderError>`
- Produces: Tauri command `bootstrap() -> Result<BootstrapSnapshot, CommandError>`
- Produces: frontend `getBootstrap(): Promise<BootstrapSnapshot>`

- [ ] **Step 1: Install and verify the pinned development toolchain**

Run:

```bash
brew install rustup
rustup toolchain install 1.98.1 --profile minimal --component clippy,rustfmt
rustc --version
cargo --version
xcode-select -p
node --version
pnpm --version
```

Expected: Rust reports `1.98.1`, Xcode Command Line Tools resolve, Node is at least 24, and pnpm is 11.x. If `rustc` is not on `PATH`, add the Homebrew rustup shim using Homebrew's printed instruction; do not install a second Rust distribution.

- [ ] **Step 2: Create the workspace manifests and frontend test harness**

Use these version boundaries and commit the resolved lockfiles:

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.98.1"
profile = "minimal"
components = ["clippy", "rustfmt"]
```

```toml
# Cargo.toml
[workspace]
members = ["crates/prompting-time-core", "src-tauri"]
resolver = "3"

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["macros", "process", "rt-multi-thread", "sync", "time"] }
uuid = { version = "1", features = ["serde", "v7"] }
```

```json
// package.json
{
  "name": "prompting-time",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "packageManager": "pnpm@11.19.0",
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "test": "vitest run",
    "lint": "eslint . --max-warnings=0",
    "tauri": "tauri"
  },
  "dependencies": {
    "@tauri-apps/api": "^2",
    "react": "^19",
    "react-dom": "^19"
  },
  "devDependencies": {
    "@eslint/js": "^9",
    "@tauri-apps/cli": "^2",
    "@testing-library/jest-dom": "^6",
    "@testing-library/react": "^16",
    "@types/react": "^19",
    "@types/react-dom": "^19",
    "@vitejs/plugin-react": "latest",
    "eslint": "^9",
    "globals": "latest",
    "jsdom": "latest",
    "typescript": "^5.9",
    "typescript-eslint": "latest",
    "vite": "latest",
    "vitest": "latest"
  }
}
```

Run `pnpm install`. Expected: `pnpm-lock.yaml` is created without peer dependency errors.

Create a simple source-controlled SVG icon combining a clock face and prompt chevron, then run `pnpm tauri icon assets/app-icon.svg`. Generated platform icon files are committed; later visual polish modifies the SVG source and regenerates them rather than editing generated icons.

- [ ] **Step 3: Write failing provider-discovery and app-shell tests**

```rust
// crates/prompting-time-core/src/providers/mod.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_binary_is_reported_without_panicking() {
        let result = discover_provider("definitely-not-installed", ProviderId::Codex).await;
        assert!(matches!(result, Err(ProviderError::NotInstalled { .. })));
    }
}
```

```tsx
// src/app/App.test.tsx
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";

describe("App", () => {
  it("shows both provider diagnostics", async () => {
    const bootstrap = vi.fn().mockResolvedValue({
      providers: [
        { id: "codex", installed: true, version: "0.144.1", diagnostic: null },
        { id: "claude", installed: true, version: "2.1.205", diagnostic: null }
      ]
    });
    render(<App bootstrap={bootstrap} />);
    expect(await screen.findByText("Codex 0.144.1")).toBeVisible();
    expect(screen.getByText("Claude 2.1.205")).toBeVisible();
  });
});
```

Run:

```bash
cargo test -p prompting-time-core missing_binary_is_reported_without_panicking
pnpm test -- src/app/App.test.tsx
```

Expected: both fail because the provider types/discovery and `App` do not exist.

- [ ] **Step 4: Implement provider discovery, the thin Tauri bootstrap command, and a minimal health screen**

Implement `ProviderId` as a serde-tagged enum and discover binaries with `tokio::process::Command`, invoking `codex --version` or `claude --version` with a five-second timeout. Parse only the first non-warning version line; retain the raw sanitized diagnostic on failure.

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId { Codex, Claude }

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallation {
    pub id: ProviderId,
    pub installed: bool,
    pub version: Option<String>,
    pub diagnostic: Option<String>,
}
```

`src-tauri/src/commands.rs` calls the core twice and returns a `BootstrapSnapshot`; `src/bridge/api.ts` wraps `invoke("bootstrap")`; `App.tsx` receives `bootstrap` as an injectable prop and renders loading, success, and actionable error states. Configure the bundle identifier as `com.promptingtime.app`, window title as `Prompting Time`, and macOS-only bundle target as `app`.

- [ ] **Step 5: Verify the app shell**

Run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm test
pnpm build
pnpm tauri build --bundles app
```

Expected: all checks pass and `src-tauri/target/release/bundle/macos/Prompting Time.app` exists. Launch it manually and confirm both installed provider versions render.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml crates src-tauri package.json pnpm-lock.yaml vite.config.ts tsconfig.json tsconfig.node.json eslint.config.js index.html assets src .gitignore
git commit -m "feat: bootstrap Prompting Time desktop app"
```

---

### Task 2: Define canonical lifecycle and recursive agent state

**Files:**
- Create: `crates/prompting-time-core/src/domain.rs`
- Create: `crates/prompting-time-core/src/error.rs`
- Modify: `crates/prompting-time-core/src/lib.rs`

**Interfaces:**
- Produces: `Conversation`, `Message`, `ProviderRun`, `AgentNode`, `TimelineEvent`, `Approval`, `Workspace`, and strongly typed IDs
- Produces: `roll_up_status(root: AgentId, agents: &[AgentNode]) -> Result<RollupStatus, DomainError>`
- Produces: legal transition methods `ProviderRun::transition` and `Approval::resolve`

- [ ] **Step 1: Write failing lifecycle tests**

```rust
#[test]
fn waiting_descendant_rolls_attention_to_root() {
    let run = RunId::new();
    let root = AgentNode::root(run, ProviderId::Codex, "orchestrator");
    let child = AgentNode::child(run, root.id, ProviderId::Claude, "reviewer", AgentStatus::Waiting);
    let grandchild = AgentNode::child(run, child.id, ProviderId::Claude, "researcher", AgentStatus::Running);
    assert_eq!(roll_up_status(root.id, &[root, child, grandchild]).unwrap(), RollupStatus::NeedsAttention);
}

#[test]
fn completed_run_cannot_return_to_running() {
    let mut run = ProviderRun::new(ConversationId::new(), ProviderId::Codex);
    run.transition(RunStatus::Running).unwrap();
    run.transition(RunStatus::Completed).unwrap();
    assert!(matches!(run.transition(RunStatus::Running), Err(DomainError::InvalidTransition { .. })));
}
```

Run `cargo test -p prompting-time-core domain::tests`. Expected: compile failure because the domain types are absent.

- [ ] **Step 2: Implement explicit domain types and transitions**

Use UUIDv7 newtypes generated by one macro local to `domain.rs`. Define exhaustive statuses:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus { Queued, Running, Waiting, Completed, Interrupted, Failed }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus { Queued, Running, Waiting, Completed, Interrupted, Failed }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MutationState { NoneObserved, Observed, Unknown }
```

Implement transitions with direct `match (from, to)` control flow. Validate parent/run consistency, reject cycles while rolling up status, and apply precedence `NeedsAttention > Active > Failed > Interrupted > Completed` to descendants.

- [ ] **Step 3: Add serialization and cycle regression tests**

Test camelCase JSON, stable provider labels, missing parents, and a two-node cycle. Expected: malformed trees return `DomainError`, never recurse indefinitely.

- [ ] **Step 4: Verify and commit**

Run `cargo test -p prompting-time-core domain::tests && cargo clippy --workspace --all-targets -- -D warnings`.

```bash
git add crates/prompting-time-core/src
git commit -m "feat: add canonical conversation lifecycle"
```

---

### Task 3: Add transactional SQLite persistence and pagination

**Files:**
- Modify: `crates/prompting-time-core/Cargo.toml`
- Create: `crates/prompting-time-core/migrations/0001_initial.sql`
- Create: `crates/prompting-time-core/src/store.rs`
- Modify: `crates/prompting-time-core/src/lib.rs`
- Create: `crates/prompting-time-core/tests/recovery.rs`

**Interfaces:**
- Produces: `Store::open(path: &Path) -> Result<Store, StoreError>`
- Produces: `create_conversation`, `create_run`, `append_run_event`, `list_conversations`, `load_timeline`, `pending_recovery`
- Consumes: canonical types from Task 2

- [ ] **Step 1: Write failing storage tests**

```rust
#[tokio::test]
async fn event_and_run_state_commit_atomically() {
    let store = Store::open_in_memory().await.unwrap();
    let conversation = store.create_conversation(NewConversation::projectless("Test")).await.unwrap();
    let (run, root) = store.create_run(conversation.id, ProviderId::Codex).await.unwrap();
    store.append_run_event(run.id, root.id, ProviderEventRecord::started()).await.unwrap();

    let recovered = store.pending_recovery().await.unwrap();
    assert_eq!(recovered[0].run.status, RunStatus::Running);
    assert_eq!(recovered[0].events.len(), 1);
}

#[tokio::test]
async fn timeline_uses_stable_cursor_pagination() {
    let store = seeded_store_with_events(125).await;
    let first = store.load_timeline(conversation_id(), None, 50).await.unwrap();
    let second = store.load_timeline(conversation_id(), first.next_cursor, 50).await.unwrap();
    assert_eq!(first.items.len(), 50);
    assert_eq!(second.items.len(), 50);
    assert!(first.items.last().unwrap().sequence < second.items.first().unwrap().sequence);
}
```

Run `cargo test -p prompting-time-core --test recovery`. Expected: compile failure because `Store` is absent.

- [ ] **Step 2: Add SQLx and the initial migration**

Add SQLx 0.9 with `runtime-tokio`, `sqlite-bundled`, `migrate`, `macros`, `uuid`, and `time`. The migration creates `workspaces`, `conversations`, `provider_sessions`, `provider_runs`, `agent_nodes`, `messages`, `events`, `approvals`, and `routing_decisions` with foreign keys and `ON DELETE` behavior matching ownership. Use integer `sequence` keys for cursor pagination and JSON text columns only for content/payloads whose shape is validated at the Rust boundary.

Required indexes:

```sql
CREATE INDEX idx_conversations_status_updated ON conversations(status, updated_at DESC, id DESC);
CREATE INDEX idx_conversations_workspace_updated ON conversations(workspace_id, updated_at DESC, id DESC);
CREATE INDEX idx_runs_conversation_created ON provider_runs(conversation_id, created_at, id);
CREATE INDEX idx_agents_run_parent ON agent_nodes(run_id, parent_id);
CREATE INDEX idx_events_conversation_sequence ON events(conversation_id, sequence);
CREATE INDEX idx_approvals_pending ON approvals(status, created_at) WHERE status = 'pending';
```

- [ ] **Step 3: Implement `Store` with database-enforced initialization**

`Store::open` creates the parent directory, configures `foreign_keys=ON`, `journal_mode=WAL`, `busy_timeout=5000`, and a bounded pool, then runs embedded migrations. `append_run_event` starts one transaction, inserts the event, applies the legal run/agent transition, updates the conversation timestamp, and commits. No caller may update lifecycle columns with arbitrary SQL.

Reject page limits outside `1..=200`. Encode cursors as opaque URL-safe base64 JSON containing `(sequence, id)`; invalid cursors return `StoreError::InvalidCursor`.

- [ ] **Step 4: Test restart recovery and concurrent writers**

Use a `tempfile::TempDir`, close and reopen the pool, and verify queued/running/waiting runs are returned while terminal runs are not. Spawn 16 tasks appending independent events and assert all sequences are present exactly once.

- [ ] **Step 5: Verify and commit**

Run:

```bash
cargo fmt --all --check
cargo test -p prompting-time-core store
cargo test -p prompting-time-core --test recovery
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
git add Cargo.toml Cargo.lock crates/prompting-time-core
git commit -m "feat: persist conversations transactionally"
```

---

### Task 4: Implement safe project and worktree ownership

**Files:**
- Create: `crates/prompting-time-core/src/workspace.rs`
- Modify: `crates/prompting-time-core/src/lib.rs`

**Interfaces:**
- Produces: `WorkspaceManager::prepare(WorkspaceRequest) -> Result<WorkspaceLease, WorkspaceError>`
- Produces: `WorkspaceManager::cleanup_eligibility(&WorkspaceLease) -> Result<CleanupEligibility, WorkspaceError>`
- Produces: `WorkspaceManager::remove_owned(&WorkspaceLease) -> Result<(), WorkspaceError>`

- [ ] **Step 1: Write failing worktree safety tests**

```rust
#[tokio::test]
async fn dirty_owned_worktree_is_not_removable() {
    let repo = TestRepository::new().await;
    let manager = WorkspaceManager::new(repo.app_data_dir());
    let lease = manager.prepare(WorkspaceRequest::isolated(repo.path())).await.unwrap();
    tokio::fs::write(lease.path.join("untracked.txt"), "keep me").await.unwrap();
    assert_eq!(manager.cleanup_eligibility(&lease).await.unwrap(), CleanupEligibility::Blocked(WorkspaceBlocker::UntrackedFiles));
}

#[tokio::test]
async fn projectless_workspace_has_no_worktree() {
    let app_data = tempdir().unwrap();
    let lease = WorkspaceManager::new(app_data.path()).prepare(WorkspaceRequest::projectless(ConversationId::new())).await.unwrap();
    assert!(lease.project_root.is_none());
    assert!(!lease.owned_worktree);
    assert!(lease.path.starts_with(app_data.path().join("scratch")));
}
```

Run `cargo test -p prompting-time-core workspace::tests`. Expected: compile failure.

- [ ] **Step 2: Implement direct Git command handling**

Resolve and canonicalize the selected path. Detect Git with `git -C <path> rev-parse --show-toplevel`. For isolated mode, derive an opaque repository ID and conversation ID path under Application Support, create branch `prompting-time/<conversation-id>`, and invoke `git worktree add -b`. For a projectless conversation, create an app-owned `scratch/<conversation-id>` execution directory but expose no project in the UI; retain that directory if it contains user-visible artifacts. Capture stdout/stderr separately and return typed errors containing the attempted operation but not file contents.

Use explicit commands for cleanup checks: `git status --porcelain=v2`, `git log <base>..<branch> --oneline`, `git worktree list --porcelain`, and an owned-process query from the runtime. `remove_owned` rechecks eligibility immediately before `git worktree remove` and refuses unknown ownership.

- [ ] **Step 3: Cover blockers independently**

Add tests for modified tracked files, untracked files, unique commits, an active process, a missing worktree, a non-owned worktree, and a clean owned worktree. Tests may remove only paths created under their `TempDir`.

- [ ] **Step 4: Verify and commit**

Run `cargo test -p prompting-time-core workspace::tests && cargo clippy --workspace --all-targets -- -D warnings`.

```bash
git add crates/prompting-time-core/src/workspace.rs crates/prompting-time-core/src/lib.rs
git commit -m "feat: manage isolated worktrees safely"
```

---

### Task 5: Add the explainable automatic router

**Files:**
- Create: `crates/prompting-time-core/src/router.rs`
- Modify: `crates/prompting-time-core/src/lib.rs`

**Interfaces:**
- Produces: `Router::route(RouteRequest) -> Result<RoutingDecision, RoutingError>`
- Consumes: `ProviderId`, provider capabilities/health, current provider, routing profile, usage counters, and optional override

- [ ] **Step 1: Write routing precedence tests**

```rust
#[test]
fn override_beats_continuity_and_balance() {
    let request = RouteRequest::builder("continue the implementation")
        .override_provider(ProviderId::Claude)
        .current_provider(ProviderId::Codex)
        .eligible([healthy(ProviderId::Codex), healthy(ProviderId::Claude)])
        .usage([(ProviderId::Codex, 0), (ProviderId::Claude, 10)])
        .build();
    let decision = Router::default().route(request).unwrap();
    assert_eq!(decision.provider, ProviderId::Claude);
    assert_eq!(decision.reason, RoutingReason::ManualOverride);
}

#[test]
fn unavailable_override_fails_instead_of_silently_switching() {
    let request = request_with_override(ProviderId::Claude, [healthy(ProviderId::Codex), unavailable(ProviderId::Claude)]);
    assert!(matches!(Router::default().route(request), Err(RoutingError::RequestedProviderUnavailable { provider: ProviderId::Claude, .. })));
}
```

Also test continuity, capability requirements, least-used balancing, deterministic tie-breaking, profiles, and no eligible providers. Run `cargo test -p prompting-time-core router::tests`; expect compile failure.

- [ ] **Step 2: Implement deterministic scoring without unvalidated quality claims**

Filter eligibility first. A manual override selects only that provider and reports why it cannot run instead of overriding the user. Next require declared capabilities. Apply continuity unless the message explicitly requests a different provider. For remaining ties, `UsageBalance` selects the lowest recent root-run count; `Balanced` gives continuity priority then balances; `BestFit` uses required capabilities then continuity and uses balance only as a tie-breaker.

Classify task signals locally for observability, but do not hard-code claims that Codex or Claude is intrinsically better at a task without evaluation evidence. Store `TaskKind` and the score breakdown in `RoutingDecision`.

- [ ] **Step 3: Add property tests for eligibility invariants**

Use `proptest` to generate health/capability combinations and assert the router never chooses an ineligible provider, is deterministic for identical inputs, and always honors an eligible override.

- [ ] **Step 4: Verify and commit**

Run `cargo test -p prompting-time-core router && cargo clippy --workspace --all-targets -- -D warnings`.

```bash
git add Cargo.toml Cargo.lock crates/prompting-time-core/src/router.rs crates/prompting-time-core/src/lib.rs
git commit -m "feat: route work with explainable decisions"
```

---

### Task 6: Build the provider contract and owned run supervisor

**Files:**
- Modify: `crates/prompting-time-core/src/providers/mod.rs`
- Create: `crates/prompting-time-core/src/providers/process.rs`
- Create: `crates/prompting-time-core/src/runtime.rs`
- Create: `crates/prompting-time-core/tests/adapter_contract.rs`

**Interfaces:**
- Produces: object-safe `ProviderAdapter`
- Produces: `ProviderEvent`, owned `ProviderTurn`, `ProviderSession`, `TurnRequest`, `ApprovalResponse`, `ProviderCapabilities`
- Produces: `RunSupervisor::submit(RunRequest) -> Result<RunHandle, RuntimeError>` and `shutdown()`
- Consumes: `Store` and canonical domain types

- [ ] **Step 1: Write the shared adapter contract against a fake adapter**

```rust
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn health(&self) -> Result<ProviderHealth, ProviderError>;
    async fn start_session(&self, request: StartSession) -> Result<ProviderSession, ProviderError>;
    async fn resume_session(&self, native_id: &str, request: ResumeSession) -> Result<ProviderSession, ProviderError>;
    async fn start_turn(&self, session: &ProviderSession, request: TurnRequest) -> Result<ProviderTurn, ProviderError>;
    async fn steer(&self, session: &ProviderSession, active_turn: &str, text: &str) -> Result<(), ProviderError>;
    async fn respond(&self, session: &ProviderSession, request_id: &str, response: ApprovalResponse) -> Result<(), ProviderError>;
    async fn interrupt(&self, session: &ProviderSession, active_turn: &str) -> Result<(), ProviderError>;
}
```

`ProviderTurn` couples the bounded event receiver to an object-safe owner whose async shutdown stops and awaits every process or request owned by the turn. The contract suite verifies ordered events, actual stream closure after exactly one terminal event, approval pause/resume, interruption, provider-native ID retention, stream-error propagation, and owned resource shutdown. Run it first and expect failure because the fake adapter and supervisor are absent.

- [ ] **Step 2: Implement a bounded JSON-lines subprocess transport**

`JsonLineProcess` owns `Child`, stdin, stdout reader, stderr reader, a bounded event channel, and cancellation token. It must kill and await its child on shutdown/drop through an explicit owner task. Limit individual lines to 8 MiB, cap buffered events at 256, and surface malformed JSON and oversized frames as typed protocol errors.

Test with a helper process that emits valid, delayed, malformed, and oversized lines. Never use real provider binaries in hermetic tests.

- [ ] **Step 3: Implement `RunSupervisor` with bounded root concurrency**

Use a `Semaphore` initialized to four and an owned `JoinSet`. `submit` persists the queued run before spawning. The task acquires a permit, starts/resumes the provider session, persists each normalized event transactionally, and records a terminal state exactly once. Cancellation tokens are indexed by run ID and removed when the owned task joins.

Implement mutation tracking from normalized tool events. Automatic fallback is permitted once only when both `MutationState::NoneObserved` and durable `DispatchCertainty::NotDispatched` prove it safe; `Observed`, `Unknown`, and ambiguous dispatch all require user action. Approval responses persist exact intent before provider dispatch and acknowledge that intent transactionally before buffered provider events resume.

- [ ] **Step 4: Test queueing, cancellation, crash, and fallback**

Start five blocking fake runs. Assert four become running, one remains queued, releasing one starts the fifth, interrupting cancels the right child, and a stream crash after a mutating event never starts the fallback adapter.

- [ ] **Step 5: Verify and commit**

Run:

```bash
cargo test -p prompting-time-core --test adapter_contract
cargo test -p prompting-time-core runtime
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
git add Cargo.toml Cargo.lock crates/prompting-time-core
git commit -m "feat: supervise provider runs"
```

---

### Task 7: Implement the Codex App Server adapter

**Files:**
- Create: `crates/prompting-time-core/src/providers/codex.rs`
- Create: `crates/prompting-time-core/tests/fixtures/codex/session.jsonl`
- Modify: `crates/prompting-time-core/src/providers/mod.rs`
- Extend: `crates/prompting-time-core/tests/adapter_contract.rs`

**Interfaces:**
- Produces: `CodexAdapter::connect(binary: PathBuf) -> Result<CodexAdapter, ProviderError>`
- Implements: `ProviderAdapter`
- Consumes: `JsonLineProcess`

- [ ] **Step 1: Capture and sanitize the supported Codex schema and fixture**

Run:

```bash
probe_dir=$(mktemp -d)
codex app-server generate-json-schema --out "$probe_dir"
rg -n 'thread/start|thread/resume|turn/start|turn/steer|turn/interrupt|skills/list|requestUserInput' "$probe_dir"
```

Expected: all required methods appear. Do not commit the full generated schema. In Codex CLI 0.144.1, child relationships are carried by the `collabAgentToolCall` item variant, typed `subAgentActivity` items carry `agentThreadId`, `agentPath`, and activity kind, and permission-profile approval is a distinct `item/permissions/requestApproval` server request. Build a small, fully typed-schema-valid fixture containing initialize/initialized, thread start, turn start, agent delta, child-agent relationship and activity items, each supported approval shape and resolution, and turn completion. Keep an invented unknown notification in a separate generic-wire forward-compatibility fixture because it cannot satisfy the closed typed notification union. Replace real paths, IDs, messages, and account data with invented values before staging, then run the fixture through the real parser and dispatcher behavior. An ignored opt-in test must generate the installed 0.144.1 schemas locally and validate the committed recording without committing the schema bundle.

- [ ] **Step 2: Write adapter parsing and request-correlation tests**

Feed the fixture through an in-memory transport. Assert typed JSON-RPC response IDs resolve the correct pending request without string/number collisions, late and duplicate responses cannot poison active turns, notifications normalize only for the matching active thread and turn, unknown notifications are retained as `ProviderEvent::Unrecognized` without failing the stream, and request errors preserve code/message without provider payload leakage. Abandoned request state and late-response tombstones must remain bounded.

- [ ] **Step 3: Implement handshake, session, turn, and approval methods**

On connection, send `initialize` with `Prompting Time` client metadata, wait for success, then send `initialized`. Maintain a monotonic request ID and `HashMap<RequestId, oneshot::Sender<_>>` owned by the reader task. Map:

- `start_session` → `thread/start`
- `resume_session` → `thread/resume`
- `start_turn` → `turn/start`
- `steer` → `turn/steer`
- `interrupt` → `turn/interrupt`
- `respond` → the matching server-request response

Pass the recorded workspace directory and explicit permission policy; never use danger-full-access or bypass approvals by default. Persist `thread.id`, `thread.sessionId`, active turn ID, and native item IDs. Aggregate assistant deltas by native item ID through normal ingestion, approval staging/drain, and restart recovery.

Coordinate each provisional turn registration, owner, cleanup guard, and `turn/start` request with one unique generation and explicit queued, writing, awaiting-response, caller-consumed, and abandoned phases. Only caller consumption finishes a request. Cancelling or capacity-rejecting a queued request unregisters only that exact undispatched generation, while an ambiguous written request is interrupted or fails closed; dropping a response-ready start still cleans up its exact generation. When a start response activates after both caller and event receivers close, retain that exact cancelling generation until interrupt confirmation or process teardown and reject any replacement while the fatal interrupt remains pending. Never replace an existing registration merely because its receiver closed. Validate explicit interrupt IDs and owner generations before mutation, mark a turn cancelling only after the native interrupt write is admitted, and coalesce concurrent interrupt callers onto one native confirmation. Capacity rejection leaves direct interruption retryable. Terminal completion may internally confirm and tombstone an admitted interrupt before its late response, but it must not resolve primary or coalesced callers or release ownership until writer-acknowledged rejection of every outstanding server request succeeds; cleanup failure fails the callers and tears down the connection. Dropping the direct caller after write does not abandon confirmation. If natural terminal completion wins after owner shutdown is queued, reconcile an exact `NotDispatched` cancellation with the shared completed flag and complete shutdown without killing the healthy shared process. Seal provisional output at its first terminal event, and reserve bounded stream capacity so exactly one terminal remains observable to a slow consumer without blocking the shared dispatcher. Normalize and durably persist only the typed approval details required for authorization: command and working directory; correlated file change paths, kinds, move targets, grant root, and reason without patch contents; and the exact requested filesystem/network permission profile.

Treat every server request as requiring a bounded response write acknowledged by the owned stdin writer before success or fatal teardown. Classify envelopes by method and raw ID before payload parsing. Recognized requests with missing, null, non-scalar, non-integral, or out-of-range IDs receive exactly one `id: null` invalid-request response and emit no event; malformed response IDs are connection-fatal. Keep bounded server-request tombstones separate from client-response correlation. Detect a duplicate outstanding request before unsupported-method, ownership, cancellation, or provisional-terminal admission; atomically remove the original, send one writer-acknowledged error, tombstone it, and close the connection before cleanup or user response can send a second frame. Reuse after a completed response closes without a second response. Validate nonempty thread and turn IDs against the active owner before retaining a request, reject missing or stale owners explicitly, preflight provisional capacity before insertion, reject new requests after cancellation begins, and reject every outstanding request before confirmed interruption or terminal completion releases ownership. Parse malformed recognized requests into an explicit `-32602` response before any fatal teardown. Before UI admission, require every field mandated by Codex 0.144.1: command item ID and signed 64-bit start timestamp; file item ID, timestamp, and observed same-owner correlation; user-input item ID; and permission item ID, timestamp, working directory, and closed typed profile. Enforce schema numeric bounds, including permission glob scan depth of at least one. Verify every user response belongs to the supplied session and active turn, and reject it without writing once the turn is cancelled or interrupt-pending so cancellation cleanup remains the sole responder. Events and requests arriving after `turn/started` but before the correlated `turn/start` response must be retained in a bounded provisional state, including fast completion; the response must confirm the announced turn ID, and a provisional terminal must suppress requests already rejected during cleanup. Preserve every `requestUserInput` question, header, prompt, option, other/secret flag, and per-question answer list; reject scalar or incomplete answer shapes rather than duplicating one answer. Permission-profile approval is parsed into a closed typed model and may return only the exact displayed profile when approved and an empty profile when denied; unknown nested permission fields fail closed. Collaboration events require their sender to match the outer thread, require nonempty tool and status values, and allow receiver IDs without states while rejecting states for unlisted receivers. Unsupported server requests fail explicitly instead of remaining pending. A slow or abandoned turn consumer must not block request correlation, approvals, or unrelated sessions on the shared app-server process. Use the owned turn as the supervisor's single interruption authority, retain direct adapter interruption for other callers, and share confirmation so neither path duplicates the native request. A failed completion is terminal cleanup despite surfacing as a provider error. Dispatcher or process death, malformed recognized notification ownership, an unconfirmed interrupt deadline, ambiguous pre-ID cancellation, a blocked abandoned write, a malformed or mismatched turn-start success, or failure to write any required server-request response makes health unavailable and kills and reaps the process so Codex cannot continue mutating after Prompting Time has failed the run. Cancelling a request that is still queued must not terminate a responsive shared connection.

- [ ] **Step 4: Run the shared contract and opt-in live smoke**

Run hermetic tests first. Then, from a new temporary Git repository with no private files, run the ignored test with `PROMPTING_TIME_LIVE_CODEX=1`. The prompt must request exactly `Reply READY without using tools.` Assert a streamed assistant message and completed turn, then archive/delete only the test session if the adapter created it ephemerally.

Expected: contract tests pass; the live result is reported separately and does not gate public CI.

- [ ] **Step 5: Commit**

```bash
git add crates/prompting-time-core/src/providers crates/prompting-time-core/tests
git commit -m "feat: integrate Codex app server"
```

---

### Task 8: Validate and implement the Claude Code adapter

**Files:**
- Create: `crates/prompting-time-core/src/providers/claude.rs`
- Create: `crates/prompting-time-core/tests/fixtures/claude/session.jsonl`
- Create: `crates/prompting-time-core/tests/claude_protocol.rs`
- Modify: `crates/prompting-time-core/src/providers/mod.rs`
- Extend: `crates/prompting-time-core/tests/adapter_contract.rs`

**Interfaces:**
- Produces: `ClaudeAdapter::new(binary: PathBuf, runtime_dir: PathBuf) -> ClaudeAdapter`
- Implements: `ProviderAdapter`
- Consumes: `JsonLineProcess`
- Produces for ignored tests only: `LiveClaudeProbe::{spawn, send, begin, wait_for_assistant_delta, interrupt, resume, defer_next_approval, deny_deferred, finish}`

- [ ] **Step 1: Add ignored protocol experiments before adapter code**

Implement a test-only `LiveClaudeProbe` around an owned `JsonLineProcess`. It rejects execution unless `PROMPTING_TIME_LIVE_CLAUDE=1`, always uses a fresh `TempDir`, records parsed events, and kills/awaits the child in `finish` and `Drop`. Then add these four ignored tests:

```rust
#[tokio::test]
#[ignore = "uses the installed Claude account"]
async fn live_stream_accepts_two_turns_on_one_session() {
    let mut probe = LiveClaudeProbe::spawn(PermissionMode::DontAsk).await.unwrap();
    let first = probe.send("Reply with ONE and do not use tools.").await.unwrap();
    let second = probe.send("Reply with TWO and do not use tools.").await.unwrap();
    assert_eq!(first.session_id, second.session_id);
    assert_eq!(first.final_text.trim(), "ONE");
    assert_eq!(second.final_text.trim(), "TWO");
    probe.finish().await.unwrap();
}

#[tokio::test]
#[ignore = "uses the installed Claude account"]
async fn live_deferred_approval_can_resume() {
    let mut probe = LiveClaudeProbe::spawn(PermissionMode::Manual).await.unwrap();
    probe.defer_next_approval();
    let deferred = probe.send("Create approval-probe.txt containing PROBE.").await.unwrap();
    assert!(deferred.pending_approval.is_some());
    assert!(!probe.cwd().join("approval-probe.txt").exists());
    let resumed = probe.deny_deferred(deferred.pending_approval.unwrap()).await.unwrap();
    assert_eq!(resumed.session_id, deferred.session_id);
    assert!(!probe.cwd().join("approval-probe.txt").exists());
    probe.finish().await.unwrap();
}

#[tokio::test]
#[ignore = "uses the installed Claude account"]
async fn live_interrupt_preserves_resumable_session() {
    let mut probe = LiveClaudeProbe::spawn(PermissionMode::DontAsk).await.unwrap();
    let active = probe.begin("List the integers from 1 through 200, one per line, without tools.").await.unwrap();
    probe.wait_for_assistant_delta(&active).await.unwrap();
    let session_id = probe.interrupt(active).await.unwrap();
    let resumed = probe.resume(&session_id, "Reply RESUMED without tools.").await.unwrap();
    assert_eq!(resumed.session_id, session_id);
    assert_eq!(resumed.final_text.trim(), "RESUMED");
    probe.finish().await.unwrap();
}

#[tokio::test]
#[ignore = "uses the installed Claude account"]
async fn live_child_agent_events_have_stable_identity() {
    let mut probe = LiveClaudeProbe::spawn(PermissionMode::DontAsk).await.unwrap();
    let result = probe.send("Ask exactly one subagent to reply CHILD. Do not use other tools.").await.unwrap();
    let starts = result.child_agent_starts();
    let stops = result.child_agent_stops();
    assert_eq!(starts.len(), 1);
    assert_eq!(stops.len(), 1);
    assert_eq!(starts[0].native_agent_id, stops[0].native_agent_id);
    probe.finish().await.unwrap();
}
```

Use only a fresh `TempDir`, an invented prompt, and a restrictive permission mode. Run each independently and save sanitized protocol fixtures.

- [ ] **Step 2: Enforce the protocol decision gate**

Run:

```bash
PROMPTING_TIME_LIVE_CLAUDE=1 cargo test -p prompting-time-core --test claude_protocol -- --ignored --nocapture --test-threads=1
```

Expected: long-lived input, deferred approval/resume, interruption/resume, and child identity are demonstrated with the installed Claude Code version. If any required behavior is unavailable, stop implementation, record the observed event/exit behavior in `docs/provider-protocols.md`, and revise this plan to place a narrowly scoped Claude Agent SDK sidecar behind `ProviderAdapter`. Do not build terminal scraping or bypass approvals as a workaround.

- [ ] **Step 3: Write hermetic fixture and adapter-contract tests**

Use the sanitized fixture to verify system/init messages, assistant deltas, tool starts/results, permission requests, `AskUserQuestion`, subagent start/stop, result usage metadata, retry notices, and terminal success/error events. Unknown fields must be tolerated; missing required session/turn identity must fail clearly.

- [ ] **Step 4: Implement Claude process lifecycle**

Start Claude with `--print --input-format stream-json --output-format stream-json --verbose --include-partial-messages`, an explicit UUID session ID, the recorded workspace, and a restrictive permission mode. Use the validated defer/resume mechanism for approvals and `--resume <session-id>` after process exits. Pass prompts through stdin as structured messages; never interpolate prompts into shell command strings.

Map provider events into the same normalized event set as Codex while retaining native JSON. Report unsupported active steering as a capability difference rather than simulating it.

- [ ] **Step 5: Run the shared contract and commit**

Run:

```bash
cargo test -p prompting-time-core --test adapter_contract claude
cargo test -p prompting-time-core providers::claude
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
git add crates/prompting-time-core/src/providers crates/prompting-time-core/tests
git commit -m "feat: integrate Claude Code sessions"
```

If the protocol gate fails, this task ends before adapter implementation or commit. The evidence and revised sidecar plan are reviewed as a separately scoped change.

---

### Task 9: Add bounded handoff and conversation orchestration

**Files:**
- Create: `crates/prompting-time-core/src/handoff.rs`
- Create: `crates/prompting-time-core/src/app.rs`
- Modify: `crates/prompting-time-core/src/lib.rs`
- Create: `crates/prompting-time-core/tests/provider_switch.rs`

**Interfaces:**
- Produces: `HandoffBuilder::build(HandoffInput) -> HandoffCapsule`
- Produces: `PromptingTime::create_conversation`, `submit`, `steer`, `respond_to_approval`, `interrupt`, `archive`
- Consumes: `Store`, `Router`, `WorkspaceManager`, `RunSupervisor`, and provider registry

- [ ] **Step 1: Write failing handoff budget and switching tests**

```rust
#[test]
fn handoff_keeps_objective_constraints_and_newest_messages_within_budget() {
    let capsule = HandoffBuilder::new(32_000).build(long_handoff_input()).unwrap();
    assert!(capsule.rendered.chars().count() <= 32_000);
    assert!(capsule.rendered.contains("Current objective"));
    assert!(capsule.rendered.contains("Do not change the public API"));
    assert!(capsule.rendered.contains("newest user message"));
    assert!(!capsule.rendered.contains("hidden_reasoning"));
}

#[tokio::test]
async fn switching_back_resumes_provider_and_sends_only_unseen_context() {
    let codex = Arc::new(FakeAdapter::new(ProviderId::Codex));
    let claude = Arc::new(FakeAdapter::new(ProviderId::Claude));
    let app = test_app(codex.clone(), claude).await;
    let conversation = app.create_conversation(projectless()).await.unwrap();
    app.submit(message(conversation.id, "first").with_provider(ProviderId::Codex)).await.unwrap();
    app.submit(message(conversation.id, "second").with_provider(ProviderId::Claude)).await.unwrap();
    app.submit(message(conversation.id, "third").with_provider(ProviderId::Codex)).await.unwrap();
    assert_eq!(codex.resume_count(), 1);
    assert_eq!(codex.last_handoff().messages, vec!["second"]);
}
```

Run the tests and expect failure because `HandoffBuilder` and `PromptingTime` do not exist.

- [ ] **Step 2: Implement deterministic capsule construction**

Render labeled sections for objective, current request, constraints, decisions, child-agent outcomes, workspace state, and recent visible messages. Reserve budget for objective/current request/constraints first, then add newest messages in reverse and restore chronological order. Exclude provider raw payloads, hidden reasoning fields, diagnostic logs, and detailed successful tool output. Include unresolved failure output only after redaction.

Store the rendered capsule and a content hash on the provider run so the inspector can show exactly what was supplied.

- [ ] **Step 3: Implement application use cases as the only orchestration entrypoint**

`PromptingTime::submit` validates conversation state, rejects a second normal turn while one is active, routes the request, creates or resumes the provider session, builds unseen handoff context when the provider changes, persists the user message and routing decision, and asks `RunSupervisor` to execute. Do not let Tauri commands coordinate these steps themselves.

During durable event ingestion, materialize typed child-agent relationships into `AgentNode` rows using provider-native thread IDs scoped to the recorded provider session tree. Repeated child updates change the existing node's typed status; conflicting parentage or identity fails closed and remains diagnostic. Task 7 preserves these relationships and statuses durably, but this application-layer materialization is intentionally completed here so the UI never infers agent identity from generic tool text.

`respond_to_approval` verifies the approval is still pending and belongs to the active native request. `archive` stops active work or refuses with an actionable conflict, and records archive state without deleting a worktree.

- [ ] **Step 4: Test switch, restart, failure, and duplicate-command idempotency**

Cover small full-transcript handoff, large compacted handoff, return to a native session, app restart between providers, stale approval responses, duplicate submit command IDs, and fallback before/after mutation.

- [ ] **Step 5: Verify and commit**

Run `cargo test -p prompting-time-core --test provider_switch && cargo test -p prompting-time-core app && cargo clippy --workspace --all-targets -- -D warnings`.

```bash
git add crates/prompting-time-core
git commit -m "feat: orchestrate routed provider conversations"
```

---

### Task 10: Expose a typed Tauri command and event boundary

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/{lib.rs,commands.rs,state.rs}`
- Create: `src/bridge/types.ts`
- Modify: `src/bridge/api.ts`
- Create: `src/bridge/api.test.ts`

**Interfaces:**
- Produces commands: `bootstrap`, `list_conversations`, `load_timeline`, `create_conversation`, `submit_message`, `steer_run`, `respond_to_approval`, `interrupt_run`, `archive_conversation`, `inspect_workspace`
- Produces event: `prompting-time://app-event` carrying `AppEvent`
- Consumes: `PromptingTime` application service

- [ ] **Step 1: Write failing bridge contract tests**

Mock `@tauri-apps/api/core.invoke` and `@tauri-apps/api/event.listen`. Verify exact command names, camelCase arguments, error decoding, unsubscribe behavior, and that the bridge never exposes provider-native payloads in ordinary snapshots.

```ts
it("submits a message with an idempotency key", async () => {
  invoke.mockResolvedValue({ runId: "run-1", status: "queued" });
  await submitMessage({ conversationId: "c-1", text: "hello", providerOverride: null, commandId: "cmd-1" });
  expect(invoke).toHaveBeenCalledWith("submit_message", { request: { conversationId: "c-1", text: "hello", providerOverride: null, commandId: "cmd-1" } });
});
```

- [ ] **Step 2: Implement `AppState` initialization**

At Tauri setup, resolve `app.path().app_data_dir()`, open the store, build workspace/router/provider services, reconcile recovery state, and manage one `Arc<AppState>`. If initialization fails, preserve a bootstrap diagnostic screen rather than panic. On window/app exit, call `RunSupervisor::shutdown` and await owned children with a bounded timeout.

- [ ] **Step 3: Implement thin commands and sanitized errors**

Each command deserializes a request, calls exactly one application-service method, and maps `AppError` to:

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
    pub action: Option<String>,
}
```

Emit `AppEvent` only after the corresponding durable transaction commits. Use commands for request/response and one event channel for incremental changes; do not send large timelines through global events.

- [ ] **Step 4: Generate/check TypeScript domain types from Rust**

Use `specta` and `tauri-specta` only for boundary DTOs and commands, generating `src/bridge/types.ts` during development. Add a test that regenerates to a temp file and byte-compares it with the committed binding, so Rust remains authoritative and CI detects drift.

- [ ] **Step 5: Verify and commit**

Run:

```bash
cargo test --workspace
pnpm test -- src/bridge/api.test.ts
pnpm build
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
git add Cargo.toml Cargo.lock src-tauri src/bridge package.json pnpm-lock.yaml
git commit -m "feat: expose typed desktop application API"
```

---

### Task 11: Build the command-center shell and recursive conversation tree

**Files:**
- Create: `src/app/store.ts`
- Modify: `src/app/App.tsx`
- Create: `src/features/conversations/ConversationTree.tsx`
- Create: `src/features/conversations/ConversationTree.test.tsx`
- Create: `src/styles/tokens.css`, `src/styles/app.css`
- Modify: `src/main.tsx`

**Interfaces:**
- Produces: `createAppStore(api)`, `useAppStore(selector)`, `ConversationTree`
- Consumes: typed bridge from Task 10

- [ ] **Step 1: Write failing store and recursive-tree tests**

```tsx
it("renders an orchestrating grandchild beneath its parent", () => {
  render(<ConversationTree conversations={[conversationWithThreeLevels()]} selectedId="c1" onSelect={vi.fn()} />);
  expect(screen.getByRole("treeitem", { name: /Auth refactor/ })).toHaveAttribute("aria-level", "1");
  expect(screen.getByRole("treeitem", { name: /API reviewer/ })).toHaveAttribute("aria-level", "2");
  expect(screen.getByRole("treeitem", { name: /Schema researcher/ })).toHaveAttribute("aria-level", "3");
});
```

Test immutable event reduction, status filtering, project grouping, queued count, selection, expand/collapse, keyboard traversal, and status rollup labels.

- [ ] **Step 2: Implement a small external store around `useSyncExternalStore`**

The store owns normalized UI snapshots keyed by IDs, subscribes once to `prompting-time://app-event`, applies sequence-checked events, and reloads a snapshot if a sequence gap occurs. It exposes selectors; components do not call Tauri directly. Avoid adding Redux/Zustand until behavior proves the need.

- [ ] **Step 3: Implement the accessible left pane**

Use semantic tree roles, roving keyboard focus, buttons for expansion, text labels in addition to color, and virtualize only after a measured threshold. Group roots by project and projectless status; render descendants recursively with an indentation guide and provider/status badges.

- [ ] **Step 4: Implement responsive layout tokens**

Define neutral dark theme tokens, a 255px resizable left pane, a flexible center pane, and a 285px collapsible inspector. At narrow window widths the inspector becomes an overlay; the conversation tree remains accessible from a toolbar button. Respect reduced motion and system font scaling.

- [ ] **Step 5: Verify and commit**

Run `pnpm test -- src/app src/features/conversations && pnpm build && pnpm lint`.

```bash
git add src
git commit -m "feat: add recursive conversation command center"
```

---

### Task 12: Build timeline, composer, approvals, and inspector

**Files:**
- Create: `src/features/timeline/{Timeline.tsx,Timeline.test.tsx,Composer.tsx,Composer.test.tsx}`
- Create: `src/features/inspector/{Inspector.tsx,ApprovalCard.tsx,Inspector.test.tsx}`
- Modify: `src/app/App.tsx`, `src/app/store.ts`, `src/styles/app.css`

**Interfaces:**
- Produces: paginated `Timeline`, `Composer`, collapsible `Inspector`, `ApprovalCard`
- Consumes: bridge commands and store from Tasks 10–11

- [ ] **Step 1: Write failing behavior tests**

Cover provider-labeled messages, collapsed tool groups, recursive agent cards, cursor pagination, auto/manual provider selection, switch-disabled-during-active-turn behavior, interrupt-then-switch flow, approval allow/deny, stale approval state, inspector routing reason, handoff preview, workspace blockers, and focus restoration.

```tsx
it("requires interruption before switching an active provider", async () => {
  render(<Composer state={runningCodexConversation()} api={api} />);
  await user.selectOptions(screen.getByLabelText("Provider"), "claude");
  expect(screen.getByRole("dialog", { name: /Interrupt Codex/ })).toBeVisible();
  expect(api.submitMessage).not.toHaveBeenCalled();
});
```

- [ ] **Step 2: Implement timeline pagination and streaming updates**

Fetch the newest bounded page, prepend older pages on request, and preserve scroll position. Merge live events by durable ID/sequence. Render provider messages, progress, tools, approvals, questions, failures, and unrecognized provider activity with distinct accessible components. Large tool output remains collapsed and fetched only on demand.

- [ ] **Step 3: Implement composer and routing controls**

The composer supports Auto, Codex, and Claude; Auto displays the active profile. Generate one UUID command ID per send and reuse it on transport retry. While a run is active, the primary action becomes Steer only when the adapter reports steering support; otherwise it explains that the user must interrupt or wait.

- [ ] **Step 4: Implement approvals and inspector**

Approval cards show provider, requesting agent path, operation summary, scope, and exact choices. Disable buttons after one click and reconcile with durable state. The inspector shows provider reason/score inputs, workspace/worktree state, changed-file summary, active descendant count, handoff content, provider versions, and cleanup blockers. Collapse state is local UI preference.

- [ ] **Step 5: Run accessibility and behavior checks**

Use Testing Library queries by role/name and add `axe-core` assertions for the three-pane screen and approval dialog. Manually test keyboard-only tree navigation, focus after approval, reduced motion, and a 1280×720 window.

- [ ] **Step 6: Commit**

```bash
git add src package.json pnpm-lock.yaml
git commit -m "feat: add routed conversation workspace"
```

---

### Task 13: Add recovery, notifications, privacy enforcement, CI, and public delivery

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/{lib.rs,state.rs}`
- Create: `scripts/privacy-scan.sh`
- Create: `.github/workflows/ci.yml`
- Create: `README.md`
- Modify: `.gitignore`, `src-tauri/tauri.conf.json`
- Modify: `docs/superpowers/specs/2026-09-03-prompting-time-design.md`
- Delete: `docs/superpowers/plans/2026-09-03-prompting-time.md` only after every durable requirement is reconciled into the specification and README

**Interfaces:**
- Produces: restart reconciliation, actionable macOS notifications, privacy scan, CI, unsigned release app
- Consumes: all previous tasks

- [ ] **Step 1: Write failing recovery and notification tests**

Seed queued/running/waiting runs, restart `AppState`, and assert queued work is restored, resumable provider sessions are reconciled, ambiguous running work becomes interrupted, and notifications fire only for background completion/failure/attention transitions. Inject a fake notifier; do not abstract unrelated platform services.

- [ ] **Step 2: Implement restart reconciliation and owned shutdown**

During bootstrap, compare durable nonterminal runs with owned process/session state. Requeue work that never started, resume only when the provider confirms a resumable session, and mark ambiguous runs interrupted with an actionable diagnostic. On shutdown, cancel owned tasks, await them for five seconds, then kill/await remaining owned children and persist the final observed state.

- [ ] **Step 3: Implement macOS notifications**

Use Tauri's notification plugin. Notify only when the app is backgrounded and a root conversation completes, fails, or rolls into needs-attention. Notification bodies contain the conversation title and status but no prompt, response, tool, path, or file content.

- [ ] **Step 4: Add the repository privacy scanner**

Implement `scripts/privacy-scan.sh` with `set -euo pipefail`. It scans tracked files only using `git grep` for private absolute home paths, known credential formats, provider auth files, `.codex`, `.claude`, and forbidden runtime extensions. When `PROMPTING_TIME_PRIVATE_TERMS_FILE` points to a newline-delimited local file outside the repository, scan each nonblank literal as an additional deny term without printing it. CI runs the generic scan; the pre-publication local run supplies the private denylist. Tests create a temporary Git repository with one forbidden fixture and assert the scanner fails, then remove it and assert success.

- [ ] **Step 5: Add CI and documentation**

CI on macOS checks:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm install --frozen-lockfile
pnpm lint
pnpm test
pnpm build
pnpm tauri build --bundles app
bash scripts/privacy-scan.sh
```

README documents architecture, prerequisites, provider setup links, privacy boundary, development commands, unsigned app installation, limitations, live smoke opt-ins, and the Milestone 2 resource-library boundary. Do not claim live provider verification unless the corresponding smoke test passed in this implementation run.

- [ ] **Step 6: Run full verification and inspect the artifact**

Run all CI commands locally. Launch `Prompting Time.app` and exercise one projectless session, one isolated-worktree session, four concurrent roots plus one queued root, a recursive agent tree, one approval, interruption, provider switch, restart recovery, and safe archive behavior. Record hermetic results separately from live account-backed results.

- [ ] **Step 7: Perform final code and specification reconciliation**

Review the complete diff for duplicated models, provider leakage into UI/core policy, unowned tasks, unbounded queries/channels, unsafe worktree deletion, prompt-bearing logs, and stale documentation. Update the canonical specification to match observed final behavior. Remove dead scaffolding, obsolete fixtures, completed temporary brainstorming artifacts from tracked paths, and dependencies with no remaining use.

- [ ] **Step 8: Commit the release candidate**

```bash
git add .github .gitignore README.md scripts src src-tauri crates Cargo.toml Cargo.lock package.json pnpm-lock.yaml docs
git commit -m "chore: prepare Prompting Time public release"
```

- [ ] **Step 9: Publish the requested public repository after the privacy gate passes**

Run:

```bash
gh auth status
PROMPTING_TIME_PRIVATE_TERMS_FILE=/absolute/private/denylist.txt bash scripts/privacy-scan.sh
gh repo create prompting-time --public --source=. --remote=origin --push
gh repo view --web
```

Expected: the private denylist exists outside the repository and contains the user's company/project-specific terms; GitHub creates the public `prompting-time` repository under the authenticated user's account, pushes the verified branch, and opens the repository page. If a repository with that name already exists, stop and inspect ownership/remotes instead of overwriting or attaching to it automatically.

---

## Final verification matrix

| Boundary | Hermetic evidence | Live/visual evidence |
|---|---|---|
| Domain and recursion | Unit and property tests | Three-level agent tree visible |
| Persistence and recovery | Temp SQLite restart/concurrency tests | App restart preserves coherent state |
| Worktrees | Temp Git repositories and blocker tests | Real disposable repo isolation |
| Routing | Deterministic/unit/property tests | Reason and override visible per run |
| Codex | Fixture + shared adapter contract | Opt-in installed-CLI smoke |
| Claude | Fixture + shared adapter contract | Required protocol gate + opt-in smoke |
| Handoff | Budget/switch/restart tests | Inspector shows exact supplied capsule |
| Approvals | Fake-adapter lifecycle tests | Harmless allow/deny flow in app |
| UI | Vitest, Testing Library, axe | Keyboard and 1280×720 review |
| Privacy | Tracked-file scanner regression | Pre-publish scan output |
| Packaging | macOS CI release build | Launch built unsigned `.app` |

Implementation is complete only when all applicable rows have current evidence, failures and skipped account-backed checks are reported accurately, the canonical specification matches the implementation, and the public repository contains no private material.
