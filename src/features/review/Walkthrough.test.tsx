import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PluginActionRequest } from "../../bridge/types";
import { actionResult, fixturePlugin, fixtureReviews, fixtureStatus, fixtureWalkthrough, pageActions } from "../../test/plugins";
import { render, settle } from "../../test/render";
import { ReviewScreen } from "../agents/ReviewScreen";
import { createReviewViews } from "./views";

const plugin = fixturePlugin({ actions: [...pageActions, { name: "skip", title: "Skip a review", description: "Marks a review done.", confirm: false }] });
const base = fixtureReviews()[0]!;
const RUN = base.run_id;
/** acme/widgets#101, review 2 of 2252c0f…, which says approve once CI passes; the pull request moved to cccc2222dddd after it. */
const review = { ...base, head_sha: "2252c0f0000", pr_head_sha: "cccc2222dddd", cause: null, skipped_by_user: false };
/** Review 1 of the same pull request, which said look first. */
const earlier = { ...base, run_id: "r-earlier", started_at: "2026-11-24T09:00:00Z", ended_at: "2026-11-24T09:20:00Z", verdict: "look_first", findings: 1, needs_you: false, head_sha: "1111111aaaa", pr_head_sha: "cccc2222dddd", safe_to_approve: false, cause: null, skipped_by_user: false };

function renderWalkthrough(options: { shown?: object; entry?: object } = {}) {
  const pluginRequest = vi.fn(async (sent: PluginActionRequest) => {
    switch (sent.action) {
      case "show-review": return actionResult({ stdout: JSON.stringify(options.shown ?? fixtureWalkthrough(sent.args[0]!)) });
      case "retry": return actionResult({ stdout: "queued acme/widgets#101 for the next round\n" });
      case "submit": return sent.confirmedFingerprint === null
        ? actionResult({ stdout: `printed the request\nfingerprint: ${"ab".repeat(32)}\n`, fingerprint: "ab".repeat(32), preview: '{ "event": "APPROVE" }' })
        : actionResult({ stdout: "posted the review\n" });
      default: return actionResult({ stdout: "{}" });
    }
  });
  const onNotice = vi.fn();
  const onSubmitted = vi.fn();
  // One store of views for the render: a prop written as a call would make a new one on every read.
  const views = createReviewViews();
  render(() => (
    <ReviewScreen
      plugin={plugin}
      status={{ value: fixtureStatus("2026-11-26T23:45:00Z", { reviews: [options.entry ?? review, earlier] }), error: null }}
      runId={RUN}
      actions={{ pluginRequest, savePluginSettings: vi.fn() }}
      controls={{ refreshPlugins: vi.fn(async () => {}) }}
      onPluginFilesChange={() => () => {}}
      onOpenReview={vi.fn()}
      onOpenCause={vi.fn()}
      triage={{ skip: vi.fn(async () => {}), remove: vi.fn(async () => {}), restore: vi.fn(async () => {}) }}
      onSubmitted={onSubmitted}
      views={views}
      onNotice={onNotice}
    />
  ));
  return { pluginRequest, onNotice, onSubmitted };
}

const walkthrough = () => screen.findByRole("navigation", { name: "Walkthrough" });
const placeCard = () => document.querySelector<HTMLElement>(".place-card");

afterEach(() => {
  localStorage.clear();
  document.body.classList.remove("review-dimming");
});

describe("a review's walkthrough", () => {
  it("opens on the walkthrough with its navigator, headed by the verdict, and Diff shows the page under the same top", async () => {
    renderWalkthrough();
    await walkthrough();
    const verdict = screen.getAllByRole("region", { name: "Verdict" })[0]!;
    expect(verdict).toHaveAttribute("data-tone", "success");
    expect(verdict).toHaveTextContent("Approve once CI passes");
    expect(verdict).toHaveTextContent("The policy is small and its tests pass.");
    expect(verdict).toHaveTextContent("Review 2 · 2252c0f · 1 draft comment");
    expect(screen.getByRole("button", { name: "Walkthrough" })).toHaveAttribute("aria-pressed", "true");
    expect(document.querySelector(".review-page")).toHaveAttribute("hidden");

    fireEvent.click(screen.getByRole("button", { name: "Diff" }));
    flush();
    expect(document.querySelector(".review-page")).not.toHaveAttribute("hidden");
    expect(screen.queryByRole("navigation", { name: "Walkthrough" })).not.toBeInTheDocument();
    expect(within(document.querySelector<HTMLElement>(".review-page-top")!).getByRole("region", { name: "Verdict" })).toHaveTextContent("Approve once CI passes");
  });

  it("tints the verdict orange for a review that needs a look", async () => {
    renderWalkthrough({ entry: { ...review, verdict: "look_first", safe_to_approve: false } });
    await walkthrough();
    expect(screen.getByRole("region", { name: "Verdict" })).toHaveAttribute("data-tone", "attention");
    expect(screen.getByRole("region", { name: "Verdict" })).toHaveTextContent("Look first");
  });

  it("says the commits since the review in one folded line with Rerun, and × hides it for this version of the pull request", async () => {
    const { pluginRequest, onNotice } = renderWalkthrough();
    await walkthrough();
    const line = screen.getByRole("button", { name: /^2 new commits since this review/ });
    expect(line).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("client: log each retry")).not.toBeInTheDocument();
    fireEvent.click(line);
    flush();
    expect(line).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("client: log each retry")).toBeInTheDocument();
    expect(screen.getByText(/You haven't opened this pull request's review before\./)).toBeInTheDocument();

    fireEvent.click(within(screen.getByRole("region", { name: "Changes since you last looked" })).getByRole("button", { name: "Rerun review" }));
    await waitFor(() => expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "retry", args: [RUN], confirmedFingerprint: null }));

    fireEvent.click(screen.getByRole("button", { name: "Hide until the pull request gets new commits" }));
    flush();
    expect(screen.queryByRole("region", { name: "Changes since you last looked" })).not.toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem("prompting-time.review-since-hidden")!)).toEqual({ "acme/widgets#101": `${RUN}@cccc2222dddd` });
    expect(onNotice).toHaveBeenCalledWith("Hidden until the pull request gets new commits.", expect.any(Function));
    onNotice.mock.calls[0]![1]();
    flush();
    expect(screen.getByRole("region", { name: "Changes since you last looked" })).toBeInTheDocument();
  });

  it("names the version you last opened, which Prompting Time remembers, and comes back when the pull request moves", async () => {
    localStorage.setItem("prompting-time.review-last-opened", JSON.stringify({ "acme/widgets#101": { run: "r-earlier", head: "aaaa1111bbbb", at: "2026-11-24T10:00:00Z" } }));
    // Hidden for an older head of the pull request, so the new commits bring the line back.
    localStorage.setItem("prompting-time.review-since-hidden", JSON.stringify({ "acme/widgets#101": `${RUN}@aaaa1111bbbb` }));
    renderWalkthrough();
    await walkthrough();
    const line = screen.getByRole("button", { name: /^2 new commits since this review · 1 since you last looked/ });
    fireEvent.click(line);
    flush();
    expect(screen.getByText(/You last opened review 1 \(1111111\) on Nov 24, which said look first with 1 finding\./)).toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem("prompting-time.review-last-opened")!)["acme/widgets#101"]).toMatchObject({ run: RUN, head: "cccc2222dddd" });
  });

  it("steps through the places to look with j and k, dims the rest until the mouse moves, and opens each place's reason", async () => {
    renderWalkthrough();
    const nav = await walkthrough();
    fireEvent.keyDown(document.body, { key: "j" });
    flush();
    expect(within(nav).getByRole("button", { name: /^1The delay doubles without a cap/ })).toHaveAttribute("aria-current", "true");
    expect(document.querySelector(".walkthrough")).toHaveAttribute("data-dim");
    expect(document.body).toHaveClass("review-dimming");
    await waitFor(() => expect(placeCard()).toHaveTextContent("After 20 attempts the delay is hours."), { timeout: 10_000 });
    expect(placeCard()).toHaveTextContent("Read, not run · read delay");

    fireEvent.mouseMove(window, { clientX: 40, clientY: 40 });
    flush();
    expect(document.querySelector(".walkthrough")).not.toHaveAttribute("data-dim");
    fireEvent.keyDown(document.body, { key: "j" });
    flush();
    expect(within(nav).getByRole("button", { name: /^2Five attempts, not three/ })).toHaveAttribute("aria-current", "true");
    fireEvent.keyDown(document.body, { key: "k" });
    flush();
    expect(within(nav).getByRole("button", { name: /^1The delay doubles/ })).toHaveAttribute("aria-current", "true");
    fireEvent.keyDown(document.body, { key: "Escape" });
    flush();
    expect(document.querySelector(".walkthrough")).not.toHaveAttribute("data-dim");
  });

  it("draws the calls before and after, struck where removed, and a call opens its line in the code", async () => {
    const scrolled = vi.fn();
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled(this.getAttribute("data-column-number"));
    };
    renderWalkthrough();
    await walkthrough();
    expect(screen.getByRole("group", { name: "A failed upload: calls before and after this change" })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "client to server: try_once, removed" })).toHaveAttribute("data-status", "removed");
    expect(screen.getByRole("button", { name: "policy to policy: sleep(0), now delay(n), changed" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "client to policy: retry, new" }));
    await waitFor(() => expect(scrolled).toHaveBeenCalledWith("3"), { timeout: 10_000 });
    // @ts-expect-error jsdom has no scrollIntoView; the test put one in for itself.
    delete Element.prototype.scrollIntoView;
  });

  it("folds a layer you mark read to one line, and opens it again on a click", async () => {
    renderWalkthrough();
    await walkthrough();
    const layer = screen.getByRole("region", { name: "Layer 1: The retry policy" });
    fireEvent.click(within(layer).getByRole("button", { name: "Mark layer 1 read" }));
    flush();
    const folded = within(layer).getByRole("button", { expanded: false });
    expect(folded).toHaveTextContent("The retry policy");
    expect(folded).toHaveTextContent("1 file · read");
    expect(screen.getByRole("button", { name: "Layer 1 read" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(folded);
    flush();
    expect(within(layer).getByRole("button", { name: "Layer 1 read · undo" })).toBeInTheDocument();
  });

  it("lists what Whooper checked and didn't, then the files that usually change with these", async () => {
    renderWalkthrough();
    await walkthrough();
    expect(screen.getByText("Ran the client tests at head and base")).toBeInTheDocument();
    expect(screen.getByText("Didn't run the upload integration suite")).toBeInTheDocument();
    expect(screen.getByLabelText("Files that usually change with these")).toHaveTextContent("src/server.rs changed with client.rs in 80% of 10 commits");
  });

  it("turns a view off for every review from the Views menu, and shows no summary for a view without data", async () => {
    renderWalkthrough({ shown: fixtureWalkthrough(RUN, { co_change: [] }) });
    await walkthrough();
    expect(screen.queryByLabelText("Files that usually change with these")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^Views/ }));
    const calls = await screen.findByRole("menuitemcheckbox", { name: /^Calls before and after/ });
    expect(screen.getByRole("menuitemcheckbox", { name: /^Files that usually change with these/ })).toHaveTextContent("Nothing for this review.");
    expect(screen.queryByRole("menuitemcheckbox", { name: /How the changed code connects/ })).not.toBeInTheDocument();
    fireEvent.click(calls);
    await waitFor(() => expect(localStorage.getItem("prompting-time.review-views-off")).toBe('["calls"]'));
    await waitFor(() => expect(screen.queryByRole("group", { name: /calls before and after this change/ })).not.toBeInTheDocument());
    expect(JSON.parse(localStorage.getItem("prompting-time.review-views-off")!)).toEqual(["calls"]);
    expect(screen.getByRole("button", { name: /^Views · 1 off/ })).toBeInTheDocument();
  });

  it("waits as Decide… until clicked, then submits through the draft action and the confirmation", async () => {
    const { pluginRequest, onSubmitted } = renderWalkthrough();
    await walkthrough();
    expect(screen.queryByRole("group", { name: "Your decision" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Decide…" }));
    flush();
    const bar = screen.getByRole("group", { name: "Your decision" });
    expect(bar).toHaveTextContent("1 draft comment, 1 included");
    expect(within(bar).getByRole("radio", { name: "Approve" })).toBeChecked();
    fireEvent.click(within(bar).getByRole("button", { name: "Submit review" }));
    await waitFor(() => expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "draft", args: [RUN, "approve"], confirmedFingerprint: null }));
    const dialog = await screen.findByRole("alertdialog", { name: "Submit review" });
    await waitFor(() => expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "submit", args: [RUN], confirmedFingerprint: null }));
    fireEvent.click(await within(dialog).findByRole("button", { name: "Submit review" }));
    await waitFor(() => expect(onSubmitted).toHaveBeenCalledWith(RUN));
  });

  it("opens the decision bar once you scroll to the end", async () => {
    renderWalkthrough();
    await walkthrough();
    const scroller = document.querySelector<HTMLElement>(".walk-scroll")!;
    Object.defineProperties(scroller, { scrollHeight: { value: 2000 }, clientHeight: { value: 800 }, scrollTop: { value: 1200, writable: true } });
    fireEvent.scroll(scroller);
    flush();
    expect(screen.getByRole("group", { name: "Your decision" })).toBeInTheDocument();
  });

  it("shows the page as today when the plugin wrote no overview", async () => {
    renderWalkthrough({ shown: fixtureWalkthrough(RUN, { overview: null }) });
    await settle();
    await settle();
    expect(screen.queryByRole("navigation", { name: "Walkthrough" })).not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Show the review as" })).not.toBeInTheDocument();
    expect(document.querySelector(".review-page")).not.toHaveAttribute("hidden");
    expect(screen.getByRole("note")).toHaveTextContent("New commits since this review");
  });
});
