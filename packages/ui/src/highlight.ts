import { getSharedHighlighter } from "@pierre/diffs";
import { DIFF_THEME, registerDiffTheme } from "./diffTheme";

/** One colored piece of a highlighted line. `color` is a Sigil variable, such as var(--s-code-string-color). */
export interface CodeToken {
  content: string;
  color?: string;
}

/**
 * Highlights code with the Shiki highlighter Diff already uses and the same Sigil theme, so the app
 * has one highlighter. Returns the lines' tokens, or null when Shiki has no grammar for the
 * language. Shiki loads each grammar the first time it is needed.
 */
export async function highlightCode(code: string, language: string): Promise<CodeToken[][] | null> {
  registerDiffTheme();
  try {
    const highlighter = await getSharedHighlighter({ themes: [DIFF_THEME], langs: [language] });
    return highlighter
      .codeToTokens(code, { lang: language, theme: DIFF_THEME })
      .tokens.map((line) => line.map(({ content, color }) => ({ content, color })));
  } catch {
    return null;
  }
}
