import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type { PluginActionRequest, PluginActionResult } from "../../bridge/types";
import { actionResult, fixturePlugin, fixtureReviews, fixtureShown, pageActions } from "../../test/plugins";
import { axeViolations, render, settle } from "../../test/render";
import { FailedReview } from "./FailedReview";
import { readReviews, type ReviewEntry } from "./reviews";

const FAILED = "20261124T090000Z-55";
/** An earlier run of #101 that failed without recording why. */
const UNRECORDED = "20261125T080000Z-101";
const REASON = "the review run exited 1 without findings; see runs/20261124T090000Z-55/stderr.log";

/**
 * Renders the screen for a failed review over a fake Whooper. `answer` may answer an action in place
 * of the fake: show-review prints the run as fixtureShown does, and retry, round, and ask succeed.
 */
function renderFailed(runId = FAILED, answer: (request: PluginActionRequest) => PluginActionResult | undefined = () => undefined, overrides: Partial<ReviewEntry> = {}, sameCause: { count: number; open(): void } | null = null) {
  const review = { ...readReviews({ reviews: fixtureReviews() }).find((entry) => entry.run_id === runId)!, ...overrides };
  const pluginRequest = vi.fn(async (request: PluginActionRequest): Promise<PluginActionResult> => {
    const answered = answer(request);
    if (answered) return answered;
    switch (request.action) {
      case "show-review": return actionResult({ stdout: JSON.stringify(fixtureShown(request.args[0]!)) });
      case "retry": return actionResult({ stdout: "queued acme/gadgets#55 for the next round\n" });
      case "round": return actionResult({ stdout: "polled: 1 pull request to review\nacme/gadgets#55: approve once CI passes\n" });
      case "ask": return actionResult({ stdout: "The agent lost its connection before it read the pull request.\n" });
      default: throw new Error(`unexpected ${request.action}`);
    }
  });
  render(() => <FailedReview plugin={fixturePlugin({ actions: pageActions })} review={review} actions={{ pluginRequest, savePluginSettings: vi.fn() }} sameCause={sameCause} />);
  return { pluginRequest };
}

const click = (name: string) => {
  fireEvent.click(screen.getByRole("button", { name }));
  flush();
};

describe("a failed review", () => {
  it("shows the pull request, that the review failed and why, and the answers to earlier questions", async () => {
    const earlier = { question: "Why did this review fail?", finding: null, answer: "The run's **stderr.log** ends with a lost connection.", agent: "devin",
      started_at: "2026-11-24T10:00:00Z", ended_at: "2026-11-24T10:02:00Z", transcript: null };
    const { pluginRequest } = renderFailed(FAILED, (request) => (request.action === "show-review"
      ? actionResult({ stdout: JSON.stringify(fixtureShown(FAILED, { asks: [earlier] })) })
      : undefined));

    expect(screen.getByRole("heading", { level: 2, name: "move the projection" })).toBeInTheDocument();
    expect(screen.getByText("acme/gadgets#55")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 3, name: "Review failed" })).toBeInTheDocument();
    expect(await screen.findByLabelText("Why it failed")).toHaveTextContent(REASON);
    const answers = screen.getByRole("list", { name: "Answers" });
    expect(within(answers).getByText("Why did this review fail?")).toBeInTheDocument();
    expect(answers).toHaveTextContent("The run's stderr.log ends with a lost connection.");
    expect(pluginRequest).toHaveBeenCalledExactlyOnceWith({ plugin: "whooper", action: "show-review", args: [FAILED], confirmedFingerprint: null });
    expect(await axeViolations()).toEqual([]);
  });

  it("says when Whooper recorded no reason", async () => {
    renderFailed(UNRECORDED);
    expect(screen.getByRole("heading", { level: 2, name: "separate cache configuration" })).toBeInTheDocument();
    expect(screen.getByText("acme/widgets#101")).toBeInTheDocument();
    expect(await screen.findByText("Whooper didn't record why this review failed.")).toBeInTheDocument();
    expect(screen.queryByLabelText("Why it failed")).not.toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Answers" })).not.toBeInTheDocument();
  });

  it("names a cause that blocks every review, says the review waits, and opens every review waiting on it", async () => {
    const open = vi.fn();
    renderFailed(FAILED, () => undefined, { outcome: "waiting", cause: { id: "docker", summary: "Docker isn't running", scope: "all" } }, { count: 3, open });
    expect(screen.getByRole("heading", { name: "Waiting · Docker isn't running" })).toBeInTheDocument();
    expect(screen.getByText(/so this review waits for the next round/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
    click("See all 3 reviews waiting");
    expect(open).toHaveBeenCalledOnce();
    click("Ask why");
    await settle();
    expect(screen.getByRole("textbox", { name: "Question" })).toHaveValue("Why is this review waiting?");
    expect(await axeViolations()).toEqual([]);
  });

  it("asks why with the question typed and adds the answer", async () => {
    const { pluginRequest } = renderFailed();
    await settle();

    click("Ask why");
    await settle();
    const box = screen.getByRole("textbox", { name: "Question" });
    expect(box).toHaveValue("Why did this review fail?");
    expect(box).toHaveFocus();
    fireEvent.input(box, { target: { value: "Did the agent read the pull request?" } });
    flush();
    click("Ask");

    const answers = await screen.findByRole("list", { name: "Answers" });
    expect(pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "ask", args: [FAILED, "Did the agent read the pull request?"], confirmedFingerprint: null });
    expect(within(answers).getByText("Did the agent read the pull request?")).toBeInTheDocument();
    expect(answers).toHaveTextContent("The agent lost its connection before it read the pull request.");
    expect(screen.queryByRole("textbox", { name: "Question" })).not.toBeInTheDocument();
  });

  it("shows Whooper's own words when an action fails", async () => {
    renderFailed(FAILED, (request) => {
      switch (request.action) {
        case "show-review": return actionResult({ exitCode: 1, stderr: "whooper-show: no run 20261124T090000Z-55 in runs\n" });
        case "ask": return actionResult({ exitCode: 1, stderr: "whooper-ask: the ask run passed its 5-minute limit\n" });
        default: return undefined;
      }
    });
    expect(await screen.findByText("whooper-show: no run 20261124T090000Z-55 in runs")).toHaveAttribute("role", "alert");

    click("Ask why");
    click("Ask");
    expect(await screen.findByText("whooper-ask: the ask run passed its 5-minute limit")).toHaveAttribute("role", "alert");
    expect(screen.getByRole("textbox", { name: "Question" })).toHaveValue("Why did this review fail?");
    await waitFor(() => expect(screen.getByRole("button", { name: "Ask" })).toBeEnabled());
  });
});
