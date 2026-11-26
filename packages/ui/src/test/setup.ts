import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "./render";

afterEach(cleanup);

// jsdom has no ResizeObserver; Zag.js positions menus with Floating UI, which observes sizes.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
