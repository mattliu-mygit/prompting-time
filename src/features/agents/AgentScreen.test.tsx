import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type { PluginSnapshot } from "../../bridge/types";
import { actionResult, fixturePlugin, fixtureSettings, fixtureStatus } from "../../test/plugins";
import { axeViolations, render, settle } from "../../test/render";
import type { AgentTab } from "./agentMenu";
import { AgentScreen } from "./AgentScreen";

const controlled = fixturePlugin({ actions: [
  ...fixturePlugin().actions,
  { name: "round", title: "Poll now", description: "Runs a round now.", confirm: false },
] });

function renderAgent(options: { plugin?: PluginSnapshot; status?: Record<string, unknown>; tab?: AgentTab; log?: string; error?: string } = {}) {
  const onTab = vi.fn();
  const readPluginLog = vi.fn(async () => ({ path: "/synthetic/plugins/whooper.log", text: options.log ?? "round started\ngh: could not reach GitHub\nround done", truncated: false }));
  render(() => (
    <AgentScreen
      plugin={options.plugin ?? controlled}
      status={{ value: fixtureStatus("Running · next poll 3:45 PM", options.status), error: null }}
      tab={options.tab ?? "logs"}
      onTab={onTab}
      error={options.error ?? null}
      actions={{ pluginRequest: vi.fn(async () => actionResult({ stdout: JSON.stringify(fixtureSettings) })), savePluginSettings: vi.fn() }}
      controls={{ readPluginLog }}
      onPluginFilesChange={() => () => {}}
    />
  ));
  return { onTab, readPluginLog };
}

describe("a plugin's Logs and Settings", () => {
  it("has the tabs Logs and Settings, and shows why the last control failed", async () => {
    const { onTab } = renderAgent({ error: "whooper: Docker isn't running" });
    expect(screen.getByRole("alert")).toHaveTextContent("whooper: Docker isn't running");
    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Logs", "Settings"]);
    // The header holds the name, the status words, and the controls; the screen repeats none of them.
    expect(screen.queryByRole("heading", { name: "Whooper" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Poll now" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Settings" }));
    expect(onTab).toHaveBeenCalledWith("settings");
    expect(await axeViolations()).toEqual([]);
  });

  it("shows the log with a search that keeps matching lines and a copy of them", async () => {
    const { readPluginLog } = renderAgent({ tab: "logs" });
    await settle();
    expect(readPluginLog).toHaveBeenCalledWith("whooper");
    const log = screen.getByLabelText("Whooper's log");
    expect(log).toHaveTextContent("round started gh: could not reach GitHub round done");
    fireEvent.input(screen.getByRole("textbox", { name: "Search the log" }), { target: { value: "github" } });
    flush();
    expect(log.textContent).toBe("gh: could not reach GitHub");
    expect(screen.getByText("1 line")).toBeInTheDocument();
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    fireEvent.click(screen.getByRole("button", { name: "Copy log" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("gh: could not reach GitHub"));
    expect(await axeViolations()).toEqual([]);
  });
});
