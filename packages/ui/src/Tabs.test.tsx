import { fireEvent, screen } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
import { describe, expect, it } from "vitest";
import { Tabs } from "./Tabs";
import { axeViolations, render } from "./test/render";

const tabs = [
  { value: "runs", label: "Runs" },
  { value: "logs", label: "Logs" },
  { value: "settings", label: "Settings" },
];

function renderTabs() {
  const [value, setValue] = createSignal("runs");
  render(() => (
    <Tabs label="Fixture agent" tabs={tabs} value={value()} onChange={setValue}>
      <p>{`The ${value()} panel`}</p>
    </Tabs>
  ));
}

describe("Tabs", () => {
  it("labels the panel with the selected tab and reaches only that tab with Tab", async () => {
    renderTabs();
    expect(screen.getByRole("tablist", { name: "Fixture agent" })).toBeInTheDocument();
    const runs = screen.getByRole("tab", { name: "Runs" });
    expect(runs).toHaveAttribute("aria-selected", "true");
    expect(runs.tabIndex).toBe(0);
    expect(screen.getByRole("tab", { name: "Logs" }).tabIndex).toBe(-1);
    expect(screen.getByRole("tabpanel", { name: "Runs" })).toHaveTextContent("The runs panel");
    expect(await axeViolations()).toEqual([]);

    fireEvent.click(screen.getByRole("tab", { name: "Settings" }));
    flush();
    expect(screen.getByRole("tabpanel", { name: "Settings" })).toHaveTextContent("The settings panel");
  });

  it("selects and focuses with the arrow keys, wrapping around, and with Home and End", () => {
    renderTabs();
    const runs = screen.getByRole("tab", { name: "Runs" });
    runs.focus();
    fireEvent.keyDown(runs, { key: "ArrowRight" });
    flush();
    expect(screen.getByRole("tab", { name: "Logs" })).toHaveFocus();
    expect(screen.getByRole("tab", { name: "Logs" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(document.activeElement!, { key: "End" });
    flush();
    expect(screen.getByRole("tab", { name: "Settings" })).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
    flush();
    expect(runs).toHaveFocus();
    fireEvent.keyDown(runs, { key: "ArrowLeft" });
    flush();
    expect(screen.getByRole("tab", { name: "Settings" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(document.activeElement!, { key: "Home" });
    flush();
    expect(screen.getByRole("tabpanel", { name: "Runs" })).toBeInTheDocument();
  });
});
