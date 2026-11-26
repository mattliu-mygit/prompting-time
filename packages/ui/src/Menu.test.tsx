import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { describe, expect, it, vi } from "vitest";
import { Menu } from "./Menu";
import { axeViolations, render } from "./test/render";

const items = [
  { value: "rerun", label: "Re-run review" },
  { value: "skip", label: "Skip PR" },
  { value: "logs", label: "Open logs", disabled: true },
];

describe("Menu", () => {
  it("opens from its button, moves with the arrow keys, and runs the chosen item", async () => {
    const onSelect = vi.fn();
    render(() => <Menu label="More actions" icon={<svg />} items={items} onSelect={onSelect} />);
    const trigger = screen.getByRole("button", { name: "More actions" });
    expect(trigger).toHaveAttribute("aria-haspopup", "menu");
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();

    trigger.focus();
    fireEvent.click(trigger);
    const list = await screen.findByRole("menu", { name: "More actions" });
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("menuitem", { name: "Open logs" })).toHaveAttribute("aria-disabled", "true");
    expect(await axeViolations()).toEqual([]);

    fireEvent.keyDown(list, { key: "ArrowDown" });
    fireEvent.keyDown(list, { key: "ArrowDown" });
    await waitFor(() => expect(screen.getByRole("menuitem", { name: "Skip PR" })).toHaveAttribute("data-highlighted"));
    fireEvent.keyDown(list, { key: "Enter" });
    await waitFor(() => expect(onSelect).toHaveBeenCalledExactlyOnceWith("skip"));
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("closes on Escape and returns focus to its button", async () => {
    const onSelect = vi.fn();
    render(() => <Menu label="More actions" icon={<svg />} items={items} onSelect={onSelect} />);
    const trigger = screen.getByRole("button", { name: "More actions" });
    trigger.focus();
    fireEvent.click(trigger);
    fireEvent.keyDown(await screen.findByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(onSelect).not.toHaveBeenCalled();
  });
});
