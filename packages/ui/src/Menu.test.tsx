import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
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

  it("keeps its rows when the caller rebuilds the same items", async () => {
    const [current, setCurrent] = createSignal(items);
    render(() => <Menu label="More actions" icon={<svg />} items={current()} onSelect={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    const row = await screen.findByRole("menuitem", { name: "Skip PR" });
    setCurrent(items.map((item) => ({ ...item })));
    flush();
    expect(screen.getByRole("menuitem", { name: "Skip PR" })).toBe(row);
  });

  it("draws radio choices as menuitemradio with the chosen one checked", async () => {
    const [chosen, setChosen] = createSignal("all");
    const statuses = ["all", "failed"].map((value) => ({ value, label: value === "all" ? "All statuses" : "Failed" }));
    render(() => (
      <Menu
        label="Filter"
        icon={<svg />}
        items={statuses.map((status) => ({ ...status, type: "radio" as const, checked: status.value === chosen() }))}
        onSelect={setChosen}
      />
    ));
    const trigger = screen.getByRole("button", { name: "Filter" });
    fireEvent.click(trigger);
    expect(await screen.findByRole("menuitemradio", { name: "All statuses" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "Failed" })).toHaveAttribute("aria-checked", "false");
    expect(await axeViolations()).toEqual([]);
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Failed" }));
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    expect(chosen()).toBe("failed");
    fireEvent.click(trigger);
    expect(await screen.findByRole("menuitemradio", { name: "Failed" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "All statuses" })).toHaveAttribute("aria-checked", "false");
  });
});
