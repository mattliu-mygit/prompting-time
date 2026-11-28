import { fireEvent, screen } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { Checkbox } from "./Checkbox";
import { NumberField, TextField } from "./Field";
import { Select } from "./Select";
import { axeViolations, render } from "./test/render";

describe("form controls", () => {
  it("label their controls and describe them with the hint and the error", async () => {
    render(() => (
      <>
        <NumberField label="Poll every" unit="minutes" min={5} max={120} value="15" hint="5 to 120" />
        <NumberField label="Stop a poll after" unit="minutes" value="3" error="Must be at least 5." />
        <TextField label="Review skills" value="multi-angle-review" />
        <Select label="Review agent" options={[{ value: "devin", label: "Devin" }, { value: "claude", label: "Claude" }]} />
        <Checkbox label="Untagged team asks" />
      </>
    ));
    const interval = screen.getByRole("spinbutton", { name: "Poll every" });
    expect(interval).toHaveAttribute("min", "5");
    expect(interval).toHaveAttribute("max", "120");
    expect(interval).toHaveAccessibleDescription("5 to 120");
    expect(interval).not.toHaveAttribute("aria-invalid");
    const limit = screen.getByRole("spinbutton", { name: "Stop a poll after" });
    expect(limit).toHaveAttribute("aria-invalid", "true");
    expect(limit).toHaveAccessibleDescription("Must be at least 5.");
    expect(screen.getAllByText("minutes")).toHaveLength(2);
    expect(screen.getByRole("textbox", { name: "Review skills" })).toHaveValue("multi-angle-review");
    expect(screen.getByRole("combobox", { name: "Review agent" })).toHaveValue("devin");
    expect(screen.getByRole("checkbox", { name: "Untagged team asks" })).not.toBeChecked();
    expect(await axeViolations()).toEqual([]);
  });

  it("report changes through the native events", () => {
    const onInput = vi.fn();
    const onSelect = vi.fn();
    const onCheck = vi.fn();
    render(() => (
      <>
        <TextField label="Message" onInput={(event) => onInput(event.currentTarget.value)} />
        <Select label="Agent" options={[{ value: "devin", label: "Devin" }, { value: "claude", label: "Claude" }]} onChange={(event) => onSelect(event.currentTarget.value)} />
        <Checkbox label="Include in review" onChange={(event) => onCheck(event.currentTarget.checked)} />
      </>
    ));
    fireEvent.input(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Pre-approving to unblock" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Agent" }), { target: { value: "claude" } });
    fireEvent.click(screen.getByRole("checkbox", { name: "Include in review" }));
    expect(onInput).toHaveBeenCalledWith("Pre-approving to unblock");
    expect(onSelect).toHaveBeenCalledWith("claude");
    expect(onCheck).toHaveBeenCalledWith(true);
  });
  it("keep a select's value when its options are drawn again", () => {
    // A caller that builds the options on each read, as a settings form does, replaces every
    // <option>, and a select whose chosen option is removed falls back to its first.
    const [value, setValue] = createSignal("devin");
    const [extra, setExtra] = createSignal<string | null>(null);
    const options = () => [
      { value: "claude", label: "Claude" },
      { value: "devin", label: "Devin" },
      ...(extra() ? [{ value: extra()!, label: extra()! }] : []),
    ];
    render(() => <Select label="Agent" options={options()} value={value()} onChange={(event) => setValue(event.currentTarget.value)} />);
    const select = screen.getByRole("combobox", { name: "Agent" });
    expect(select).toHaveValue("devin");

    setExtra("codex");
    flush();
    expect(select).toHaveValue("devin");

    fireEvent.change(select, { target: { value: "codex" } });
    flush();
    expect(select).toHaveValue("codex");

    // A value set before its option exists shows once the option arrives.
    setExtra(null);
    setValue("astra");
    flush();
    setExtra("astra");
    flush();
    expect(select).toHaveValue("astra");
  });
});
