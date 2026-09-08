# Whole-code audit and live acceptance checks

Reviewed revision: `a7e4a7fc5bc8b634e04a67e76624a3af7636ff82`.

This is a diagnostic review, not a fix or release acceptance. Three independent reviewers
covered the frontend/Tauri boundary, core application/storage/workspace lifecycle, and provider
adapters. The coordinator checked the findings against source and reran the focused behavioral
reproductions. No application code, provider settings, or existing conversations were changed.

## Confirmed findings

### 1. P1: Provider shutdown does not terminate tool descendants

`crates/prompting-time-core/src/providers/process.rs:50` starts the CLI without establishing an
owned process group. `kill_and_wait` at line 662 kills and reaps only that direct process.

An isolated subprocess reproduction used the real `JsonLineProcess` with a shell that starts a
delayed sentinel writer. Explicit shutdown, owner drop, and unexpected stdout EOF each removed
the direct process, but the descendant subsequently wrote the sentinel. All three cases were
reproduced independently by the coordinator.

The app can report cleanup complete while earlier tool work continues mutating files. This is
a transport ownership defect, not evidence that every normal Codex/Claude turn leaves orphans.
Establish and terminate the owned subprocess tree, then test that descendants cannot mutate
after cleanup returns. Do not signal unrelated processes.

### 2. P1: Long timelines move the composer off-screen

`src/styles/app.css:1-6` gives the flex application shell a minimum height but no constrained
height. Timeline content expands the document instead of scrolling inside the timeline pane.

An isolated Chromium experiment using the repository CSS and 80 synthetic messages at
1280 by 720 produced a 7904-pixel document, with the composer starting at pixel 7725. Timeline
client and scroll heights were both 7578, so its scroll container had no internal overflow.
Changing only the synthetic shell's height to `100%` restored the document to 720 pixels and
timeline client/scroll heights to 394/7578. This also prevents the existing timeline auto-scroll
from working as intended. Constrain the shell and add browser-level overflow coverage.

This was a real browser layout test with synthetic DOM, not a native Tauri walkthrough.

### 3. P1: Running Codex turns have no direct stop control

`src/features/timeline/Composer.tsx:285` hides the Interrupt button when steering is available.
Codex advertises both steering and interruption, so the ordinary running state only offers
“Steer Codex.” A component reproduction with Claude unavailable confirmed there was no Interrupt
button, and selecting Auto or Codex did not open an interruption dialog. Switching to another
available provider is an indirect workaround, but fails when no alternative is available.

Keep interruption independently available for an active interruptible turn. Test all capability
combinations, especially a steerable provider with no available alternative.

### 4. P2: Steering instructions disappear from canonical history and handoffs

`crates/prompting-time-core/src/runtime.rs:1179-1186` dispatches accepted steering without
persisting its text. An application-level fixture started Codex, accepted a new constraint,
interrupted the run, and switched to Claude. The constraint was absent from every canonical
timeline event and the next provider's prompt.

This loses user intent unless the first provider happens to echo it. Persist steering in the
authoritative conversation model, with explicit treatment of dispatch success or ambiguity.

### 5. P2: Answered questions are missing from cross-provider context

`crates/prompting-time-core/src/app.rs:972-981` reads the message projection for handoffs, while
answered questions are stored separately as approval resolutions and lifecycle events. An
application fixture acknowledged a non-secret environment choice, confirmed its durable answer,
completed the turn, and switched providers. Neither question nor answer reached the next prompt.

Include acknowledged, non-secret question/answer context in bounded handoffs. The fix must retain
secret-answer privacy and avoid assuming that a provider's summary repeats the user's choice.

### 6. P2: Git hooks survive command timeout and rollback

`crates/prompting-time-core/src/workspace.rs:2229-2240` starts Git without subprocess-group
ownership; `terminate_and_reap` at lines 2363-2378 kills only Git itself. A real temporary Git
repository used a finite post-checkout hook that slept for 12 seconds before writing a sentinel.
Worktree preparation returned `GitCommandTimedOut` after about 10.2 seconds and completed
rollback while the sentinel was absent; the hook wrote it afterward.

Terminate owned Git descendants before rollback. This has the same lifetime problem as finding
1 but occurs in a separate execution implementation and needs its own regression.

### 7. P2: The documented Tauri development command uses mismatched ports

`src-tauri/tauri.conf.json:7-9` launches `pnpm dev` and expects `localhost:1420`.
`vite.config.ts:4-11` has no server port, and the package script invokes plain Vite. A dev-server
attempt selected port 5173 before sandbox listening restrictions stopped it. Thus Tauri waits
for a different port from the server it starts. Configure a matching fixed port and strict-port
behavior. A complete native dev launch was not performed.

### 8. P2: Startup failures hide their actionable diagnostic

`src/app/store.ts:182-185` waits for bootstrap and conversation loading in one rejecting
`Promise.all`. When `AppState::failed` holds a storage/recovery diagnostic, bootstrap can return
it but conversation loading necessarily fails because the application service is absent.
Synchronization discards bootstrap, leaving only the generic service-unavailable message.

A store/component reproduction confirmed that bootstrap remains null and the precise diagnostic
and recovery instruction are absent. Preserve bootstrap diagnostics independently of successful
conversation loading and show them on the startup-error screen.

### 9. P1: Current Codex child events fail the application's identity contract

The new live application delegation test ended with `contract violation`, leaving only a failed
root and no canonical child nodes. A separate bounded probe through the unchanged Codex adapter
observed two `SubAgentActivity::Started` events before any `ChildAgentActivity` declaration.
`crates/prompting-time-core/src/store.rs:5007-5009` requires that child identity to already exist,
so the initial child-start event cannot be persisted. This is a confirmed mismatch between the
installed Codex 0.153.4 event stream and the application's identity assumptions.
A focused reproduction against the production store independently returned
`NativeAgentIdentityConflict` for that first child-start event.

Continuing the adapter-only trace independently encountered `subagent-activity-kind`: the
normalizer at `crates/prompting-time-core/src/providers/codex.rs:2386-2392` accepts only started,
interacted, and interrupted kinds. The exact rejected native kind was not retained, so its
meaning remains unresolved. Turn and adapter shutdown both succeeded after this bounded probe.

Re-establish the current Codex child lifecycle contract and normalize identity, parentage, and
terminal events before claiming recursive Codex support. Do not simply weaken identity checks or
treat unknown activity as completion. Descendant-owned notification and approval forwarding is
also still unverified; source inspection suggests it needs a focused follow-up after these
confirmed blockers are fixed.

## Verification

- Workspace Rust suite: 485 passed, zero failed, 17 account/schema tests ignored.
- Frontend suite: 130 passed; frontend lint passed.
- Three existing live application checks passed in 46.86 seconds: exact-file deny/allow,
  Claude → Claude → Codex → Claude marker continuity, and Claude depth-two canonical ancestry.
- Additional live concurrency check passed: four simultaneous conversations, two per provider,
  each recalled its own distinct marker across repeated turns.
- Additional live interruption check passed: interrupt a held Claude write approval, reject a
  stale response, resume the same conversation, and verify the write never occurred.
- Additional live Codex recursive-delegation check failed with a contract violation and only
  the failed root in the canonical tree. A bounded adapter-only follow-up confirmed the event
  ordering and compatibility problems in finding 9; this is not a successful recursive live gate.
- Coordinator reproduced the two context-loss cases, Git-hook timeout, all three provider
  descendant-cleanup cases, and both frontend component regressions.

Live calls used Codex CLI 0.153.4 with the inherited Astra selection and Claude Code 2.1.205,
invented prompts, and temporary workspaces. No private resources were imported. Existing suite
success does not negate these missing-regression failures.

## Boundaries and suggested sequence

Native app inspection did not resolve a running application, so native window behavior and
notification delivery remain unverified. The layout evidence above is from isolated Chromium.
No fixes, commits, pushes, or pull requests were made as part of the review.

Prioritize owned-process cleanup and Codex delegation compatibility, then basic UI scrolling
and interruption, then context preservation, followed by startup and development diagnostics.
Use separately scoped fixes and retain each focused reproducer as a regression in the relevant
suite. No broad abstraction or architectural rewrite is justified by this review.

## Follow-up fixes

The findings and verification above describe the reviewed baseline, not the subsequent fixes.
The approved follow-up retains focused regression coverage and separately scoped local commits.

Owned-process cleanup (findings 1 and 6) now isolates provider transports and Git commands in
owned subprocess groups and signals the group before reaping its leader. Regression fixtures
cover delayed descendants, natural leader exit, shutdown, and a real temporary Git hook that
must not mutate after timeout/rollback. The containment boundary is signalable descendants that
remain in the owned group; deliberate process-group/session or credential escapes are excluded.

Version-matched Codex research resolved finding 9's unknown activity kind as `completed` and
established metadata-only authoritative ancestry. Subsequent implementation adds early/nested
identity discovery and native child lifecycle/control ownership. Canonical live child approval
and three-level recursion checks passed. Independent review also exposed acknowledgement,
cancellation, and terminal-replay races; those now have deterministic regressions and reviewed
fixes. Final whole-change integration verification remains separate from these focused gates.

Whole-change review also found that rejected child discoveries could bypass global request-ID
admission. Common admission now runs before any child correlation: malformed IDs are rejected,
duplicate outstanding IDs invalidate their original before connection teardown, and completed
IDs cannot receive another response. Written-frame regressions cover rejection, expiry,
cancellation, replacement, and valid replay. Independent rereview approved this follow-up and
the whole change.

UI/startup/development fixes (2, 3, 7, 8) are implemented and reviewed. The explicit Interrupt
action now remains available with steering, and startup errors retain the precise diagnostic
and recovery action. Vite serves localhost:1420 and rejects an occupied port. A real-App synthetic
Chromium fixture measured the original 8057-pixel document at a 720-pixel viewport; with the fix,
document and shell stay at 720 pixels, the composer ends at pixel 696, and all long panes scroll
internally. Child selection remains usable. All 140 frontend tests, lint, build, and browser
fixture type-check passed. This does not establish native WebView acceptance.

Canonical user-intent fixes (4, 5) now save confirmed steering and acknowledged non-secret Q&A
through the existing message/event projection. Focused checks cover repeated provider switching,
child provenance, secret filtering, UTF-8 bounds, atomic rollback, native question IDs, immutable
run context cuts, and message-sequence watermarks. Owned control lifetimes coordinate terminal
publication with acknowledgement, including caller drop and queued-work panic/shutdown races.
The 208 scoped checks passed and independent task review approved the changes. No historical
backfill or silent replay was added. All nine findings have individually reviewed fixes;
whole-change verification is recorded separately below. Native visual acceptance and publication
are not implied.

## Whole-change verification

At final code revision `7a485c8`, the serial Rust workspace suite with all features passed:
549 tests, zero failures, and 19 opt-in account/schema tests ignored. Workspace Clippy passed
with all targets/features and warnings denied. Frontend verification passed 140 tests, lint,
production build, and browser-fixture type-check. Independent whole-change review approved the
source and documentation after the request-ID follow-up.

The final Codex child denial/approval and recursion gates passed in 66.70 seconds. Four concurrent
conversations, two per provider, each retained their distinct marker over repeated turns in
10.02 seconds. The three Claude application gates passed in 57.31 seconds at `33662c6`: exact-file
denial/approval, projectless Claude → Claude → Codex → Claude continuity, and completed depth-two
ancestry. The subsequent change only tightened Codex request-ID admission; the affected Codex
gates and both-provider concurrency were refreshed at the final source checkpoint.

The final interruption/resume check passed in 7.75 seconds: a held Claude write approval was
interrupted, a stale answer was rejected, and the same conversation resumed without writing the
target. Two earlier auxiliary attempts timed out waiting for an approval. On the instrumented
attempt, the run had already completed without any approval: Claude declined the synthetic
`never-write.txt` request. The test wait did not detect terminal runs. Adding terminal detection
and using a neutral `approval-target.txt` fixture exercised the intended path successfully,
without changing application code. The first uninstrumented timeout's exact cause was not retained.

The optimized macOS arm64 application bundle built successfully at version 0.1.0. After applying
a local ad-hoc signature to the bundle, strict deep signature verification passed. This is not
Developer ID signing or notarization. The native application was not launched or restarted;
native WebView behavior and notification delivery remain unverified. No changes were published.

An earlier parallel run once missed the initialization-timeout fixture's PID file. Focused and
serial reruns, including the final workspace suite, passed. Its cause remains unproven; scheduling
contention is only a hypothesis, not a diagnosed product defect or a claimed fix.
