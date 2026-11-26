import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type { PluginActionRequest } from "../../bridge/types";
import { actionResult, fixturePlugin, fixtureReviews, fixtureStatus, pageActions } from "../../test/plugins";
import { axeViolations, render, settle } from "../../test/render";
import { ReviewScreen } from "./ReviewScreen";

const plugin = fixturePlugin({ actions: [...pageActions, { name: "skip", title: "Skip a review", description: "Marks a review done without submitting it.", confirm: false }] });
const base = fixtureReviews()[0]!;
/** Two runs of acme/widgets#101: review 1 of 1111111 and review 2 of 2222222, after which the pull request moved to 3333333. */
const runs = [
  { ...base, run_id: "r2", started_at: "2026-11-24T15:00:00Z", ended_at: "2026-11-24T15:20:00Z", head_sha: "2222222bbbbbbb", pr_head_sha: "3333333ccccccc", cause: null, skipped_by_user: false },
  { ...base, run_id: "r1", started_at: "2026-11-23T09:00:00Z", ended_at: "2026-11-23T09:20:00Z", needs_you: false, head_sha: "1111111aaaaaaa", pr_head_sha: "3333333ccccccc", cause: null, skipped_by_user: false },
];

function renderReview(runId: string, reviews: object[] = runs) {
  const pluginRequest = vi.fn(async (sent: PluginActionRequest) => {
    switch (sent.action) {
      case "retry": return actionResult({ stdout: "queued acme/widgets#101 for the next round\n" });
      case "round": return actionResult({ stdout: "acme/widgets#101: approve once CI passes\nround done\n" });
      case "skip": return actionResult({ stdout: sent.args.includes("--undo") ? "unskipped\n" : "skipped\n" });
      default: return actionResult({ stdout: "{}" });
    }
  });
  const onOpenReview = vi.fn();
  const refreshPlugins = vi.fn(async () => {});
  // The app's triage runs the plugin's action, then opens the next item, which this screen leaves to it.
  const triage = {
    skip: vi.fn(async (run: string) => void await pluginRequest({ plugin: "whooper", action: "skip", args: [run], confirmedFingerprint: null })),
    remove: vi.fn(async () => {}),
    restore: vi.fn(async () => {}),
  };
  render(() => (
    <ReviewScreen
      plugin={plugin}
      status={{ value: fixtureStatus("2026-11-26T23:45:00Z", { reviews }), error: null }}
      runId={runId}
      actions={{ pluginRequest, savePluginSettings: vi.fn() }}
      controls={{ refreshPlugins }}
      onPluginFilesChange={() => () => {}}
      onOpenReview={onOpenReview}
      onOpenCause={vi.fn()}
      triage={triage}
      onSubmitted={vi.fn()}
    />
  ));
  return { pluginRequest, onOpenReview, refreshPlugins, triage };
}

const click = (name: string) => {
  fireEvent.click(screen.getByRole("button", { name }));
  flush();
};

describe("a review's header", () => {
  it("shows the newest version with the commit it reviewed, and opens an earlier one from its menu", async () => {
    const { onOpenReview } = renderReview("r2");
    const menu = screen.getByRole("button", { name: /^Review 2 · 2222222 · Nov 24$/ });
    fireEvent.click(menu);
    const versions = await screen.findAllByRole("menuitemradio");
    expect(versions.map((item) => [item.textContent, item.getAttribute("aria-checked")])).toEqual([["Review 2 · 2222222 · Nov 24", "true"], ["Review 1 · 1111111 · Nov 23", "false"]]);
    fireEvent.click(versions[1]!);
    await waitFor(() => expect(onOpenReview).toHaveBeenCalledWith("r1"));
  });

  it("opens an earlier version read-only, without Rerun or Skip, and goes back to the newest", () => {
    const { onOpenReview } = renderReview("r1");
    expect(screen.getByRole("button", { name: /^Review 1 · 1111111 · Nov 23 · read-only$/ })).toBeInTheDocument();
    expect(screen.getByRole("note")).toHaveTextContent("Review 1 of 2, read-only: only the newest review can be submitted.");
    expect(screen.queryByRole("button", { name: "Rerun review" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Skip" })).not.toBeInTheDocument();
    click("Open review 2");
    expect(onOpenReview).toHaveBeenCalledWith("r2");
  });

  it("says when the pull request has commits after the reviewed one, and reruns it with retry, then offers Run now", async () => {
    const { pluginRequest, refreshPlugins } = renderReview("r2");
    const banner = screen.getByRole("note");
    expect(banner).toHaveTextContent("New commits since this review · it reviewed 2222222; the pull request is at 3333333.");
    fireEvent.click(screen.getAllByRole("button", { name: "Rerun review" })[1]!);
    expect(await screen.findByText("queued acme/widgets#101 for the next round")).toBeInTheDocument();
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "retry", args: ["r2"], confirmedFingerprint: null });
    expect(refreshPlugins).toHaveBeenCalled();
    click("Run now");
    expect(await screen.findByLabelText("What the round printed")).toHaveTextContent("round done");
    expect(pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "round", args: [], confirmedFingerprint: null });
  });

  it("skips the review with the skip action, and Undo runs it with --undo", async () => {
    const { pluginRequest } = renderReview("r2", [{ ...runs[0], pr_head_sha: runs[0]!.head_sha }, runs[1]]);
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
    click("Skip");
    expect(await screen.findByText("Skipped")).toBeInTheDocument();
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "skip", args: ["r2"], confirmedFingerprint: null });
    click("Undo");
    await waitFor(() => expect(pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "skip", args: ["r2", "--undo"], confirmedFingerprint: null }));
    expect(await screen.findByRole("button", { name: "Skip" })).toBeInTheDocument();
  });

  it("offers Retry review for a failed review", async () => {
    renderReview("r2", [{ ...runs[0], outcome: "failed", needs_you: false, findings: 0 }, runs[1]]);
    await settle();
    expect(screen.getByRole("button", { name: "Retry review" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Review failed" })).toBeInTheDocument();
    // The page's frame is not drawn for a failed review, so axe can check the whole screen.
    expect(await axeViolations()).toEqual([]);
  });

  it("offers neither Rerun nor Skip while the review runs", () => {
    renderReview("r2", [{ ...runs[0], outcome: "running", ended_at: null }, runs[1]]);
    expect(screen.getAllByText(/^Reviewing… started/).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /review$/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Skip" })).not.toBeInTheDocument();
  });
});
