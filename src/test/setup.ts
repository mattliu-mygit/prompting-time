import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import type {} from "vitest/jsdom";
import { cleanup } from "./render";

// Node defines a localStorage global of its own, which has no storage unless Node starts with a
// file for it; the app keeps its display preferences in the browser's.
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: jsdom.window.localStorage });

afterEach(() => {
  cleanup();
  // Display preferences, such as which sidebar sections are open, start fresh in each test.
  window.localStorage.clear();
});

// jsdom has no ResizeObserver; Zag.js positions menus with Floating UI, which observes sizes.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
