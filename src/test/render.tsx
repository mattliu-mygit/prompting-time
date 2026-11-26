import { render as renderSolid, type JSX } from "@solidjs/web";
import axe from "axe-core";
import { flush } from "solid-js";

const mounted: { dispose: () => void; container: HTMLElement }[] = [];

/** Renders into a fresh element in document.body and applies the first update; setup.ts unmounts it after the test. */
export function render(view: () => JSX.Element) {
  const container = document.body.appendChild(document.createElement("div"));
  const entry = { dispose: renderSolid(view, container), container };
  mounted.push(entry);
  flush();
  return {
    container,
    unmount() {
      mounted.splice(mounted.indexOf(entry), 1);
      entry.dispose();
      container.remove();
    },
  };
}

export function cleanup() {
  for (const { dispose, container } of mounted.splice(0)) {
    dispose();
    container.remove();
  }
}

/** Lets pending promises settle, then applies Solid's queued updates. */
export async function settle() {
  for (let round = 0; round < 5; round += 1) await Promise.resolve();
  flush();
}

/**
 * The ids of axe's violations in the whole document. jsdom draws nothing, so color contrast is left
 * to the browser. "region" (all content inside landmarks) is checked only for a whole page.
 */
export async function axeViolations(options: { page?: boolean } = {}): Promise<string[]> {
  const result = await axe.run(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: options.page ?? false } } });
  return result.violations.map(({ id }) => id);
}
