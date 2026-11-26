import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { createSignal } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { Button } from "./Button";
import { ConfirmDialog } from "./ConfirmDialog";
import { axeViolations, render } from "./test/render";

function renderDialog() {
  const [open, setOpen] = createSignal(false);
  const onConfirm = vi.fn(() => setOpen(false));
  const onCancel = vi.fn(() => setOpen(false));
  render(() => (
    <main>
      <Button onClick={() => setOpen(true)}>Submit review</Button>
      <ConfirmDialog open={open()} title="Submit this review to GitHub?" confirmLabel="Submit" onConfirm={onConfirm} onCancel={onCancel}>
        <p>2 comments and an approval post as one review.</p>
      </ConfirmDialog>
    </main>
  ));
  const opener = screen.getByRole("button", { name: "Submit review" });
  return { opener, onConfirm, onCancel };
}

async function openDialog(opener: HTMLElement) {
  opener.focus();
  fireEvent.click(opener);
  return screen.findByRole("alertdialog", { name: "Submit this review to GitHub?" });
}

describe("ConfirmDialog", () => {
  it("is a modal alert dialog that hides the page behind it", async () => {
    const { opener } = renderDialog();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    const dialog = await openDialog(opener);
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveAccessibleDescription("2 comments and an approval post as one review.");
    await waitFor(() => expect(opener.closest("[aria-hidden='true']")).not.toBeNull());
    expect(await axeViolations()).toEqual([]);
  });

  it("cancels on Escape and on Cancel, confirms on its button, and returns focus to the opener", async () => {
    const { opener, onConfirm, onCancel } = renderDialog();
    fireEvent.keyDown(await openDialog(opener), { key: "Escape" });
    await waitFor(() => expect(onCancel).toHaveBeenCalledOnce());
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    await waitFor(() => expect(opener).toHaveFocus());

    await openDialog(opener);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(onCancel).toHaveBeenCalledTimes(2));

    await openDialog(opener);
    fireEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() => expect(onConfirm).toHaveBeenCalledOnce());
    expect(onCancel).toHaveBeenCalledTimes(2);
  });

  it("puts focus on Cancel when it opens", async () => {
    // Zag.js only focuses elements that have a size, and jsdom lays out nothing.
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1);
    const { opener } = renderDialog();
    await openDialog(opener);
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus());
    vi.restoreAllMocks();
  });

  it("disables its buttons and ignores Escape while busy", async () => {
    const [open, setOpen] = createSignal(false);
    const [busy, setBusy] = createSignal(false);
    const onCancel = vi.fn(() => setOpen(false));
    render(() => (
      <main>
        <Button onClick={() => setOpen(true)}>Archive</Button>
        <ConfirmDialog open={open()} busy={busy()} title="Archive Work?" confirmLabel="Archive" onConfirm={() => setBusy(true)} onCancel={onCancel}>
          <p>History is kept.</p>
        </ConfirmDialog>
      </main>
    ));
    const opener = screen.getByRole("button", { name: "Archive" });
    opener.focus();
    fireEvent.click(opener);
    const dialog = await screen.findByRole("alertdialog", { name: "Archive Work?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(dialog).toHaveAttribute("aria-busy", "true"));
    for (const button of within(dialog).getAllByRole("button")) expect(button).toBeDisabled();
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(onCancel).not.toHaveBeenCalled();
    setBusy(false);
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeEnabled());
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(onCancel).toHaveBeenCalledOnce());
    await waitFor(() => expect(opener).toHaveFocus());
  });

  it("can disable only its confirm button", async () => {
    render(() => (
      <ConfirmDialog open title="Interrupt Codex" confirmLabel="Interruption unavailable" confirmDisabled onConfirm={vi.fn()} onCancel={vi.fn()}>
        <p>Codex does not report interruption support.</p>
      </ConfirmDialog>
    ));
    const dialog = await screen.findByRole("alertdialog", { name: "Interrupt Codex" });
    expect(within(dialog).getByRole("button", { name: "Interruption unavailable" })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeEnabled();
  });
});
