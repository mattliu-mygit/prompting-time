import { fireEvent, screen } from "@testing-library/dom";
import { describe, expect, it, vi } from "vitest";
import { ListRow } from "./ListRow";
import { StatusDot } from "./StatusDot";
import { axeViolations, render } from "./test/render";

describe("ListRow and StatusDot", () => {
  it("names the row by its text, marks the selected row, and hides the decorative dot", async () => {
    const onClick = vi.fn();
    render(() => (
      <ul>
        <li>
          <ListRow icon={<svg />} title="separate the cache configuration" count={2} status="attention" meta="widgets#101 · approve once CI passes" selected onClick={onClick} />
        </li>
        <li>
          <ListRow icon={<svg />} title="Whooper" count="4 queued" status="success" meta="running · next poll 3:45 PM" />
        </li>
      </ul>
    ));
    const selected = screen.getByRole("button", { name: "separate the cache configuration 2 widgets#101 · approve once CI passes" });
    expect(selected).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("button", { name: /^Whooper 4 queued/ })).not.toHaveAttribute("aria-current");
    expect(selected.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "attention");
    expect(selected.querySelector(".pt-status-dot")).toHaveAttribute("aria-hidden", "true");
    fireEvent.click(selected);
    expect(onClick).toHaveBeenCalledOnce();
    expect(await axeViolations()).toEqual([]);
  });

  it("gives a dot with a label an accessible name", async () => {
    render(() => <StatusDot status="error" label="Failed" />);
    expect(screen.getByRole("img", { name: "Failed" })).toHaveAttribute("data-status", "error");
    expect(await axeViolations()).toEqual([]);
  });
});
