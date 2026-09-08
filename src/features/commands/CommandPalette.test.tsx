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

  it("searches duplicate agent names with conversation context and selects after closing", async () => {
    const onSelectAgent = vi.fn();
    const props = { onOpenChange: vi.fn(), conversations: [], selectedId: null, onSelectConversation: vi.fn(), commands: [], onSelectAgent,
      agents: [
        { conversationId: "a", conversationTitle: "Compiler", agent: { id: "one", label: "Reviewer" } },
        { conversationId: "b", conversationTitle: "Runtime", agent: { id: "two", label: "Reviewer" } },
      ],
    };
    const view = render(<CommandPalette {...props} open />);
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "Reviewer Runtime" } });
    await waitFor(() => expect(screen.getByRole("option", { name: /Reviewer Runtime/ })).toBeVisible());
    expect(screen.queryByRole("option", { name: /Reviewer Compiler/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /Reviewer Runtime/ }));
    expect(onSelectAgent).not.toHaveBeenCalled();
    view.rerender(<CommandPalette {...props} open={false} />);
    await waitFor(() => expect(onSelectAgent).toHaveBeenCalledWith("b", "two"));
    expect(props.onSelectConversation).not.toHaveBeenCalled();
  });
});
