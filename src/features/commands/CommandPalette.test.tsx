import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import axe from "axe-core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import CommandPalette from "./CommandPalette";

describe("CommandPalette", () => {
  const scrollIntoView = Object.getOwnPropertyDescriptor(Element.prototype, "scrollIntoView");
  beforeEach(() => {
    vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
    Object.defineProperty(Element.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    if (scrollIntoView) Object.defineProperty(Element.prototype, "scrollIntoView", scrollIntoView);
    else Reflect.deleteProperty(Element.prototype, "scrollIntoView");
  });

  it("uses arrow selection, skips disabled commands, and runs only after dialog focus cleanup", async () => {
    const run = vi.fn();
    const disabledRun = vi.fn();
    const onOpenChange = vi.fn();
    const props = {
      onOpenChange, conversations: [], selectedId: null, onSelectConversation: vi.fn(),
      commands: [
        { id: "first", label: "First", run: vi.fn() },
        { id: "unavailable", label: "Unavailable", disabled: true, run: disabledRun },
        { id: "last", label: "Last", run },
      ],
    };
    const { rerender } = render(<CommandPalette {...props} open />);
    const input = screen.getByRole("combobox");
    await waitFor(() => expect(screen.getByRole("option", { name: "First" })).toHaveAttribute("aria-selected", "true"));
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: "Last" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(run).not.toHaveBeenCalled();
    rerender(<CommandPalette {...props} open={false} />);
    await waitFor(() => expect(run).toHaveBeenCalledOnce());
    expect(disabledRun).not.toHaveBeenCalled();
  });

  it("has accessible dialog, search, grouped results, and current conversation semantics", async () => {
    render(<CommandPalette
      open onOpenChange={vi.fn()}
      conversations={[{ id: "one", title: "Compiler", projectRoot: "/projects/compiler" }]}
      selectedId="one" onSelectConversation={vi.fn()}
      commands={[{ id: "new", label: "New conversation", run: vi.fn() }]}
    />);
    expect(screen.getByRole("dialog", { name: "Search conversations and commands" })).toBeVisible();
    expect(screen.getByRole("option", { name: /Compiler.*Current/ })).toBeVisible();
    const result = await axe.run(document.body, { rules: { "color-contrast": { enabled: false } } });
    expect(result.violations.map(({ id }) => id)).toEqual([]);
  });
});
