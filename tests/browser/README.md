# Browser layout regression

This Vite-only fixture renders the real App, store, timeline, composer, sidebar,
and inspector with invented data. It never connects to Tauri or providers and is
not included in the production entry point. Type-check it with
`pnpm exec tsc --project tests/browser/tsconfig.json`. Use an installed Playwright CLI or
the Codex Playwright wrapper; no browser test runner dependency is required.

Start `pnpm dev` from the repository root. Run the following from
`output/playwright/` to contain generated artifacts:

```sh
playwright-cli -s=layout-check open http://localhost:1420/tests/browser/layout.html
playwright-cli -s=layout-check resize 1280 720
playwright-cli -s=layout-check snapshot
```

The inspector starts closed. At this width, open **Show inspector** using its fresh
snapshot ref before running the long-pane check:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/check-layout.ts")).checkLayout()'
```

Using fresh snapshot refs, expand Synthetic conversation 0 and select Synthetic
child 0. Confirm the selected tree item and repeat the measurement command. The
check throws if the shell/composer escape the viewport or any long pane cannot
scroll internally. Close only this session with
`playwright-cli -s=layout-check close`, then stop the dev server you started.

The fixture also supports folder-first creation entirely in browser memory. Open
New conversation and verify Choose folder creates an empty `project` conversation
with Best fit selected and focus in Message. Without a folder creates `New
conversation`. Expand Advanced to inspect Git execution and routing overrides;
Cancel/Escape return focus to the New conversation trigger. Collapsed options are excluded
from keyboard navigation.

Reload the fixture with `?folder=cancel`, `?folder=error`, or `?folder=non-git` to
exercise simulated picker cancellation, actionable failure, or direct directory
selection. These synthetic cases neither display a native picker nor touch the
filesystem. New conversations have an empty timeline; run the existing long-pane
layout check on the original synthetic conversations.

## Chat readability and streaming

Use `?chat=reading` for a formatted coding answer, a table/code block, compact activity,
and a nested agent tree. `?chat=approval` adds a pending request, and `?chat=failure`
adds a classified failure. Approval responses remain unsupported: these are visual
and disclosure fixtures, never command execution.

Use `?chat=stream` for a long history and a manually advanced final reply. To append
a synthetic paragraph and invalidate the real store, run:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/chat-fixture.ts")).advanceChatStream()'
```

Scroll upward before advancing; confirm the reading position stays stable and Jump
to latest restores following. Expand activity and diagnostics using fresh refs.
Diagnostics contains 150 invented notifications to exercise independent paging and
eviction. It must not load on chat refresh; inspect its counter with:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/chat-fixture.ts")).chatFixtureStats()'
```

Inspect desktop and narrow-window screenshots, keyboard navigation, code/table
overflow, message/code copy feedback, and visible approvals/failures. The long-pane
measurement helper applies to the original layout fixture, not short chat scenarios.
None of these checks establishes native Tauri UI acceptance.

Use at least 960 × 600, the native minimum window size, and a wide 1440 × 900 view.
After editing this fixture, restart Vite and reload before dynamic-import streaming
checks: hot-reloaded module URLs can otherwise leave the console importing a separate
fixture instance with no store listener.

## Composer conventions

Use `?composer=send` for browser-memory acknowledgment and sent-message rendering,
`?composer=failure` to reject the send without clearing the draft, and
`?composer=steer` for synthetic steering. No provider process runs. Check Enter sends
once, Shift+Enter adds a newline, Markdown preview/edit preserves source and caret,
long drafts grow then scroll internally, and success clears/shrinks/refocuses input.
Preview and sent user messages should render the same safe Markdown; inspect exact
submitted text and call counts with:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/composer-fixture.ts")).composerFixtureStats()'
```

Composition events can be simulated for guard checks, but that does not establish
real macOS input-method acceptance in the native app.

For conversation drafts, use `?composer=send`: enter different Markdown drafts in
Synthetic conversation 0 and 1, switch between them, and verify each restores its
own text. Send one and verify only its draft clears. Reloading discards unsent
drafts because they are deliberately in memory only. Use `?composer=failure` to
verify failed submission still retains text after switching away and back.

Use `?composer=pending` to hold a synthetic send while switching conversations.
Return to the original conversation and confirm its editor remains disabled and
another Enter does not create a second submission. Complete the request with:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/composer-fixture.ts")).completePendingSubmissions()'
```

Only the submitted conversation clears. This fixture never calls a provider.
After editing the fixture, restart Vite before this check: hot-reloaded module
URLs can otherwise make the console release a different module's pending requests.

## Conversation and command palette

At 1440 × 900 and 960 × 600, type a draft, select part of it, and press Command-K.
The palette search input must receive focus. Escape restores the editor and its
selection. Search for another synthetic conversation, press Enter, and verify its
editor receives focus; returning through the palette restores the original draft.

Check ArrowUp/ArrowDown selection, no results, and Control-K toggling as well.
From Markdown preview, the Focus message command must restore editing without
changing raw text. New conversation must close the palette before focusing Choose
folder; simulated creation focuses the new editor. Opening the narrow inspector
must focus its close button. Command-K must not open another dialog behind the
inspector or a creation/interruption dialog. Confirm no horizontal overflow and
that long palette lists scroll internally. Library behavior still needs these
browser checks; mocked DOM focus alone is not sufficient.

After installing new lazy-loaded dependencies, let Vite optimize them and reload
before acceptance. Capture fresh window errors so optimizer reloads are not
mistaken for a persistent production error.

For modal focus integration, use `?chat=approval`, type a draft, open Preview, open
Interrupt, then choose Keep running. After the dialog closes the provider selector
must regain focus, with the preview/draft preserved. A real browser is necessary:
jsdom does not model native inert focus suppression or the same rendering schedule.

## Reading context and nested conversations

Use `?chat=stream` at 1440 × 900. Scroll to an earlier observation, expand tool
activity, and switch from Synthetic conversation 0 to 1 and back. Confirm the
same message remains at the same viewport offset and the activity stays expanded.
Repeat after advancing the synthetic stream while conversation 1 is selected.
If conversation 0 was following latest instead, returning should show the newest
output. Repeat at 960 × 600; window resizing may reflow text, but should not send
a reader to the end. Reading state is session-local, not a reload guarantee.

Use `?chat=orientation` for 240 paged messages. Load older messages explicitly,
read one outside the newest 80, switch away, advance the stream, and return.
The older message should remain visible; the same helper appends one new message
to conversation 0 without changing the older messages.

Use `?tree=nested` for durable root, child, grandchild, older completed child,
missing transcript, paged siblings, and deep long-name conversations. Select each
through the same sidebar rows, ancestor path, and palette. The header and transcript
must belong to the selected conversation, with no parent/sibling activity mixed in.
Provider-observed children explain why they are read-only and offer no Send,
Steer, Interrupt, routing, or archive control. Returning to the parent preserves
its draft, nondefault provider choice, Markdown preview, and reading position.

At 960 × 600 and 1440 × 900, expand root and child, then check actual geometry:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/check-conversation-tree.ts")).checkConversationTreeGeometry()'
```

Each generation must indent visibly, controls remain at least 32px, and long deep
paths must not escape the viewport. Repeat at 125% CSS scale. Exercise tree arrow
keys, Home/End, Enter/Space, and collapse persistence across pane hide/show and
selection changes. The helper also exports `checkSelectedConversation` for exact
header and transcript-marker assertions.

The fixture exposes manual controls, with no timers or native/provider calls:
`addSyntheticChild`, `startNextSyntheticParentRun`, `advanceSyntheticChildActivity`,
`failSyntheticChildPages`, `archiveSyntheticRoot`, and `treeFixtureStats`, all in
`conversation-tree-fixture.ts`. Newly observed children must appear in an open
branch without toggling it; a later parent run must not remove the selected older
child. Reject a page, verify Retry, recover it, and page to Completed earlier child.
An archived root removes its cached descendants from active navigation. Reload
the fixture to reset it. These checks do not establish native child messaging or
historical transcript import support.

## Compact shell and composer

Use `?chat=reading` on the root conversation at 100% zoom, at 960 × 600 and
1440 × 900. The single app header should be 48–56px, the timeline should start
near its top edge, and the empty composer should contain a field and one footer:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/check-compact-layout.ts")).checkCompactLayout()'
```

Open Composer help using Space and close it using Enter. While expanded, run
`checkComposerHelpLayout()` from the same module: help must appear below every
footer action, not force Send onto a separate row. Verify Preview/Edit preserves
the raw draft and restores input focus. Errors and blocked-send explanations
must remain visible with help closed.

Use `?chat=reading&labels=long` to check an invented long root title and child
conversation name. The header remains one row and tree controls stay reachable.
Navigate to the child and back through its ancestor path using Enter; the parent
draft, preview, and reading position must survive the child visit.

Exercise Filter by keyboard, select Completed, hide/show the sidebar, and verify
the radio selection persists. Search and New must appear only once, moving to the
header while the sidebar is hidden. Open More → Archive and cancel: focus returns
to More. Open Command-K from More and cancel: no stale menu item receives focus.
From Search, use the palette to hide the sidebar, then reopen it from header
Search; focus should return to the moved trigger. Repeat creation cancellation
and successful synthetic creation from both placements.

At 125% scale, repeat narrow inspector opening: it must paint above the inert
sidebar and keep keyboard focus inside. Browser scale checks are geometry
evidence only, not proof of native WebView zoom or macOS keyboard behavior.

## Tool operation history

On `?chat=tools` at 100% scale with details closed, measure directly visible running
and failed rows at 960 × 600 and 1440 × 900:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/check-tool-operations.ts")).checkToolOperations()'
```

The check requires compact rows, unchanged message typography, usable detail
targets, and no page overflow. Repeat with `&tool-provider=claude`.

Use `?chat=tools` for invented read/search/command operations, a running operation,
a failed command, empty output, unknown outcome, bounded output, and a legacy
status-only record. Add `&tool-provider=claude` to exercise Claude attribution with
the same provider-neutral display contract. At 960 × 600 and 1440 × 900, verify
completed work groups compactly while running work and failures remain visible.
Open real details with the keyboard; check absent versus empty output, truncation,
literal output and copy feedback, and no horizontal viewport overflow.
Use `&tool-output=discarded` to verify empty retained output with truncation is not
misrepresented as a genuinely empty result. The fixture omits the native zoom
installation; use browser/CSS scaling for geometry checks, not the macOS shortcut.

The fixture exposes explicit controls without timers or provider execution:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/tool-operations-fixture.ts")).advanceToolOperation()'
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/tool-operations-fixture.ts")).failToolDetails(true)'
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/tool-operations-fixture.ts")).toolFixtureStats()'
```

Advance an open running operation and confirm it becomes completed without losing
its disclosure. Advance again to update its output under the same event ID. With
detail failures enabled, open an undisclosed operation, verify the error, disable
the failure, and retry. Hidden rows must not trigger detail reads.

Use `?chat=tool-history` for 180 messages with an operation at sequence 70. Load
older history, read that operation, type a draft, then advance it. Its status and
open detail should refresh even though it is outside the newest page, without
jumping to latest. Repeat while switched to another conversation and return.
The fixture records at most 32 recent page-request cursors for bounded-read checks.

## Comfortable readability

For message density, use `?chat=density` at 960 × 600 and 1440 × 900, at 100% scale.
It contains two invented exchanges with routine run updates. Before copy feedback
is open, run:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/check-message-density.ts")).checkMessageDensity()'
```

`measureMessageDensity()` in the same module reports geometry without asserting
the density limits. Check copy is visible without hover, sits beside the sender,
and works with Enter/Space. Success and rejected clipboard writes must show visible
status feedback without losing keyboard focus or creating horizontal overflow.
Repeat with long provider/agent attribution; text must wrap without overlapping
the copy control. Use `?chat=reading` to retain long-form Markdown/code/table checks,
`?chat=failure` for visible errors and `?chat=approval` for visible approval requests.
Truncated lifecycle events must retain explicit full-detail disclosure, rather than
being treated as ordinary one-line status updates.

Use `?chat=reading` at 1440 × 900 and 960 × 600. With the inspector initially closed,
run the computed-font/target/layout check:

```sh
playwright-cli -s=layout-check eval 'async () => (await import("/tests/browser/check-readability.ts")).checkReadability()'
```

Open the inspector and repeat with `checkReadability({ inspectorOpen: true })`.
At minimum width it must overlay, trap focus and close with Escape; at wide width
it docks beside the chat. Inspect long inspector values and buttons for wrapping,
code/tables for local scrolling, and the nested sidebar for reachable disclosures.
Expand the first conversation and its child and repeat the check at the default
sidebar width; each of the first three names must retain at least 120px of space.
Capture window `error` events in addition to console output when checking draft
growth and width changes, so observer-delivery errors are not missed.
