import "@testing-library/jest-dom/vitest";
import { OBSERVE, type DiagnosticEvent } from "solid-js";
import { afterEach } from "vitest";
import type {} from "vitest/jsdom";
import { cleanup } from "./render";

// Node defines a localStorage global of its own, which has no storage unless Node starts with a
// file for it; the app keeps its display preferences in the browser's.
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: jsdom.window.localStorage });

// Solid's dev build reports STRICT_READ_UNTRACKED when code reads a reactive value where the read
// can never update, such as a prop read in an effect's callback. Each one is a bug, so the test
// that causes one fails.
const untrackedReads: DiagnosticEvent[] = [];
OBSERVE?.diagnostics.subscribe((event) => {
  if (event.code === "STRICT_READ_UNTRACKED") untrackedReads.push(event);
});

afterEach(() => {
  cleanup();
  // Display preferences, such as which sidebar sections are open, start fresh in each test.
  window.localStorage.clear();
  const reads = untrackedReads.splice(0);
  if (reads.length > 0) {
    const where = (event: DiagnosticEvent) => [event.message, event.ownerPath && `in ${event.ownerPath.join(" › ")}`, event.loc && `at ${event.loc}`].filter(Boolean).join("\n  ");
    throw new Error(`Solid reported ${reads.length} untracked ${reads.length === 1 ? "read" : "reads"}:\n${reads.map(where).join("\n")}`);
  }
});

// jsdom has no ResizeObserver; Zag.js positions menus with Floating UI, which observes sizes.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

// jsdom lays nothing out, so it has no scrollIntoView; the sidebar reveals the selected row with it.
Element.prototype.scrollIntoView ??= function scrollIntoView() {};
