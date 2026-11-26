import { defaultPreset } from "@sigil-ui/presets";
import { describe, expect, it } from "vitest";
import { theme, themeCss, themeTs } from "../theme";

const text = "'Katagami Text', system-ui, sans-serif";
const mono = "'Katagami Mono', ui-monospace, monospace";

/** The dotted paths of the leaves that differ between two token objects. */
function changedPaths(before: unknown, after: unknown, path = ""): string[] {
  if (typeof before !== "object" || before === null || typeof after !== "object" || after === null) {
    return Object.is(before, after) ? [] : [path];
  }
  const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
  return [...keys].flatMap((key) =>
    changedPaths((before as Record<string, unknown>)[key], (after as Record<string, unknown>)[key], path ? `${path}.${key}` : key),
  );
}

describe("theme", () => {
  it("matches the committed CSS variables and TypeScript tokens", async () => {
    await expect(themeCss).toMatchFileSnapshot("./theme/sigil.css");
    await expect(themeTs).toMatchFileSnapshot("./theme/tokens.ts");
  });

  it("uses Katagami Text for text and Katagami Mono for code in every font variable", () => {
    const fonts = Object.fromEntries(
      [...themeCss.matchAll(/^\s*(--s-[\w-]*(?:font|family)[\w-]*):\s*([^;]+);/gm)]
        .filter(([, name]) => !/size|weight/.test(name))
        .map(([, name, value]) => [name, value]),
    );
    expect(fonts).toEqual({
      "--s-font-display": text,
      "--s-font-body": text,
      "--s-font-mono": mono,
      "--s-heading-family": text,
      "--s-button-font-family": "inherit",
      "--s-code-font-family": mono,
    });
  });

  it("changes nothing in Sigil's default preset but the fonts", () => {
    expect(changedPaths(defaultPreset.tokens, theme.tokens).sort()).toEqual([
      "code.font-family",
      "typography.font-body",
      "typography.font-display",
      "typography.font-mono",
      "typography.heading-family",
    ]);
  });
});
