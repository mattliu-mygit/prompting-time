# @prompting-time/ui

Prompting Time's shared UI library, for Prompting Time's Solid UI and for plugin pages such as
Whooper's.

- **Theme:** Sigil's `default` preset from `@sigil-ui/presets`, unchanged except for the fonts:
  Katagami Sans for text and Katagami Mono for code. `theme.ts` merges them with `mergePresets`,
  and `@sigil-ui/tokens` compiles the result to CSS variables (`src/theme/sigil.css`) and a typed
  token object (`src/theme/tokens.ts`). Prompting Time is dark only, so the dark colors apply.
- **Fonts:** the Katagami WOFF2 files and their license are in `fonts/`, so pages work offline.
- **Components:** Solid 2.0 components that read the theme's `--s-*` variables and nothing else:
  `Button`, `IconButton`, `CopyButton`, `Checkbox`, `TextField`, `NumberField`, `Textarea`, `Select`,
  `StatusDot`, `ListRow`, `ConfirmDialog`, `Menu`, `CommandPalette`, `Markdown`, and `Diff`, plus
  the icons in `src/icons.tsx` (Lucide's paths; license in `src/icons.LICENSE`). The dialogs and the
  menu get their focus and keyboard behavior from Zag.js state machines, run through Zag's
  framework-free `VanillaMachine` in `src/zag.ts`, because Zag's Solid adapter targets Solid 1. A
  menu item can be a radio or checkbox choice, which shows a check mark while it is on.
  `Diff` wraps `@pierre/diffs` and draws comment threads from its own Solid tree, one keyed portal
  per thread. `Markdown` parses with `unified` (remark and GitHub's extensions) and highlights code
  with the same Shiki highlighter and Sigil theme as `Diff`, so a page has one highlighter.

## Use

Import the CSS once per page, then the components:

```tsx
import "@prompting-time/ui/styles.css";
import { Button } from "@prompting-time/ui";

<Button variant="primary" onClick={submit}>Submit review</Button>;
```

The package ships its TypeScript and TSX source, so a page needs `solid-js` and `@solidjs/web` 2.x
and a Solid 2.0 build, such as Vite with `@solidjs/vite-plugin`. It is private until we decide how
plugins get it.

## Develop

- `pnpm --filter @prompting-time/ui gallery` serves every component with invented data at
  `http://localhost:5174/gallery/`. Add `?dialog` to open the confirmation dialog.
- `pnpm test` from the repository root runs these tests with the app's. They include axe checks,
  and a check that the committed theme files equal what `theme.ts` produces.
- After changing `theme.ts`, run `pnpm --filter @prompting-time/ui test -u` to rewrite the theme files.
- Solid 2.0 and its packages publish almost daily. Pin releases at least 7 days old, including
  Solid's own dependencies, which `pnpm-workspace.yaml` overrides.
