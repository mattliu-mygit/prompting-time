import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type { PluginSnapshot } from "../../bridge/types";
import { actionResult, fixturePlugin, fixtureReviews, fixtureSettings, fixtureStatus, pageActions } from "../../test/plugins";
import { axeViolations, render, settle } from "../../test/render";
import type { AgentTab } from "./agentMenu";
import { AgentScreen } from "./AgentScreen";

const controlled = fixturePlugin({ actions: [...pageActions, { name: "pause", title: "Pause", description: "Pauses.", confirm: false }, { name: "resume", title: "Resume", description: "Resumes.", confirm: false }] });
const todayAt = (hours: number, minutes = 0) => {
  const date = new Date();
  date.setHours(hours, minutes, 0, 0);
  return date.toISOString();
};
const rounds = [
  { started_at: todayAt(9, 30), ended_at: todayAt(9, 33), result: "done", selected: 2, reason: null },
  { started_at: todayAt(9, 15), ended_at: todayAt(9, 16), result: "poll_failed", selected: null, reason: "gh: could not reach GitHub" },
  { started_at: todayAt(9, 0), ended_at: null, result: "skipped", selected: null, reason: "the previous poll was still running" },
];

function renderAgent(options: { plugin?: PluginSnapshot; status?: Record<string, unknown>; tab?: AgentTab; log?: string } = {}) {
  const onAction = vi.fn();
  const onTab = vi.fn();
  const onOpenReview = vi.fn();
  const readPluginLog = vi.fn(async () => ({ path: "/synthetic/plugins/whooper.log", text: options.log ?? "round started\ngh: could not reach GitHub\nround done", truncated: false }));
  render(() => (
    <AgentScreen
      plugin={options.plugin ?? controlled}
      status={{ value: fixtureStatus(todayAt(23, 45), { reviews: fixtureReviews(), rounds, ...options.status }), error: null }}
      tab={options.tab ?? "runs"}
      onTab={onTab}
      pending={null}
      error={null}
      onAction={onAction}
      actions={{ pluginRequest: vi.fn(async () => actionResult({ stdout: JSON.stringify(fixtureSettings) })), savePluginSettings: vi.fn() }}
      controls={{ readPluginLog }}
      onPluginFilesChange={() => () => {}}
      onOpenReview={onOpenReview}
    />
  ));
  return { onAction, onTab, onOpenReview, readPluginLog };
}

describe("a background agent's screen", () => {
  it("shows one status sentence, Poll now and Pause, and a menu with Stop, Settings, and Logs", async () => {
    const { onAction } = renderAgent();
    expect(screen.getByRole("heading", { level: 2, name: "Whooper" })).toBeInTheDocument();
    expect(screen.getByText(/^Running · next poll/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Poll now" }));
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(onAction.mock.calls).toEqual([["poll"], ["pause"]]);
    fireEvent.click(screen.getByRole("button", { name: "More Whooper actions" }));
    expect((await screen.findAllByRole("menuitem")).map((item) => item.textContent)).toEqual(["Stop", "Settings", "Logs"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Stop" }));
    await waitFor(() => expect(onAction).toHaveBeenLastCalledWith("stop"));
    expect(await axeViolations()).toEqual([]);
  });

  it("offers Resume while paused, says reopening resumes, and Start once stopped", () => {
    renderAgent({ status: { paused: true } });
    expect(screen.getByRole("button", { name: "Resume" })).toBeEnabled();
    expect(screen.getByText(/Reopening Prompting Time resumes Whooper/)).toBeInTheDocument();
  });

  it("shows the last poll, today's counts, and the rounds of 15 days", () => {
    renderAgent();
    expect(screen.getByText(/· done$/)).toBeInTheDocument();
    expect(screen.getByText(/waiting on you$/)).toHaveTextContent("1 waiting on you");
    expect(screen.getByRole("img", { name: "Rounds per day for 15 days; today 1 done, 1 failed, 1 skipped" })).toBeInTheDocument();
    // Red is the bars' only color, and the key names it.
    expect(document.querySelector(".agent-bars-key")).toHaveTextContent("failed");
  });

  it("lists rounds and every review run on Runs, filtered to failed or skipped, and opens a run", () => {
    const { onOpenReview } = renderAgent();
    const runs = within(screen.getByRole("tabpanel", { name: "Runs" }));
    const names = () => runs.getAllByRole("row").slice(1).map((row) => [...row.querySelectorAll("td")].slice(1, 3).map((cell) => cell.firstChild?.textContent));
    expect(names()).toHaveLength(8);
    // Only failures have a colored dot, red, beside their words; done polls and reviewed runs have none.
    const dots = Object.fromEntries(runs.getAllByRole("row").slice(1).map((row) => [row.querySelectorAll("td")[2]!.textContent, row.querySelector(".pt-status-dot")!.getAttribute("data-status")]));
    expect(dots).toMatchObject({ "Poll failed": "error", Failed: "error", Done: "idle", Reviewed: "idle", Skipped: "idle" });
    fireEvent.click(runs.getByRole("button", { name: "Failed" }));
    flush();
    expect(runs.getByRole("button", { name: "Failed" })).toHaveAttribute("aria-pressed", "true");
    expect(names()).toEqual([["Poll", "Poll failed"], ["Review widgets#101", "Failed"], ["Review gadgets#55", "Failed"]]);
    fireEvent.click(runs.getByRole("button", { name: "Skipped" }));
    flush();
    expect(names()).toEqual([["Poll", "Skipped"]]);
    expect(runs.getByText("the previous poll was still running")).toBeInTheDocument();
    fireEvent.click(runs.getByRole("button", { name: "All" }));
    flush();
    fireEvent.click(runs.getByRole("button", { name: "Review gadgets#77" }));
    expect(onOpenReview).toHaveBeenCalledWith("20261125T100000Z-77");
  });

  it("searches Runs by pull request: number, title words, author, or repo, leaving the rounds out", async () => {
    renderAgent();
    const runs = within(screen.getByRole("tabpanel", { name: "Runs" }));
    const shown = () => runs.queryAllByRole("row").slice(1).map((row) => row.querySelector("td:nth-child(2)")?.textContent);
    const search = (text: string) => {
      fireEvent.input(runs.getByRole("textbox", { name: "Search PRs" }), { target: { value: text } });
      flush();
    };
    search("#101");
    expect(shown()).toEqual(["Review widgets#101separate cache configuration · @sam-fixture", "Review widgets#101separate cache configuration"]);
    search("projection");
    expect(shown()).toEqual(["Review gadgets#55move the projection"]);
    search("@sam-fixture");
    expect(shown()).toEqual(["Review widgets#101separate cache configuration · @sam-fixture"]);
    search("acme/gadgets");
    expect(shown()).toEqual(["Review gadgets#77add the tags package", "Review gadgets#55move the projection"]);
    fireEvent.click(runs.getByRole("button", { name: "Failed" }));
    flush();
    expect(shown()).toEqual(["Review gadgets#55move the projection"]);
    search("nothing like this");
    expect(runs.getByText("No runs of a matching pull request.")).toBeInTheDocument();
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
