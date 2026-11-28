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
`PATH` entries, then append `$HOME/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, the standard
system binary directories, and Go's: `/usr/local/go/bin`, where Go's installer puts `go`, and
`$HOME/go/bin`, where `go install` puts programs such as gopls (Homebrew's `go` is in
`/opt/homebrew/bin`). Plugin commands get the same `PATH`. Duplicate, empty, and relative entries
are excluded; the home entries require an absolute home directory that can be represented in
`PATH`. This child-only environment also lets an explicitly located CLI find its shebang runtime,
such as Node.js. Custom installations already in `PATH` take precedence. Prompting Time does not
load shell startup files or modify the parent process environment.

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
and the inspector's toggle. **More → Archive conversation** keeps archival out of the main
reading area. The sidebar, **All work**, starts with **«** (**Fold All work**), **Search work and chats**, **New chat**, and the
status filter; then **Needs you**, shown only while something needs you; then the chats;
and at its bottom a small **Plugins** section with **Add plugin…**. **«** folds the sidebar to a narrow
rail, "» All work" in words turned 90 degrees, and a click on the rail unfolds it; a fold you
choose stays while you move between chats. New chat moves into
the header while the sidebar is folded, and the palette's **Search**
stays in the header. The list's sections work like VS Code's sidebar panes: every
section's heading, with its count, stays on screen, the folded sections stack at the
bottom, and the open ones share the space above them, each scrolling on its own. Drag the sidebar's right edge, or focus it and use
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
fold or unfold All work, toggle the inspector, or focus the message input. Arrow keys select, Enter opens
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

## Plugins

Prompting Time can host background agents, such as Whooper, as plugins. The Rust core installs,
checks, starts, and stops them, and the window shows them.

- **Screens.** Installed plugins sit in the Plugins section at the bottom of the sidebar, one
  quiet row each: the name, which opens the plugin's page at its home route, `/`; its status in
  words and a dot, from its process and the `agent` of its status format 2, such as "Running ·
  next poll 3:45 PM" or, in red, "Missing gh"; and a ⋯ menu, opened by a click, with Poll now
  (`round`), Pause or Resume (`pause`, `resume`; Resume while the status says `paused`), Stop or
  Start, Settings, and Logs. While a plugin shows, the All work sidebar folds to a narrow rail, "»
  All work · N waiting on you" in words turned 90 degrees (without the count when no chat waits);
  a click on the rail unfolds it and leaves the plugin in place until a chat is chosen, and **«**
  at the sidebar's top folds it again. The
  window's header then shows the plugin's name, its status words, and a ⋯ menu with the same
  controls; Settings and Logs open in the main view as the tabs Logs (the end of its log, with
  search and copy) and Settings (where it came from, who runs it, the programs it needs, and its
  settings in a form drawn from its settings schema, which the plugin checks and saves), and
  Open page in that menu goes back to the page. Everything else the plugin shows, such as its
  history, is on its page. Stop asks first and names the items whose dot says they run now.
  **Add plugin…**, always at the end of the Plugins section, and the palette install a plugin:
  from a release archive on this Mac, checked by its SHA-256, or by running a plugin folder in
  place, for development.
- **Menu bar.** While a plugin is installed, Prompting Time puts a clock in the macOS menu
  bar, a template image that macOS draws in the menu bar's own color, with a dot at its top right
  while a plugin's row is red, its status lists a blocker, or it can't start. Its menu repeats the
  Plugins section's rows, from the same status in the same words: each plugin's name, which opens its page;
  its status words; its Pause or Resume (Start while it is stopped) and Poll now; then how many
  items need you, which opens the first; then Open Prompting Time and Quit Prompting Time. It is
  on by default; the palette's "Hide plugins from the menu bar" turns it off and "Show
  plugins in the menu bar" back on, and this device remembers the choice.

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
- **Actions.** Prompting Time runs a plugin's actions and returns their output and exit code. It
  keeps the first 8 MiB of what an action prints on stdout and the last MiB of its stderr, and
  says when stdout was longer; a status larger than that fails to read with an error saying so.
  An action marked `confirm` first runs with `--print`, which changes nothing outside this Mac, saves
  the request it would run in a file in the plugin's state folder, names that file in its output,
  and prints `fingerprint: <hex>`, the file's SHA-256, last. Prompting Time returns that file's
  text for the confirmation, and runs the action for real only with the fingerprint the user
  confirmed, as `--expect <hex>`. Exit 13 means the request changed, so the confirmation shows the
  new one and asks again; any other refusal shows the exit code and what the plugin printed on
  stderr. Settings are saved through the `save-settings` action, which checks them.
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
- **Page messages.** The page talks to Prompting Time only through postMessage, in page protocol 2,
  the only one Prompting Time speaks; a manifest must say `page.protocol: 2`. The page sends
  `ready`, then `call {id, action, args}` to run one of its manifest's actions with `args` after
  the action's command, and gets back `result {reply_to, json}`, all the action printed (parsed
  when it is JSON, else the text), or `error {reply_to, message, stage?, exit_code?, stderr?,
  truncated?}`. An action that succeeds but prints more than 8 MiB gets an `error` with
  `truncated: true` and no part of its output, so a page never parses cut-off JSON; a page that
  needs more reads it in parts.
  `message` is for display. `stage` says how far a call got: `cancelled` when the user cancelled
  the confirmation, `preview` when a confirm action's `--print` refused (the confirmation shows the
  refusal, and the page hears once the user closes it), or `run` when the action failed when run
  for real; it is absent when Prompting Time refused the call before then. When the plugin's
  command refused, `exit_code` (null on a signal) and `stderr` (untrimmed, at most its last MiB)
  say how. Prompting Time drops any message that does not fit, refuses an action the manifest does
  not declare, runs an action marked `confirm` only through the confirmation, and refuses
  `--print` and `--expect` from the page for one. Unasked, the page gets `open {route}` for the item or blocker the user opened and
  `changed {files}` when files in the plugin's state folder change, null when more changed than
  one event lists. A call holds at most 64 arguments of at most 64 KiB together.
- **Status format 2.** A plugin whose manifest has a `status` key, `{"format": 2, "groups": [{"id",
  "title", "needs_you", "order"}], "item_actions": [...]}`, prints the format in
  [`docs/plugin-status-v2.schema.json`](docs/plugin-status-v2.schema.json) from its `status` action:
  its row's words and dot (`agent`), problems that stop every job (`blockers`), at most 50
  `items`, each in one of the declared groups, and optionally `paused`. Other top-level fields,
  such as what the plugin's page reads, are ignored. Group IDs are unique and lowercase-kebab,
  and `item_actions` name declared actions. The sidebar's Needs you is one list: the blockers
  ("blocks every job"), then the items of each group marked `needs_you`, by the groups' `order`
  and each group's items newest first, each showing its title, its group's title beside its dot,
  and the plugin's name, then the chats waiting on you. Items in the other groups stay out of the
  sidebar; they are the plugin's page's to show, and the palette's Work group still finds them.
  Command-J steps through Needs you, and the menu bar counts the plugins' items in it. "Search
  work and chats" finds blockers by title and Needs you items by title, detail, or project
  folder; Escape clears it and Enter opens the first result. A row's ⋯ menu, opened by a
  click, lists the item's `actions` and runs the chosen one as `<command> --item <id>`, through
  the confirmation for an action marked `confirm`; a blocker's `action` runs with no extra
  arguments.
  Every Needs you row's ⋯ menu, a waiting chat's too, ends with Hide, which takes the entry out of
  Needs you until it changes: an item until its `updated_at` does, a blocker until its title or
  dot does, and a chat until it stops waiting. A notice says "Hidden <title>" with Undo, and
  "Hidden · N", folded at the end of Needs you, opens on a click to the hidden entries, each
  with Show again. Chips above Needs you, shown while it holds more than one kind of entry,
  choose what it lists: All, Chats, or one plugin's, each with its count. Command-J and search
  keep to the chosen chip; Command-J, the menu bar's count, and opening the next item skip what
  is hidden. A chat that is hidden or filtered out shows with its folder's chats. This device
  remembers what it hides and the chosen chip. An item or blocker that does not fit is left out, as are row actions the manifest
  does not allow. A click on an item or blocker, Command-J landing on one, or choosing one in the
  palette opens the plugin's page at its `route`, with the sidebar folded to its rail, keeping
  the page loaded from one row to the next. When the item or blocker the page shows leaves Needs you, because the plugin's status
  moves it to another group or drops it, as after the user acts on it in the page, or the user hides it, Prompting Time
  opens the next thing under Needs you in Command-J's order, or stays when nothing is left; an
  item opened from another group stays, as does any while its plugin's status can't be read.
  Whooper's page draws a review's diff and header and a failed review itself.

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
`pnpm --filter @prompting-time/ui gallery` serves its component gallery, and `bash scripts/pack-ui.sh`
packs the tarball plugin pages pin (see its README).

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
