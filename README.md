# Prompting Time

Prompting Time is a work-in-progress local macOS desktop app for managing coding-agent conversations across installed CLI harnesses. It owns the shared conversation, routing, execution tree, approvals, and workspace state while each provider keeps ownership of its native authentication and sessions.

The current release candidate has a Codex App Server adapter. It has no Claude adapter: Claude Code is discovered and shown with a diagnostic, but remains unavailable for execution because the only protocol probes run so far were blocked by an unauthenticated CLI and are inconclusive.

## What is implemented

- Multiple concurrent projectless and project-backed conversations.
- Recursive agent trees: any child may act as an orchestrator and create descendants.
- Deterministic automatic routing with a visible reason and per-turn provider override.
- Provider-neutral switching and bounded context-handoff machinery at turn boundaries, covered hermetically with fake adapters and Codex protocol boundaries. Live Codex-to-Claude switching is unavailable and unverified.
- Streamed timeline activity, approval and question handling, steering, interruption, and safe archival.
- SQLite-backed conversation state with bounded queries, stable pagination, and startup recovery.
- A narrow Tauri macOS notification integration for background completion, failure, and needs-attention transitions, with hermetic policy and deduplication tests. Native delivery has not been exercised by an automated Tauri E2E or visual smoke test.

The React/TypeScript interface talks to a typed Tauri command boundary. Rust owns provider processes, routing, lifecycle supervision, SQLite persistence, and Git worktree safety. Provider-specific protocol data is normalized at adapter boundaries before it reaches the shared application model.

## Runtime compatibility

- The current unsigned bundle is an Apple Silicon artifact. Its application binary declares macOS 11.0 as its deployment target; that inspection is not a blanket compatibility claim for every macOS 11+ machine.
- Public CI builds on a macOS 14 runner. The locally inspected release artifact is Apple Silicon; Intel and universal bundles have not been locally built or tested.

## Development prerequisites

- Xcode Command Line Tools (`xcode-select --install`).
- Public CI pins Node.js 24.12.0 and pnpm 11.19.0. This checkout was also built locally with Node.js 26.5.0; current official Node.js macOS binaries target macOS 13.5 or newer ([Node.js supported platforms](https://github.com/nodejs/node/blob/main/BUILDING.md#platform-list)).
- Rust 1.98.1 through rustup; `rust-toolchain.toml` selects the required components.
- At least one supported local provider CLI.

### Codex

Install Codex, authenticate it with `codex login`, and confirm `codex login status` succeeds before starting Prompting Time. Prompting Time launches and owns `codex app-server`; it does not copy Codex credentials. The integration follows the official [Codex App Server documentation](https://learn.chatgpt.com/docs/app-server).

### Claude Code

Claude Code can be discovered, but there is no Claude adapter and it is not currently eligible for routing or selectable for execution in the UI. The protocol probes are inconclusive: the observed CLI was unauthenticated, so they did not establish long-lived streaming, deferred approvals, interruption/resume, or child-agent identity. Authenticate interactively with `claude` and `/login`, then run the opt-in probe described below. The relevant official interfaces are documented under [headless mode](https://code.claude.com/docs/en/headless), [user input](https://code.claude.com/docs/en/agent-sdk/user-input), and [hooks](https://code.claude.com/docs/en/hooks).

## Workspaces

A conversation can run without a project. For a Git project, Prompting Time defaults to a dedicated worktree so concurrent conversations do not share a checkout. You can explicitly choose the current checkout instead. A non-Git directory runs directly because Git worktree isolation is unavailable.

Archiving removes a conversation from active navigation but keeps its durable history. It does not imply worktree deletion. Prompting Time removes only app-owned worktrees proven clean, nondivergent, unused, and still bound to their recorded Git metadata; otherwise it reports the blocker and preserves the directory.

## Develop and verify

```sh
pnpm install --frozen-lockfile
pnpm tauri dev
```

Run the release checks with:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
pnpm lint
pnpm test
pnpm build
bash scripts/privacy-scan.test.sh
bash scripts/privacy-scan.sh
pnpm tauri build --bundles app
```

The unsigned bundle is produced at `target/release/bundle/macos/Prompting Time.app`. Open it directly from Finder for local testing, or copy it into `/Applications`. Because the development bundle is neither signed nor notarized, macOS may require you to use Finder's **Open** confirmation. Do not distribute it as a trusted release artifact.

## Opt-in live probes

Public CI uses hermetic fixtures and does not consume provider accounts. Live probes create only temporary test workspaces and must be enabled explicitly:

```sh
PROMPTING_TIME_LIVE_CODEX=1 cargo test -p prompting-time-core --test adapter_contract \
  live_codex_smoke_uses_an_empty_temporary_git_repository -- --ignored --nocapture --test-threads=1

PROMPTING_TIME_LIVE_CLAUDE=1 cargo test -p prompting-time-core --test claude_protocol \
  -- --ignored --nocapture --test-threads=1
```

No live provider result is implied by a normal test or build. The recorded Claude gate remains inconclusive because its observed CLI session was unauthenticated; see [provider protocol evidence](docs/provider-protocols.md).

## Privacy boundary

Runtime state lives under the user's macOS Application Support directory. Conversations, prompts, model output, tool output, machine paths, credentials, provider sessions, imported resources, and local configuration do not belong in this repository.

Background notifications use only the fixed app name and a fixed status label. Conversation titles and content never appear in notification payloads.

`scripts/privacy-scan.sh` examines tracked files only. In addition to its generic path, credential, provider-state, and runtime-artifact checks, a maintainer can supply a newline-delimited literal denylist stored outside the repository:

```sh
PROMPTING_TIME_PRIVATE_TERMS_FILE=/absolute/path/outside/repository.txt \
  bash scripts/privacy-scan.sh
```

The scanner reports categories only; it does not print matching content or private denylist terms. This is a release gate, not a substitute for reviewing the staged diff.

## Recovery behavior

Before any session or turn call, the supervisor atomically claims the queued run in SQLite and marks its dispatch outcome as potentially ambiguous. New-message persistence and its first claim commit in one transaction. Every provider start, resume, turn, steering, or approval-response call also checks the current durable owner immediately before dispatch. After a run is claimed, every supervised event, native-session bind, context advance, approval transition, fallback, interruption, failure, and terminal transition checks that same expected owner inside its write transaction; a superseded supervisor fails closed. Claims use a 120-second lease renewed every 15 seconds for both executing and capacity-queued work. Recovery adds a five-minute stale margin, so a scheduler stall or ordinary macOS sleep does not let another instance interrupt a live owner merely because one renewal was delayed.

A competing instance leaves claimed work alone during startup, including a wall-clock-expired claim, so a process waking from macOS sleep gets a full live scheduling window to renew. Each reconciler must first win a unique durable recovery-attempt claim before reserving supervisor capacity, enqueueing, or interrupting; CAS losers do nothing. A safely re-entered queued root promotes that temporary claim to the stable supervisor owner with another CAS immediately before enqueue. After two minutes, and every two minutes thereafter, bounded background recovery atomically takes ownership only after the lease and stale margin have elapsed and only when the prior owner is a different instance. This fences a late former owner before the run is interrupted child-first with an actionable diagnostic. A provably unclaimed queued root can be re-entered after restart. An ambiguous claim is never replayed, and running or waiting turns are never replayed because the current model does not retain a provider-confirmed active-turn token.

Shutdown asks owned run tasks and provider adapters to stop, waits up to five seconds, then forces and awaits remaining owned work. Startup reconciliation has a fixed deadline and fails closed if it cannot establish coherent durable state.

## Known limitations

- Claude execution is gated and unavailable.
- Live Codex-to-Claude provider switching is unavailable; only the provider-neutral machinery has hermetic coverage.
- The app is unsigned and not notarized.
- There is no updater or packaged installer yet.
- Active or approval-waiting provider turns are conservatively interrupted on app restart.
- Native notification delivery and full Tauri visual/E2E flows have not been smoke-tested; current evidence is hermetic Rust/UI coverage and bundle inspection.
- Notification deduplication keeps one small process-local entry per active conversation and prunes entries during complete paged resynchronization.
- Archived conversations do not yet have a dedicated history browser.
- Worktree cleanup blockers are reported but there is no force-delete path.

## Milestone 2

The next milestone is a private, provider-neutral local resource library for skills, memories, and referenced files. It will preserve source metadata, deduplicate content, record provider compatibility, and stage selected resources into provider-native formats. None of those private resources will be committed to this public repository, and Milestone 1 does not copy them.

The durable product and architecture contract is in the [Prompting Time design specification](docs/superpowers/specs/2026-09-03-prompting-time-design.md).
