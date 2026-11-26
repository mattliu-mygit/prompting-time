import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { CommandPalette, type PaletteGroup } from "./CommandPalette";
import { axeViolations, render } from "./test/render";

async function renderPalette(groups: readonly PaletteGroup[]) {
  const [open, setOpen] = createSignal(false);
  const onClose = vi.fn(() => setOpen(false));
  const onSelect = vi.fn();
  render(() => (
    <main>
      <button type="button">Search</button>
      <CommandPalette open={open()} label="Search conversations and commands" emptyText="No conversations or commands found." groups={groups} onClose={onClose} onSelect={onSelect} />
    </main>
  ));
  // Open it after mounting, as a shortcut does, so Zag.js runs its opening steps.
  screen.getByRole("button", { name: "Search" }).focus();
  setOpen(true);
  flush();
  await screen.findByRole("combobox");
  return { onClose, onSelect, setOpen };
}

describe("CommandPalette", () => {
  it("moves with the arrow keys past disabled items and runs the choice only after closing", async () => {
    const { onClose, onSelect } = await renderPalette([
      {
        heading: "Commands",
        items: [
          { id: "first", label: "First" },
          { id: "unavailable", label: "Unavailable", disabled: true },
          { id: "last", label: "Last" },
        ],
      },
    ]);
    const input = screen.getByRole("combobox", { name: "Search conversations and commands" });
    expect(screen.getByRole("option", { name: "First" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("option", { name: "Unavailable" })).toHaveAttribute("aria-disabled", "true");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    flush();
    expect(screen.getByRole("option", { name: "Last" })).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveAttribute("aria-activedescendant", screen.getByRole("option", { name: "Last" }).id);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onClose).toHaveBeenCalledOnce();
    expect(onSelect).not.toHaveBeenCalled();
    flush();
    await waitFor(() => expect(onSelect).toHaveBeenCalledExactlyOnceWith("last"));
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Search" })).toHaveFocus();
  });

  it("is a named dialog with a search field, grouped options, and the current item marked", async () => {
    await renderPalette([
      { heading: "Conversations", items: [{ id: "one", label: "Compiler", detail: "/projects/compiler", current: true }] },
      { heading: "Commands", items: [{ id: "new", label: "Show inspector" }] },
    ]);
    expect(screen.getByRole("dialog", { name: "Search conversations and commands" })).toBeVisible();
    expect(screen.getByRole("group", { name: "Conversations" })).toBeVisible();
    expect(screen.getByRole("option", { name: /Compiler.*Current/ })).toBeVisible();
    expect(await axeViolations()).toEqual([]);
  });

  it("matches every typed word against labels and keywords and says when nothing matches", async () => {
    const { onSelect } = await renderPalette([
      {
        heading: "Conversations",
        items: [
          { id: "one", label: "Reviewer", detail: "Compiler", keywords: ["Compiler"] },
          { id: "two", label: "Reviewer", detail: "Runtime", keywords: ["Runtime"] },
        ],
      },
    ]);
    const input = screen.getByRole("combobox");
    fireEvent.input(input, { target: { value: "reviewer runtime" } });
    flush();
    expect(screen.getByRole("option", { name: /Reviewer.*Runtime/ })).toBeVisible();
    expect(screen.queryByRole("option", { name: /Reviewer.*Compiler/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /Reviewer.*Runtime/ }));
    flush();
    await waitFor(() => expect(onSelect).toHaveBeenCalledExactlyOnceWith("two"));
  });

  it("shows the empty text and closes on Escape without choosing", async () => {
    const { onClose, onSelect } = await renderPalette([{ heading: "Commands", items: [{ id: "a", label: "Show inspector" }] }]);
    const input = screen.getByRole("combobox");
    fireEvent.input(input, { target: { value: "zzz" } });
    flush();
    expect(screen.getByText("No conversations or commands found.")).toBeVisible();
    fireEvent.keyDown(input, { key: "Escape" });
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    flush();
    expect(onSelect).not.toHaveBeenCalled();
  });
});
