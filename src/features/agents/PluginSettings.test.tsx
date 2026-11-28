import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type { PluginActionRequest, PluginActionResult, PluginSnapshot, SavePluginSettingsRequest } from "../../bridge/types";
import { actionResult, fixturePlugin, fixtureSettings } from "../../test/plugins";
import { axeViolations, render } from "../../test/render";
import { PluginSettings } from "./PluginSettings";

/** Renders the settings screen over a fake bridge whose get-settings prints `settings`. */
function renderSettings(plugin: PluginSnapshot = fixturePlugin(), settings: unknown = fixtureSettings) {
  let printed = settings;
  const pluginRequest = vi.fn(async (request: PluginActionRequest): Promise<PluginActionResult> => {
    expect(request.action).toBe("get-settings");
    return actionResult({ stdout: JSON.stringify(printed) });
  });
  const savePluginSettings = vi.fn<(request: SavePluginSettingsRequest) => Promise<PluginActionResult>>();
  render(() => <PluginSettings plugin={plugin} status={undefined} actions={{ pluginRequest, savePluginSettings }} />);
  return { pluginRequest, savePluginSettings, print: (next: unknown) => { printed = next; } };
}

const input = (element: HTMLElement, value: string) => {
  fireEvent.input(element, { target: { value } });
  flush();
};

describe("a background agent's settings", () => {
  it("draws its fields from the settings schema with the values get-settings printed", async () => {
    const { pluginRequest } = renderSettings();
    const every = await screen.findByRole("spinbutton", { name: "Poll every" });
    expect(every).toHaveValue(15);
    expect(every.closest(".pt-field")).toHaveTextContent("minutes");
    expect(screen.getByRole("textbox", { name: "Start" })).toHaveValue("10:00");
    const days = screen.getByRole("group", { name: "Days" });
    expect(within(days).getAllByRole("checkbox").map((box) => [box.closest("label")!.textContent, (box as HTMLInputElement).checked]))
      .toEqual([["Mon", true], ["Tue", true], ["Wed", true], ["Thu", true], ["Fri", true], ["Sat", false], ["Sun", false]]);
    const agent = screen.getByRole("combobox", { name: "Review agent" });
    expect(agent).toHaveValue("devin");
    expect(within(agent).getAllByRole("option").map((option) => option.textContent)).toEqual(["Claude", "Devin"]);
    const claude = screen.getByRole("group", { name: "Claude" });
    expect(within(claude).getByRole("spinbutton", { name: "Most one review may spend" })).toHaveValue(null);
    expect(screen.getByRole("textbox", { name: "Review skills" })).toHaveValue("fixture-review\nfixture-style");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(pluginRequest).toHaveBeenCalledExactlyOnceWith({ plugin: "whooper", action: "get-settings", args: [], confirmedFingerprint: null });
    expect(await axeViolations()).toEqual([]);
  });

  it("saves only the settings the form changed, then shows what the plugin saved", async () => {
    const { savePluginSettings, print } = renderSettings();
    input(await screen.findByRole("spinbutton", { name: "Poll every" }), "30");
    fireEvent.click(within(screen.getByRole("group", { name: "Days" })).getByRole("checkbox", { name: "Fri" }));
    flush();
    savePluginSettings.mockImplementationOnce(async () => {
      print({ ...fixtureSettings, schedule: { ...fixtureSettings.schedule, interval_minutes: 30, days: ["mon", "tue", "wed", "thu"] } });
      return actionResult();
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(screen.getByText("Saved.")).toBeInTheDocument());
    expect(savePluginSettings).toHaveBeenCalledExactlyOnceWith({
      plugin: "whooper",
      values: JSON.stringify({ schedule: { interval_minutes: 30, days: ["mon", "tue", "wed", "thu"] } }),
    });
    expect(screen.getByRole("spinbutton", { name: "Poll every" })).toHaveValue(30);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("shows a chosen option as chosen, after the edit and after the save", async () => {
    // Claude is the first option, which a select falls back to, so the fixture starts there.
    const { savePluginSettings, print } = renderSettings(fixturePlugin(), { ...fixtureSettings, jobs: { review: "claude" } });
    const agent = await screen.findByRole("combobox", { name: "Review agent" });
    expect(agent).toHaveValue("claude");
    fireEvent.change(agent, { target: { value: "devin" } });
    flush();
    expect(screen.getByRole("combobox", { name: "Review agent" })).toHaveValue("devin");
    savePluginSettings.mockImplementationOnce(async () => {
      print({ ...fixtureSettings, jobs: { review: "devin" } });
      return actionResult();
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(screen.getByText("Saved.")).toBeInTheDocument());
    expect(savePluginSettings).toHaveBeenCalledExactlyOnceWith({ plugin: "whooper", values: JSON.stringify({ jobs: { review: "devin" } }) });
    expect(screen.getByRole("combobox", { name: "Review agent" })).toHaveValue("devin");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("shows the plugin's refusal in its own words and keeps the edit", async () => {
    const { savePluginSettings } = renderSettings();
    input(await screen.findByRole("spinbutton", { name: "Stop a poll after" }), "3");
    savePluginSettings.mockResolvedValueOnce(actionResult({ exitCode: 1, stderr: "whooper-settings: settings.poll.timeout_minutes is below 5\n" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("whooper-settings: settings.poll.timeout_minutes is below 5");
    expect(savePluginSettings).toHaveBeenCalledExactlyOnceWith({ plugin: "whooper", values: JSON.stringify({ poll: { timeout_minutes: 3 } }) });
    expect(screen.getByRole("spinbutton", { name: "Stop a poll after" })).toHaveValue(3);
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Revert" }));
    flush();
    expect(screen.getByRole("spinbutton", { name: "Stop a poll after" })).toHaveValue(10);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("says where the plugin came from, who runs it, and which programs are missing", async () => {
    renderSettings(fixturePlugin({
      source: { kind: "archive", path: "/synthetic/release/whooper-0.1.0.tar.gz", sha256: "b".repeat(64) },
      process: { kind: "notRunning", exit: null },
      prerequisites: [
        { command: "gh", purpose: "Reads pull requests.", needed: true, path: "/synthetic/bin/gh" },
        { command: "slackdump", purpose: "Reads the review asks.", needed: true, path: null },
        { command: "claude", purpose: "Runs Claude jobs.", needed: false, path: null },
      ],
    }));
    await screen.findByRole("spinbutton", { name: "Poll every" });
    expect(screen.getByText(/^Version 0\.1\.0 · local archive \/synthetic\/release\/whooper-0\.1\.0\.tar\.gz, SHA-256 bbbbbbbbbbbb…$/)).toBeInTheDocument();
    expect(screen.getByText("Missing slackdump")).toBeInTheDocument();
    expect(screen.getByText("Prompting Time starts it when it opens once nothing below is missing.")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("Missing: slackdump. Install it on the PATH, then reopen Prompting Time to start Whooper.");
    const programs = screen.getByRole("list");
    expect(within(programs).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "gh/synthetic/bin/gh",
      "slackdumpmissing. Reads the review asks.",
      "claudenot needed with these settings",
    ]);
  });

  it("says who started a running plugin", async () => {
    renderSettings(fixturePlugin({ process: { kind: "runningElsewhere", pid: 77 } }));
    expect(await screen.findByText(/A copy started elsewhere, such as in a terminal, runs it \(process 77\)/)).toBeInTheDocument();
  });
});
