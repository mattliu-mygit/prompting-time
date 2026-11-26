# Prompting Time

Prompting Time is a work-in-progress local macOS desktop app for managing coding-agent conversations across installed CLI harnesses. It owns the shared conversation, routing, execution tree, approvals, and workspace state while each provider keeps ownership of its native authentication and sessions.

The current release candidate integrates Codex App Server and Claude Code. Supported, authenticated CLIs are available for automatic routing or explicit provider selection. Claude's actual adapter has passed focused authenticated streaming/resume and approval smoke tests; application orchestration and native UI evidence are reported separately.

## What is implemented

- Multiple concurrent projectless and project-backed conversations.
- Nested conversations: any child may act as an orchestrator and create descendants.
- Deterministic automatic routing with a visible reason and per-turn provider override.
- Provider switching and bounded context handoff at turn boundaries, with hermetic coverage and a passed live Claude → Codex/Astra → Claude application smoke. See [version-specific evidence](docs/provider-protocols.md).
- Streamed timeline activity, approval and question handling, steering where supported, interruption, and safe archival.
- SQLite-backed conversation state with bounded queries, stable pagination, and startup recovery.
- A narrow Tauri macOS notification integration for background completion, failure, and needs-attention transitions, with hermetic policy and deduplication tests. Native delivery has not been exercised by an automated Tauri E2E or visual smoke test.

The interface is Solid 2.0 and TypeScript, built from the shared components in `packages/ui` (`@prompting-time/ui`), and talks to a typed Tauri command boundary. Rust owns provider processes, routing, lifecycle supervision, SQLite persistence, and Git worktree safety. Provider-specific protocol data is normalized at adapter boundaries before it reaches the shared application model.

## Runtime compatibility

- The current development bundle is an Apple Silicon artifact. Its application binary declares macOS 11.0 as its deployment target; that inspection is not a blanket compatibility claim for every macOS 11+ machine.
- Public CI builds on a macOS 14 runner. The locally inspected release artifact is Apple Silicon; Intel and universal bundles have not been locally built or tested.

## Development prerequisites

- Xcode Command Line Tools (`xcode-select --install`).
- Public CI pins Node.js 24.12.0 and pnpm 11.19.0. This checkout was also built locally with Node.js 26.5.0; current official Node.js macOS binaries target macOS 13.5 or newer ([Node.js supported platforms](https://github.com/nodejs/node/blob/main/BUILDING.md#platform-list)).
- Rust 1.98.1 through rustup; `rust-toolchain.toml` selects the required components.
- At least one supported local provider CLI.

Provider discovery, authentication checks, and launches preserve the order of absolute inherited
`PATH` entries, then append `$HOME/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, and the standard
system binary directories. Duplicate, empty, and relative entries are excluded; the home fallback
requires an absolute home directory that can be represented in `PATH`. This child-only environment
also lets an explicitly located CLI find its shebang runtime, such as Node.js. Custom installations
already in `PATH` take precedence. Prompting Time does not load shell startup files or modify the
parent process environment.

### Codex

Install Codex, authenticate it with `codex login`, and confirm `codex login status` succeeds before starting Prompting Time. Prompting Time launches and owns `codex app-server`; it does not copy Codex credentials. The integration follows the official [Codex App Server documentation](https://learn.chatgpt.com/docs/app-server).

### Claude Code

Install Claude Code and run `claude auth login`; verify `claude auth status --json` before starting Prompting Time. Desktop registration uses the adapter's bounded health check and reads only the authentication boolean. Missing, unsupported, unauthenticated, or uninspectable installations remain visible with actionable diagnostics and are excluded from routing.

The compatibility gate accepts major version 2 at or above 2.1.205. Live evidence is specifically for 2.1.205; newer allowed releases still undergo fail-closed protocol validation. The adapter starts one owned process per active turn and resumes its native session on subsequent turns. It supports streaming, in-app permission and single-select question responses, interruption, resume, and recursive child hierarchy; it does not support steering. The CLI selects its default model.

Claude runs with default permissions through stdio, empty filesystem settings sources, and an empty strict MCP configuration. Imported hooks, skills, memories, and other private resources are outside this milestone. The relevant official interfaces are documented under [headless mode](https://code.claude.com/docs/en/headless) and [user input](https://code.claude.com/docs/en/agent-sdk/user-input).

## Workspaces

A conversation can run without a project. For a Git project, Prompting Time defaults to a dedicated worktree so concurrent conversations do not share a checkout. You can explicitly choose the current checkout instead. A non-Git directory runs directly because Git worktree isolation is unavailable.

**New chat** asks where the agent should work: the folders of your recent chats (up to
five, most recent first), **Another folder…**, which opens the native macOS folder
picker, or **No folder**, for questions that don't need code. Prompting Time checks the
folder automatically, creates the chat, and puts the cursor in the composer. No title,
objective, typed path, or manual directory check is required. The folder name supplies
the initial title; your first message supplies the task.

New chats use **Auto · Best fit**. The collapsed **Advanced** options let you
change routing or choose the current Git checkout before choosing a folder. Isolated
worktrees do not copy uncommitted checkout edits, and failed isolation never silently
falls back to the current checkout. Existing conversations keep their saved settings.

Archiving removes a conversation from active navigation but keeps its durable history. It does not imply worktree deletion. Prompting Time removes only app-owned worktrees proven clean, nondivergent, unused, and still bound to their recorded Git metadata; otherwise it reports the blocker and preserves the directory.

## Conversation continuity

The default layout gives the conversation room to breathe: the inspector starts
closed and opens on demand. Sizes follow Sigil's default preset at standard display
scaling: chat and draft text use its 16px large size, controls and sidebar labels
14px, code 13px, and secondary text 12px, its smallest; controls are at least 30px
high. Text is Katagami Text and code Katagami Mono. The new-window default is
1440 × 900; smaller windows remain
supported, with the inspector opening as an overlay when it would crowd the chat.

A single compact header holds the conversation title, provider, current status,
and pane controls. **More → Archive conversation** keeps archival out of the main
reading area. The sidebar starts with the pinned background agents, then **Search PRs
and chats**, **New chat**, and the status filter above its list; New chat moves into the
header when the sidebar is hidden, and the palette's **Search** stays in the header. The
list's sections work like VS Code's sidebar panes: every section's heading, with its count,
stays on screen, the folded sections stack at the bottom, and the open ones share the space
above them, each scrolling on its own. Drag the sidebar's right edge, or focus it and use
the arrow keys, to make it 220 to 520 pixels wide; the width, like which of its sections are
open, is remembered on this device.

On macOS, **⌘+** (or **⌘=**) zooms the whole app in, **⌘−** zooms out, and
**⌘0** resets to 100%. Zoom ranges from 75% to 200% in 25-point steps and is
remembered on this device across launches. These shortcuts also work while typing.

The center pane prioritizes the conversation: assistant replies and your messages
render Markdown, tables, and copyable code blocks. Newly captured tools show an
actual action and target in one compact row that updates as the operation finishes.
Consecutive successful work collapses into a summary of the loaded operations;
running tools, failures, unknown outcomes, and requests for approval stay visible. **Show details**
reveals captured input, output, error, and context, with explicit empty, missing,
or truncated output. Older status-only records remain readable but cannot recover
details that were never captured. Routine protocol notifications live under **Diagnostics** in
the right inspector and load only when you open that section or request another page
or refresh. They remain in durable history without crowding out chat messages.

Messages use a compact layout with a copy icon beside the sender, smaller bubble
padding, and tighter gaps. Copy stays visible and keyboard accessible. Routine
run updates share one muted line, wrapping when needed; failures, notices, and
truncated details remain visible. Message text and paragraph readability are unchanged.

Scrolling up pauses following streamed output. **Jump to latest** returns to the
current reply. Long responses and event details remain bounded, with explicit
disclosure for truncated content; copy controls distinguish previews from complete
messages. Markdown never runs code or automatically loads remote images. Ordinary
HTTP(S) links open in the default browser; other external URL schemes are inert.

The composer edits Markdown source with an optional preview. **Enter** submits;
**Shift+Enter** inserts a newline. The input grows with the draft, then scrolls within
its height limit. Successful submission clears the draft and restores input focus;
failure keeps the draft available. Submitted text preserves Markdown whitespace.
Provider selection and message actions share one footer below the input.
**Context budget**, beside Thinking, offers 200k–500k presets (300k by default)
or **Provider default**. Changes apply to the next new turn; native model limits
and response headroom may cause earlier compaction. Your visible history stays
intact. See the [native compaction contract](docs/superpowers/specs/2026-09-17-context-compaction-design.md)
for supported CLI versions and compatibility limits.
**Composer help** reveals routing guidance and keyboard hints; errors and reasons
that sending is blocked remain visible without opening help.

Unsent drafts, provider choices, and Markdown preview stay with each conversation
when you switch away and back. They are kept in memory only and are discarded when
the app closes; no draft autosave files are created. A completed send clears only
the submitted draft, not newer edits or another conversation's text.

Returning to a recently visited conversation restores your reading position and
expanded activity, including loaded older history. A view following latest keeps
following new output. Reading context stays in memory for up to ten recent views;
it is not saved across app restarts.

Press **⌘K** (or **Ctrl+K**) or use **Search** to find active conversations by title
or project and run common commands. The same palette can create a conversation,
toggle either side pane, or focus the message input. Arrow keys select, Enter opens
or runs the selection, and Escape returns to where you were. Search covers
conversation names/projects and loaded child conversations, not message contents.
Composition input and held Enter keys do not accidentally submit messages.

Roots and children use one conversation model and the same chat view. The sidebar
indents each generation; expand a child to see its own children. Completed children
remain available after the parent starts another turn. Search includes loaded
children, and an ancestor path lets you return to a parent.

Selecting a child opens only its captured activity, not its parent's transcript.
Provider-observed children may have incomplete message history and are read-only
except for supported approval responses. The app explains that limitation and
never sends a child-view message to the parent. Parent drafts and reading positions
survive visiting children. Children inherit workspace context without creating
another worktree or gaining cleanup ownership.

Child identity is currently scoped to the recorded parent run: repeated observations
reuse one child conversation, but the same native child seen in a later parent run
is a separate conversation. The app does not guess cross-run continuity or import
missing historical transcripts.

**Send**, **Steer**, and **Interrupt** stay tied to the provider's supported state.
The composer does not silently queue a message when steering is unavailable.

Confirmed steering is saved as user input in the shared timeline and message history before a
turn becomes terminal. Dropping the UI request does not abandon an admitted operation. Rejected
or uncertain steering is not presented as accepted and is never automatically replayed.

Acknowledged question answers contribute bounded question-and-answer context, associated with
the requesting provider and agent. Secret questions and answers are excluded from this shared
projection before formatting or truncation; the exact response remains in the private approval
record. Child-answer context is available to another provider, but this does not prove that the
original native root directly saw its child's answer.

Handoffs select bounded recent history. Confirmed input is not guaranteed to remain in every
future prompt after arbitrarily much newer content. These fixes apply to new input; they do not
backfill previously lost steering or historical answers. A crash without durable confirmation
leaves the outcome unconfirmed, with no replay.

## Background agent plugins

Prompting Time can host background agents, such as Whooper, as plugins. The Rust core installs,
checks, starts, and stops them, and the window shows them.

- **Screens.** Installed plugins sit pinned at the top of the sidebar, above its sections,
  one compact row each: the name, which opens the plugin's screen; Pause or Resume (`pause`,
  `resume`), Start while it is stopped, and Poll now (`round`) as buttons; a ⋯ menu with Stop,
  Settings, and Open logs; and its status in words from its process and its `status` action, such
  as "Running · next poll 3:45 PM", "Paused", or "Stopped". While the status reports a cause that
  blocks every review (`blocked`), the words read in red, "Can't review · Docker isn't running",
  until the plugin clears it, and open the cause's screen. The plugin's screen has the same words
  and controls, its health (the last poll, today's counts, and its `rounds` per day for 15 days),
  and the tabs Runs (rounds and every review run, filtered to all, failed, or skipped, and searched
  by pull request: number, title, author, or repo), Logs (the end of its log, with search and
  copy), and Settings (where it came from, who runs it, the programs it needs, and its settings in
  a form drawn from its settings schema, which the plugin checks and saves). Stop asks first and
  names the reviews in progress it ends. With no plugin installed the pinned row offers to install
  one, and the palette always does: from a release archive on this Mac, checked by its SHA-256, or
  by running a plugin folder in place, for development.
- **Menu bar.** While a background agent is installed, Prompting Time puts a clock in the macOS menu
  bar, a template image that macOS draws in the menu bar's own color, with a dot at its top right
  while an agent fails, is blocked, or can't start. Its menu repeats the pinned rows, from the same
  status in the same words: each agent's name, which opens its screen; its status words, which
  open a blocking cause's screen; its Pause or Resume (Start while it is stopped) and Poll now; then
  how many reviews need you, which opens the first; then Open Prompting Time and Quit Prompting
  Time. It is on by default; the palette's "Hide background agents from the menu bar" turns it off
  and "Show background agents in the menu bar" back on, and this device remembers the choice.

- **Install.** A plugin is a release archive holding one folder with `prompting-time-plugin.json`
  and the commands it names. Prompting Time downloads it over HTTPS with macOS's `curl` and
  refuses it unless its SHA-256 matches the one given. For development, a local archive (with
  the same check) or a local folder, which runs in place, works too. Installed copies live in
  `<Application Support>/plugins/<name>/`, beside a record of where each came from.
- **Prerequisites.** Before starting a plugin, Prompting Time looks for each program its manifest
  lists on the PATH plugin commands get, the same one provider CLIs get. A program needed only
  for some settings is checked against what the plugin's `get-settings` action prints.
- **Start and stop.** When Prompting Time opens or installs a plugin, it starts the manifest's
  start command in its own process group, unless a live copy of that command holds
  `<state folder>/run.lock/pid`, such as one started from a terminal. When Prompting Time quits,
  it sends SIGTERM to the process groups it started and kills them after 5 seconds; a copy it
  did not start keeps running. Stop and Start do the same on request: Stop ends only the copy
  Prompting Time started, and Start honors the run lock; the plugin runs again the next time
  Prompting Time opens. The start command's output goes to `<Application Support>/plugins/<name>.log`,
  which each start empties; the Logs tab shows its last 256 KiB.
- **Actions.** Prompting Time runs a plugin's actions and returns their output and exit code. An
  action marked `confirm` first runs with `--print`, which changes nothing outside this Mac, saves
  the request it would run in a file in the plugin's state folder, names that file in its output,
  and prints `fingerprint: <hex>`, the file's SHA-256, last. Prompting Time returns that file's
  text for the confirmation, and runs the action for real only with the fingerprint the user
  confirmed, as `--expect <hex>`. The confirmation explains the refusals Whooper's submit exits
  with: 10 (new commits since the review), 11 (not safe to approve; Approve anyway reruns the
  preview with `--approve-anyway`), 12 (an @-mention; Send with the mentions reruns it with
  `--allow-mentions`), and 13 (the request changed, so it shows the new one and asks again).
  Settings are saved through the `save-settings` action, which checks them.
- **State.** A plugin keeps its state in `$XDG_STATE_HOME/<name>` (default
  `~/.local/state/<name>`). Prompting Time passes its `XDG_STATE_HOME` to plugin commands, watches
  that folder, and sends the window a `pluginChanged` event naming the files that changed.

- **Pages.** A plugin's page is the folder that holds its manifest's `page.entry`. The window loads
  it from `pt-plugin://<name>/<path>` in an iframe sandboxed to scripts only, so the page has the
  origin "null" and reaches neither the window nor Tauri. Prompting Time serves only files inside
  that folder (403 for `..` or a symlink out of it), each with `Access-Control-Allow-Origin: null`,
  for the page's module scripts and fonts, and a Content Security Policy that allows the page's own
  files, inline styles, and data URLs, and no fetches. The window has its own policy, in
  `tauri.conf.json`, which frames only `pt-plugin:`; the Vite server sends it too, so development
  and the browser fixture run under it.
- **Page messages.** The page talks to Prompting Time only through postMessage, in page protocol 1
  (Whooper's `page/src/protocol.ts`). Prompting Time drops any message that does not fit, acts
  only on runs the plugin's status lists, and maps each request to an action: `show-review <run>`
  for a review, `draft <run> <change>` for a draft, `ask <run> <question> [--finding <id>]` for Ask
  about this, and `submit <run>`, which goes through the confirmation; an action marked `confirm`
  always does. Each result goes back to the page, which also hears of a new status and of changes
  to the shown run's files.
- **Reviews.** A plugin's status may list `reviews`, as `whooper-status --json` does: the runs of the
  last 30 days, each with the commit it reviewed (`head_sha`), the pull request's head
  (`pr_head_sha`), its `author`, a `cause` when it failed or waits, whether you skipped or removed
  it (`skipped_by_user`, `removed_by_user`), and whether the plugin judges it safe to approve
  (`safe_to_approve`). The sidebar lists the newest run of each pull request in sections that fold
  to a heading with a count, stacked at the bottom of the sidebar; only "Needs you" starts open,
  and it lists every item and scrolls on its own. It holds Ready to approve (the plugin judges the
  run safe to approve, or, in a status without `safe_to_approve`, says approve once CI passes and
  read the whole change), Needs a look, Review failed (a cause of the pull request's own), and
  Chats waiting on you. Then come Reviewing, one "Waiting · <cause>" section per cause that blocks
  every review (`blocked`, or a cause with scope `all`), the chats of each project, and Done. A
  review's row has a two-line title and "widgets#101 · @author · 2 findings". "Search PRs and
  chats" finds a pull request by number, title words, author, or repo in every state, removed ones
  tagged "removed", and chats by title or folder; Escape clears it and Enter opens the first
  result. A cause's screen retries each waiting or failed review with `retry <run>` and then offers
  Run now, asks about one with `ask`, opens the logs, and has Remove all, which asks first. Remove,
  on each review's row and in its header, runs `remove <run>`: the review leaves the lists, its
  files stay on disk, and a notice offers Undo (`remove <run> --undo`); a removed review's page has
  Restore. After Submit, Skip, or Remove of the open review, the next item in "Needs you" opens and
  the notice names it. Command-J steps through "Needs you", and Command-K finds every review; when
  either, the next item, or the menu bar selects a row below or above the visible part of its
  section, the section scrolls just far enough to show it. The window's header names the review's
  state, and for one that says approve once CI passes but that the plugin judges not safe to
  approve it says why, from the kinds of its findings (`show-review`): "approve once CI passes · has
  a question", or "didn't read the whole change" for a review that skipped it. The review's
  controls join that one header row: a ⋯ menu with the pull request's versions as "Review N ·
  <short sha> · <time>" (an earlier one is read-only), Rerun review (`retry <run>`), Skip
  (`skip <run>`, with Undo beside it, `--undo`), and Remove; Retry review after a failure; and,
  for a review with an overview, Walkthrough | Diff and Views. A review without one has a banner
  when the pull request has commits after the reviewed one. While a review is open, All work
  folds to a rail labeled "All work · 6 need you" on its side, with a dot for each agent and each
  item that needs you; a click or Command-\ opens the list beside it, and the same, Escape, or a
  click elsewhere closes it. Nothing opens on hover.
- **Walkthrough.** When `show-review` prints an `overview`, a finished review opens on Prompting
  Time's walkthrough instead of the page, with a navigator on the left that folds to a rail. First
  comes the verdict as a banner tinted green (safe to approve), orange (needs a look), or red
  (failed or blocked), with the plugin's reason; then "Changes since you last looked", one folded
  line when `commits_since_review` lists commits, with Rerun review and ×, which hides it for this
  review and pull request head. The line opens the commits and the version you last opened, which
  Prompting Time remembers per pull request on this device. Then What this PR does, with its
  before and after; Places to look, each numbered in orange, with its reason and evidence (j and k
  step through them, open each reason under its lines, and dim the rest until the mouse moves);
  Calls before and after (`overview.flow`), old and new calls overlaid, a removed one struck
  through; What Whooper checked and didn't; and the change layer by layer, each layer's lines
  drawn by `@prompting-time/ui`'s Diff as excerpts with the places as numbered marks in the gutter,
  the plugin's draft comments under their lines, and a read check that folds the layer. The
  decision waits as Decide… until clicked or the end is reached, then sets Comment or Approve with
  `draft <run>` and submits through the confirmation. Diff shows the page under the same banner
  and notice. Views, beside them and in Command-K, turns each view with data in the format on or
  off for every review: Changes since you last looked, Calls before and after, and Files that
  usually change with these (`co_change`); a view's summary shows only when the review has data
  for it. Three more views keep their names until the format carries their data.
- **Failed reviews.** Page protocol 1 shows only a review with findings, so a review the status
  lists as failed opens on Prompting Time's own screen instead of the page. It reads the run with
  `show-review <run>` and shows the pull request, the reason the plugin recorded (`failure`, or a
  line saying it recorded none), and earlier questions with their answers (`asks`); a review waiting
  on a blocking cause links to that cause's screen, under a red banner. Retry review in the header runs `retry <run>`,
  which queues the pull request for the plugin's next round, shows what it printed, and then offers
  Run now, which runs `round`. Ask why runs `ask <run> <question>` and adds the answer. A failure shows the plugin's own words. The screen runs only actions that need no
  confirmation.

## Develop and verify

```sh
pnpm install --frozen-lockfile
pnpm tauri dev
```

Vite binds `http://localhost:1420` and fails if the port is occupied, matching Tauri's
development URL. The optional [browser layout regression](tests/browser/README.md) renders the
real frontend with synthetic data to check pane scrolling and composer visibility. It does not
connect to native providers or establish native WebView acceptance.

The shared UI library in [`packages/ui`](packages/ui/README.md) holds the theme and the Solid 2.0
components that the Solid rewrite and plugin pages will use. `pnpm test` and `pnpm build` cover it;
`pnpm --filter @prompting-time/ui gallery` serves its component gallery.

Run the release checks with:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features -- --test-threads=1
pnpm lint
pnpm test
pnpm exec tsc --project tests/browser/tsconfig.json
pnpm build
bash scripts/privacy-scan.test.sh
bash scripts/privacy-scan.sh
pnpm tauri build --bundles app
```

The unsigned bundle is produced at `target/release/bundle/macos/Prompting Time.app`. Open it directly from Finder for local testing, or copy it into `/Applications`. Because the development bundle is not Developer ID-signed or notarized, macOS may require you to use Finder's **Open** confirmation. Do not distribute it as a trusted release artifact.

## Opt-in live probes

Public CI uses hermetic fixtures and does not consume provider accounts. Live probes create only temporary test workspaces and must be enabled explicitly:

```sh
PROMPTING_TIME_LIVE_CODEX=1 cargo test -p prompting-time-core --test adapter_contract \
  live_codex_smoke_uses_an_empty_temporary_git_repository -- --ignored --nocapture --test-threads=1

PROMPTING_TIME_LIVE_CODEX=1 cargo test -p prompting-time-core --test codex_app \
  -- --ignored --nocapture --test-threads=1

PROMPTING_TIME_LIVE_CLAUDE=1 cargo test -p prompting-time-core --test claude_adapter \
  live_adapter -- --ignored --nocapture --test-threads=1

PROMPTING_TIME_LIVE_CLAUDE=1 PROMPTING_TIME_LIVE_CODEX=1 \
  cargo test -p prompting-time-core --test claude_adapter live_app_ \
  -- --ignored --nocapture --test-threads=1
```

No live provider result is implied by a normal test or build. The application smokes use fresh projectless state, an invented marker across Claude → Claude → Codex → Claude turns, exact-file denial/approval, and depth-two delegation through canonical application APIs. They do not drive the native UI. The older hook-defer probe remains a known failure and is excluded from these commands; see [provider protocol evidence](docs/provider-protocols.md).

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

Provider transports and Git commands start in owned subprocess groups. Bounded cleanup signals
the group before reaping its leader, including after a natural leader exit, so inheriting workers
and Git hooks cannot simply outlive their direct parent. This contains signalable descendants
that remain in the group; it does not contain processes that deliberately create another group,
session, or credential boundary. Drop cleanup is a last-resort signal/reap path and cannot report
errors to its former caller.

## Known limitations

- Claude multi-select questions are safely declined; the current UI supports single-select responses.
- Codex child lifecycle and controls are supported, but child transcripts are not imported into root history. A root ending with active descendants fails conservatively and cleans up; it does not leave an unbounded background run.
- Claude task identities drive recursive hierarchy and status, but do not imply direct child-session resume. Grandchild text forwarding was not observed in the depth-two live protocol probe.
- If Claude is cancelled before its first prompt and the app then restarts, initialization alone may have left no resumable native transcript. The next attempt fails closed and may require a new conversation. Completed and interrupted prompt sessions have resumed successfully in focused live probes.
- The app is not Developer ID-signed or notarized.
- There is no updater or packaged installer yet.
- Active or approval-waiting provider turns are conservatively interrupted on app restart.
- Native notification delivery and full Tauri visual/E2E flows have not been smoke-tested; current evidence is hermetic Rust/UI coverage and bundle inspection.
- Notification deduplication keeps one small process-local entry per active conversation and prunes entries during complete paged resynchronization.
- Archived conversations do not yet have a dedicated history browser.
- Worktree cleanup blockers are reported but there is no force-delete path.

## Milestone 2

The next milestone is a private, provider-neutral local resource library for skills, memories, and referenced files. It will preserve source metadata, deduplicate content, record provider compatibility, and stage selected resources into provider-native formats. None of those private resources will be committed to this public repository, and Milestone 1 does not copy them.

The durable product and architecture contract is in the [Prompting Time design specification](docs/superpowers/specs/2026-09-03-prompting-time-design.md).
