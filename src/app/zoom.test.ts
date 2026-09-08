import { fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {} from "vitest/jsdom";
import { installAppZoom } from "./zoom";

const preferenceKey = "prompting-time.zoom";
let dispose: (() => void) | undefined;

beforeEach(() => {
  // Use the browser Storage implementation even when Node defines its own global.
  vi.stubGlobal("localStorage", jsdom.window.localStorage);
  window.localStorage.clear();
});
afterEach(() => {
  dispose?.();
  dispose = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function shortcut(key: string, options: KeyboardEventInit = {}, target: HTMLElement | Window = window) {
  return fireEvent.keyDown(target, { key, metaKey: true, ...options });
}

async function start(stored?: string) {
  if (stored !== undefined) window.localStorage.setItem(preferenceKey, stored);
  const setZoom = vi.fn<(factor: number) => Promise<void>>().mockResolvedValue(undefined);
  dispose = installAppZoom(setZoom);
  await waitFor(() => expect(setZoom).toHaveBeenCalledTimes(1));
  return setZoom;
}

describe("whole-app zoom", () => {
  it.each([[undefined, 1], ["0.75", 0.75], ["1.5", 1.5], ["2", 2]] as const)(
    "restores %s as native scale %s",
    async (stored, expected) => {
      const setZoom = await start(stored);
      expect(setZoom).toHaveBeenCalledWith(expected);
      await waitFor(() => expect(window.localStorage.getItem(preferenceKey)).toBe(String(expected)));
    },
  );

  it.each(["garbage", "NaN", "Infinity", "", "0", "0.5", "2.5", "{}"])(
    "uses default scale for invalid stored value %s",
    async (stored) => {
      const setZoom = await start(stored);
      expect(setZoom).toHaveBeenCalledWith(1);
    },
  );

  it.each([["=", false], ["+", true]] as const)("increases on Command %s", async (key, shiftKey) => {
    const setZoom = await start();
    expect(shortcut(key, { shiftKey })).toBe(false);
    await waitFor(() => expect(setZoom).toHaveBeenLastCalledWith(1.25));
    expect(window.localStorage.getItem(preferenceKey)).toBe("1.25");
  });

  it("decreases and resets to 100%", async () => {
    const setZoom = await start("1.5");
    expect(shortcut("-")).toBe(false);
    await waitFor(() => expect(setZoom).toHaveBeenLastCalledWith(1.25));
    expect(shortcut("0")).toBe(false);
    await waitFor(() => expect(setZoom).toHaveBeenLastCalledWith(1));
  });

  it.each([["0.75", "-", 0.75], ["2", "=", 2]] as const)(
    "keeps %s within the bounds for Command %s",
    async (stored, key, expected) => {
      const setZoom = await start(stored);
      expect(shortcut(key)).toBe(false);
      expect(shortcut(key, { repeat: true })).toBe(false);
      await waitFor(() => expect(window.localStorage.getItem(preferenceKey)).toBe(String(expected)));
      expect(setZoom.mock.calls.every(([factor]) => factor === expected)).toBe(true);
    },
  );

  it("handles zoom in a dialog text field without changing its draft or selection", async () => {
    const setZoom = await start();
    const dialog = document.createElement("dialog");
    const composer = document.createElement("textarea");
    composer.value = "unfinished draft";
    dialog.append(composer);
    document.body.append(dialog);
    composer.setSelectionRange(2, 7);
    // The window capture listener must work even when a dialog stops propagation.
    composer.addEventListener("keydown", (event) => event.stopPropagation());
    expect(shortcut("=", {}, composer)).toBe(false);
    await waitFor(() => expect(setZoom).toHaveBeenLastCalledWith(1.25));
    expect(composer.value).toBe("unfinished draft");
    expect([composer.selectionStart, composer.selectionEnd]).toEqual([2, 7]);
  });

  it.each([
    { key: "=", metaKey: false }, { key: "=", altKey: true },
    { key: "=", ctrlKey: true }, { key: "=", isComposing: true },
    { key: "=", keyCode: 229 }, { key: "_", shiftKey: true },
    { key: "Enter" }, { key: "a" },
  ])("preserves unrelated shortcuts and composition: %j", async ({ key, ...options }) => {
    const setZoom = await start();
    expect(shortcut(key, options)).toBe(true);
    await Promise.resolve();
    expect(setZoom).toHaveBeenCalledTimes(1);
  });

  it("survives unavailable localStorage", async () => {
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      get() { throw new Error("unavailable"); },
    });
    const setZoom = await start();
    expect(setZoom).toHaveBeenCalledWith(1);
    shortcut("=");
    await waitFor(() => expect(setZoom).toHaveBeenLastCalledWith(1.25));
  });

  it("continues zooming when persistence fails", async () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
    const setZoom = await start();
    shortcut("=");
    shortcut("=");
    await waitFor(() => expect(setZoom).toHaveBeenLastCalledWith(1.5));
  });

  it("serializes rapid shortcuts and persists only completed native changes", async () => {
    const setZoom = await start();
    let finish!: () => void;
    setZoom.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    shortcut("=");
    shortcut("=");
    shortcut("-");
    await waitFor(() => expect(setZoom).toHaveBeenCalledTimes(2));
    expect(window.localStorage.getItem(preferenceKey)).toBe("1");
    expect(setZoom.mock.calls.map(([factor]) => factor)).toEqual([1, 1.25]);
    finish();
    await waitFor(() => expect(setZoom.mock.calls.map(([factor]) => factor)).toEqual([1, 1.25, 1.5, 1.25]));
    expect(window.localStorage.getItem(preferenceKey)).toBe("1.25");
  });

  it("keeps the last successful preference on native failure and accepts later shortcuts", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const setZoom = await start();
    const failure = new Error("native failure");
    setZoom.mockRejectedValueOnce(failure);
    shortcut("=");
    await waitFor(() => expect(setZoom).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(warn).toHaveBeenCalledWith("Could not apply app zoom.", failure));
    expect(window.localStorage.getItem(preferenceKey)).toBe("1");
    shortcut("=");
    await waitFor(() => expect(window.localStorage.getItem(preferenceKey)).toBe("1.25"));
    expect(setZoom.mock.calls.map(([factor]) => factor)).toEqual([1, 1.25, 1.25]);
  });

  it("survives failure while restoring and starts later shortcuts from the default", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    window.localStorage.setItem(preferenceKey, "1.5");
    const setZoom = vi.fn<(factor: number) => Promise<void>>()
      .mockRejectedValueOnce(new Error("native unavailable"))
      .mockResolvedValue(undefined);
    dispose = installAppZoom(setZoom);
    await waitFor(() => expect(setZoom).toHaveBeenCalledWith(1.5));
    await waitFor(() => expect(warn).toHaveBeenCalledWith("Could not apply app zoom.", expect.any(Error)));
    expect(window.localStorage.getItem(preferenceKey)).toBe("1.5");
    shortcut("=");
    await waitFor(() => expect(window.localStorage.getItem(preferenceKey)).toBe("1.25"));
  });

  it("removes its listener and skips queued changes on disposal", async () => {
    const setZoom = await start();
    shortcut("=");
    dispose?.();
    expect(shortcut("=")).toBe(true);
    await Promise.resolve();
    expect(setZoom).toHaveBeenCalledTimes(1);
  });
});
