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
Cancel/Escape return focus to the toolbar trigger. Collapsed options are excluded
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

For modal focus integration, use `?chat=approval`, type a draft, open Preview, open
Interrupt, then choose Keep running. After the dialog closes the provider selector
must regain focus, with the preview/draft preserved. A real browser is necessary:
jsdom does not model native inert focus suppression or the same rendering schedule.
