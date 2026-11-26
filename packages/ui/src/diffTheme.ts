import { registerCustomTheme } from "@pierre/diffs";

/** The name Diff gives Pierre for the Shiki theme below. */
export const DIFF_THEME = "prompting-time-sigil";

const sigil = (name: string) => `var(--s-${name})`;

/**
 * A Shiki theme whose colors are Sigil's CSS variables. Custom properties inherit into Pierre's
 * Shadow DOM, so the browser resolves them there, and a theme change recolors the diff without
 * highlighting it again. Types take the number color, as in the agreed mock.
 */
export const diffTheme = {
  name: DIFF_THEME,
  type: "dark" as const,
  colors: {
    "editor.background": sigil("code-bg"),
    "editor.foreground": sigil("text"),
    // Pierre reads these two for its added and deleted line colors.
    "gitDecoration.addedResourceForeground": sigil("success"),
    "gitDecoration.deletedResourceForeground": sigil("error"),
  },
  tokenColors: [
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: sigil("code-comment-color") } },
    { scope: ["string", "punctuation.definition.string"], settings: { foreground: sigil("code-string-color") } },
    { scope: ["constant.numeric"], settings: { foreground: sigil("code-number-color") } },
    { scope: ["entity.name.type", "support.type"], settings: { foreground: sigil("code-number-color") } },
    { scope: ["keyword", "storage", "storage.type", "storage.modifier"], settings: { foreground: sigil("code-keyword-color") } },
    { scope: ["keyword.operator", "punctuation"], settings: { foreground: sigil("text") } },
    { scope: ["entity.name.function", "support.function"], settings: { foreground: sigil("code-function-color") } },
  ],
};

let registered = false;

/** Registers the theme with Pierre once; Pierre logs an error when a name is registered twice. */
export function registerDiffTheme() {
  if (registered) return;
  registerCustomTheme(DIFF_THEME, async () => diffTheme);
  registered = true;
}
