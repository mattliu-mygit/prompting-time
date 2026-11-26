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
  { name: "pause", title: "Pause", description: "Pauses.", confirm: false },
  { name: "resume", title: "Resume", description: "Resumes.", confirm: false },
] });

function renderAgent(options: { plugin?: PluginSnapshot; status?: Record<string, unknown>; tab?: AgentTab; log?: string } = {}) {
  const onAction = vi.fn();
  const onTab = vi.fn();
  const readPluginLog = vi.fn(async () => ({ path: "/synthetic/plugins/whooper.log", text: options.log ?? "round started\ngh: could not reach GitHub\nround done", truncated: false }));
  render(() => (
    <AgentScreen
      plugin={options.plugin ?? controlled}
      status={{ value: fixtureStatus("Running · next poll 3:45 PM", options.status), error: null }}
      tab={options.tab ?? "logs"}
      onTab={onTab}
      pending={null}
      error={null}
      onAction={onAction}
      actions={{ pluginRequest: vi.fn(async () => actionResult({ stdout: JSON.stringify(fixtureSettings) })), savePluginSettings: vi.fn() }}
      controls={{ readPluginLog }}
      onPluginFilesChange={() => () => {}}
    />
  ));
  return { onAction, onTab, readPluginLog };
}

describe("a background agent's screen", () => {
  it("shows one status sentence, Poll now and Pause, and a menu with Stop, Settings, and Logs", async () => {
    const { onAction } = renderAgent();
    expect(screen.getByRole("heading", { level: 2, name: "Whooper" })).toBeInTheDocument();
    expect(screen.getByText("Running · next poll 3:45 PM")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Poll now" }));
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(onAction.mock.calls).toEqual([["poll"], ["pause"]]);
    fireEvent.click(screen.getByRole("button", { name: "More Whooper actions" }));
    expect((await screen.findAllByRole("menuitem")).map((item) => item.textContent)).toEqual(["Stop", "Settings", "Logs"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Stop" }));
    await waitFor(() => expect(onAction).toHaveBeenLastCalledWith("stop"));
    expect(await axeViolations()).toEqual([]);
  });

  it("offers Resume while the status says paused, and has the tabs Logs and Settings", () => {
    const { onTab } = renderAgent({ status: { paused: true, agent: { words: "Paused", dot: "idle" } } });
    expect(screen.getByRole("button", { name: "Resume" })).toBeEnabled();
    expect(screen.getByText("Paused")).toBeInTheDocument();
    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Logs", "Settings"]);
    fireEvent.click(screen.getByRole("tab", { name: "Settings" }));
    expect(onTab).toHaveBeenCalledWith("settings");
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
