import { render as renderSolid, type JSX } from "@solidjs/web";
import axe from "axe-core";
import { flush } from "solid-js";

const mounted: { dispose: () => void; container: HTMLElement }[] = [];

/** Renders into a fresh element in document.body and applies the first update; setup.ts unmounts it after the test. */
export function render(view: () => JSX.Element) {
  const container = document.body.appendChild(document.createElement("div"));
  mounted.push({ dispose: renderSolid(view, container), container });
  flush();
  return container;
}

export function cleanup() {
  for (const { dispose, container } of mounted.splice(0)) {
    dispose();
    container.remove();
  }
}

/**
 * The ids of axe's violations in the whole document, including portaled menus and dialogs. jsdom
 * draws nothing, so color contrast is left to the browser. "region" (all content inside
 * landmarks) is the page's job, not a component's.
 */
export async function axeViolations(): Promise<string[]> {
  const result = await axe.run(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } });
  return result.violations.map(({ id }) => id);
}
