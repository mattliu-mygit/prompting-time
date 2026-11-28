# @prompting-time/ui

Prompting Time's shared UI library, for Prompting Time's Solid UI and for plugin pages such as
Whooper's.

- **Theme:** Sigil's `default` preset from `@sigil-ui/presets`, unchanged except for the fonts:
  Katagami Text for all text and Katagami Mono for code. `theme.ts` merges them with `mergePresets`,
  and `@sigil-ui/tokens` compiles the result to CSS variables (`src/theme/sigil.css`) and a typed
  token object (`src/theme/tokens.ts`). Prompting Time is dark only, so the dark colors apply.
- **Fonts:** the Katagami WOFF2 files and their license are in `fonts/`, so pages work offline.
- **Components:** Solid 2.0 components that read the theme's `--s-*` variables and nothing else:
  `Button`, `IconButton`, `CopyButton`, `Checkbox`, `TextField`, `NumberField`, `Textarea`, `Select`,
  `StatusDot`, `ListRow`, `ConfirmDialog`, `Menu`, `Tabs`, `CommandPalette`, `Markdown`, and `Diff`, plus
  the icons in `src/icons.tsx` (Lucide's paths; license in `src/icons.LICENSE`). The dialogs and the
  menu get their focus and keyboard behavior from Zag.js state machines, run through Zag's
  framework-free `VanillaMachine` in `src/zag.ts`, because Zag's Solid adapter targets Solid 1. A
  menu item can be a radio or checkbox choice, which shows a check mark while it is on, and a menu
  opens from an icon button or from a text button with a chevron. `Tabs` follows WAI-ARIA's tabs
  pattern without Zag: the caller owns the selection and draws the selected panel.
  `Diff` wraps `@pierre/diffs` and draws comment threads from its own Solid tree, one keyed portal
  per thread. It draws a file from its old and new contents, or one file of a unified diff parsed
  with the exported `parsePatchFiles`; its Shiki theme is exported as `diffTheme`. `Markdown` parses with `unified` (remark and GitHub's extensions) and highlights code
  with the same Shiki highlighter and Sigil theme as `Diff`, so a page has one highlighter.
  `Select` keeps showing its `value` when its options change, including options built again on
  each read, and shows a value given before its option exists once the option arrives.
- **Settings form logic:** `src/settingsForm.ts`, the form Prompting Time's Settings tab draws for a
  plugin, without its controls, so a plugin page can draw the same form: `settingFields` turns the
  plugin's settings JSON Schema into fields (groups, numbers with a unit taken from a title such as
  "Poll every (minutes)", text, switches, one choice, several choices, and lists); `formValue` and
  `settingValue` convert between a setting and its control's value; `rangeError` says why a number
  is outside the range of its schema's `minimum`, `exclusiveMinimum`, `maximum`, and
  `exclusiveMaximum`, such as "Must be above 0.", for the form to show beside the field and to keep
  Save off; and `changedSettings` gives only the settings a form changed, nested as the settings
  nest, for the plugin's save-settings, which checks everything else.

## Use

Import the CSS once per page, then the components:

```tsx
import "@prompting-time/ui/styles.css";
import { Button } from "@prompting-time/ui";

<Button variant="primary" onClick={submit}>Submit review</Button>;
```

Prompting Time's app imports the TypeScript source through the workspace. Plugin pages get a
built tarball instead, attached to each Prompting Time release.

## Use in a plugin page

A plugin's page depends on the npm package at an exact version, and its build bundles the package
into the page's `dist`, so nothing loads from outside the page at run time:

```json
"dependencies": {
  "@prompting-time/ui": "0.1.0",
  "@solidjs/web": "2.0.1",
  "solid-js": "2.0.1"
}
```

The exact version pins it. npm writes the package's sha512 to `package-lock.json` as `integrity`
(pnpm writes it to `pnpm-lock.yaml` under `resolution`); commit the lockfile. `npm ci` and
`pnpm install --frozen-lockfile` refuse a package whose hash differs.

The package holds:

- `dist/index.js`: the compiled components with every dependency bundled except Solid, which the
  page supplies (`solid-js` and `@solidjs/web` 2.x, as peer dependencies), so the package and the
  page share one Solid. It needs no build options of its own. Shiki's grammars are separate chunks
  that load the first time a language is highlighted. `dist/THIRD_PARTY_LICENSES.md` holds the
  bundled packages' licenses.
- `dist/types/`: TypeScript declarations. The types that come from `@pierre/diffs` (`Diff`'s file
  props, `parsePatchFiles`, `FileDiffMetadata`, `DiffLineAnnotation`, `FileContents`) and from
  `@sigil-ui/tokens` (`tokens`) resolve only when the page also installs those packages at the
  versions in this `package.json`, as dev dependencies; otherwise, with `skipLibCheck`, TypeScript
  treats them as `any`.
- `src/styles.css` and the CSS it imports, with `fonts/` (Katagami and its OFL license). The page's
  bundler copies the fonts into its own `dist`.

## Release

Releases go to npm as `@prompting-time/ui`, under the MIT license:

1. Set `version` in this `package.json` to the release's version.
2. Run `bash scripts/pack-ui.sh` from the repository root. It builds the package, writes
   `packages/ui/prompting-time-ui-<version>.tgz`, and prints its integrity.
3. Publish that tarball, not the folder:
   `npm publish packages/ui/prompting-time-ui-<version>.tgz --access public`. pnpm's pack applies
   `publishConfig`'s exports, which point at `dist`; npm publishing the folder would not. The npm
   account needs two-factor authentication: npm confirms in the browser, or takes `--otp <code>`.
4. Check that `npm view @prompting-time/ui@<version> dist.integrity` equals the integrity
   `pack-ui.sh` printed.

## Develop

- `pnpm --filter @prompting-time/ui gallery` serves every component with invented data at
  `http://localhost:5174/gallery/`. Add `?dialog` to open the confirmation dialog.
- `pnpm test` from the repository root runs these tests with the app's. They include axe checks,
  and a check that the committed theme files equal what `theme.ts` produces.
- After changing `theme.ts`, run `pnpm --filter @prompting-time/ui test -u` to rewrite the theme files.
- The packages the components use at run time are `devDependencies`, because the package build
  (`scripts/pack-ui.sh`) bundles them into the tarball; the workspace installs them either way.
  Solid stays a peer dependency.
- Solid 2.0 and its packages publish almost daily. Pin releases at least 7 days old, including
  Solid's own dependencies, which `pnpm-workspace.yaml` overrides.
