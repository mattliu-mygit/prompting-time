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
