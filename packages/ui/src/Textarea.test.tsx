import { fireEvent, screen } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { Textarea } from "./Textarea";
import { axeViolations, cleanup, render } from "./test/render";

describe("Textarea", () => {
  it("is a named multi-line input that reports typing", async () => {
    const onInput = vi.fn();
    let element: HTMLTextAreaElement | undefined;
    render(() => <Textarea ref={element} label="Comment" rows={4} placeholder="Say why" onInput={(event) => onInput(event.currentTarget.value)} />);
    const field = screen.getByRole("textbox", { name: "Comment" });
    expect(field).toBe(element);
    expect(field).toHaveAttribute("rows", "4");
    expect(field).not.toHaveAttribute("data-grow");
    fireEvent.input(field, { target: { value: "Two\nlines" } });
    expect(onInput).toHaveBeenCalledExactlyOnceWith("Two\nlines");
    expect(await axeViolations()).toEqual([]);
  });

  it("grows to fit its text when told to, and shrinks again", () => {
    const [value, setValue] = createSignal("");
    render(() => <Textarea label="Message" rows={2} grow value={value()} onInput={(event) => setValue(event.currentTarget.value)} />);
    const field = screen.getByRole("textbox", { name: "Message" });
    Object.defineProperty(field, "scrollHeight", { configurable: true, value: 160 });
    fireEvent.input(field, { target: { value: "long\n".repeat(8) } });
    flush();
    expect(parseFloat(field.style.height)).toBeGreaterThanOrEqual(160);
    Object.defineProperty(field, "scrollHeight", { configurable: true, value: 48 });
    setValue("");
    flush();
    expect(parseFloat(field.style.height)).toBeLessThan(160);
  });

  it("fits again a frame after its width changes and stops watching when it goes away", () => {
    let notify!: ResizeObserverCallback;
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: ResizeObserverCallback) { notify = callback; }
      observe() {}
      disconnect = disconnect;
    });
    let frame!: FrameRequestCallback;
    const request = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => { frame = callback; return 1; });
    const cancel = vi.spyOn(window, "cancelAnimationFrame");
    try {
      render(() => <Textarea label="Message" grow value="wrapped text" />);
      const field = screen.getByRole("textbox", { name: "Message" });
      Object.defineProperty(field, "clientWidth", { configurable: true, value: 400 });
      Object.defineProperty(field, "scrollHeight", { configurable: true, value: 200 });
      notify([], {} as ResizeObserver);
      expect(request).toHaveBeenCalledOnce();
      frame(0);
      expect(parseFloat(field.style.height)).toBeGreaterThanOrEqual(200);
      Object.defineProperty(field, "clientWidth", { configurable: true, value: 300 });
      notify([], {} as ResizeObserver);
      expect(request).toHaveBeenCalledTimes(2);
      cleanup();
      expect(disconnect).toHaveBeenCalledOnce();
      expect(cancel).toHaveBeenCalledWith(1);
    } finally {
      request.mockRestore();
      cancel.mockRestore();
      vi.unstubAllGlobals();
    }
  });
});
