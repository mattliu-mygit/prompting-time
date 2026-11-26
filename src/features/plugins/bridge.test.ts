import { describe, expect, it, vi } from "vitest";
import type { PluginStatusRead } from "../../app/store";
import type { PluginActionRequest, PluginActionResult } from "../../bridge/types";
import { actionResult, fixturePlugin, fixtureReviews, fixtureShown, fixtureStatus, pageActions } from "../../test/plugins";
import { createPageBridge, queueOf } from "./bridge";
import { readReviews } from "../agents/reviews";

const OPEN = "20261126T150000Z-101";
const SUBMITTED = "20261125T100000Z-77";
const FAILED = "20261124T090000Z-55";

function setup(options: { actions?: typeof pageActions; status?: PluginStatusRead; confirm?: (action: string, args: readonly string[]) => Promise<PluginActionResult | null> } = {}) {
  const posted: Record<string, unknown>[] = [];
  const notices: (string | null)[] = [];
  const shown: string[] = [];
  const pluginRequest = vi.fn(async (request: PluginActionRequest): Promise<PluginActionResult> => {
    const [runId] = request.args;
    switch (request.action) {
      case "show-review":
        return runId === FAILED
          ? actionResult({ stdout: JSON.stringify(fixtureShown(runId, { outcome: "failed", findings: null, drafts: null, failure: "the agent stopped" })) })
          : actionResult({ stdout: JSON.stringify(fixtureShown(runId!)) });
      case "draft":
        if (request.args[1] === "text" && request.args[3] === "") return actionResult({ exitCode: 1, stderr: "whooper-draft: the text is empty" });
        return actionResult({ stdout: JSON.stringify({ event: request.args[1] === "comment" ? "COMMENT" : "APPROVE", message: "", findings: {} }) });
      case "ask":
        return actionResult({ stdout: "An invented answer.\n" });
      default:
        throw new Error(`unexpected ${request.action}`);
    }
  });
  const confirm = vi.fn(options.confirm ?? (async () => actionResult({ stdout: "posted the review: https://example.invalid/review/1\n" })));
  let status: PluginStatusRead = options.status ?? { value: fixtureStatus("2026-11-26T15:45:00Z", { reviews: fixtureReviews() }), error: null };
  const plugin = fixturePlugin({ actions: options.actions ?? pageActions });
  const bridge = createPageBridge({
    plugin: () => plugin,
    status: () => status,
    actions: { pluginRequest, savePluginSettings: vi.fn() },
    post: (message) => posted.push(message),
    confirm,
    onShowReview: (runId) => shown.push(runId),
    onNotice: (text) => notices.push(text),
  });
  return {
    bridge, posted, notices, shown, pluginRequest, confirm,
    setStatus(next: PluginStatusRead) { status = next; },
    /** Sends a request from the page and waits for what it starts. */
    async send(message: Record<string, unknown>) {
      bridge.receive({ version: 1, ...message });
      for (let round = 0; round < 10; round += 1) await Promise.resolve();
    },
  };
}

describe("the page bridge", () => {
  it("drops messages that do not fit protocol 1 and runs nothing for them", async () => {
    const page = setup();
    for (const message of [
      "ready",
      null,
      [],
      { type: "ready", id: 1 },
      { version: 2, type: "ready", id: 1 },
      { version: 1, type: "ready" },
      { version: 1, type: "ready", id: "1" },
      { version: 1, type: "delete-everything", id: 1 },
      { version: 1, type: "toString", id: 1 },
      { version: 1, type: "submit", id: 1 },
      { version: 1, type: "submit", id: 1, run_id: 101 },
      { version: 1, type: "ask", id: 1, run_id: OPEN, question: "Why?" },
      { version: 1, type: "change-draft", id: 1, run_id: OPEN, change: { op: "delete", finding: "1" } },
      { version: 1, type: "change-draft", id: 1, run_id: OPEN, change: { op: "text", finding: "1" } },
      { version: 1, type: "change-draft", id: 1, run_id: OPEN, change: ["include", "1"] },
    ]) {
      page.bridge.receive(message);
    }
    await page.send({ type: "nothing", id: 2 });
    expect(page.posted).toEqual([]);
    expect(page.pluginRequest).not.toHaveBeenCalled();
    expect(page.confirm).not.toHaveBeenCalled();
  });

  it("answers ready with the status and the queue, then sends the review the user opened", async () => {
    const page = setup();
    page.bridge.open(SUBMITTED);
    expect(page.pluginRequest).not.toHaveBeenCalled();

    await page.send({ type: "ready", id: 1 });

    const [status, review, submitted] = page.posted;
    expect(status).toMatchObject({ version: 1, type: "status", reply_to: 1 });
    expect((status as { queue: unknown }).queue).toEqual([
      { run_id: OPEN, pr: "acme/widgets#101", title: "separate cache configuration", state: "decide", verdict: "approve_once_ci_passes", findings: 2 },
      { run_id: "20261126T140000Z-102", pr: "acme/widgets#102", title: null, state: "reviewing", verdict: null, findings: 0 },
      { run_id: SUBMITTED, pr: "acme/gadgets#77", title: "add the tags package", state: "submitted", verdict: "look_first", findings: 1 },
      { run_id: FAILED, pr: "acme/gadgets#55", title: "move the projection", state: "failed", verdict: null, findings: 0 },
    ]);
    expect(page.pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "show-review", args: [SUBMITTED], confirmedFingerprint: null });
    expect(review).toMatchObject({
      version: 1, type: "review",
      review: { run_id: SUBMITTED, repo: "acme/gadgets", pr: 77, title: "add the tags package", author: null, patch: "diff --git a/src/a.rs b/src/a.rs\n" },
    });
    expect(review).not.toHaveProperty("reply_to");
    expect(submitted).toMatchObject({ type: "submit-result", run_id: SUBMITTED, posted: true });
    expect((submitted as { message: string }).message).toMatch(/^commented (on|at) /);
    expect(page.shown).toEqual([SUBMITTED]);
  });

  it("opens a review the page asks for, and refuses a run the plugin does not list", async () => {
    const page = setup();
    await page.send({ type: "ready", id: 1 });
    await page.send({ type: "open-review", id: 2, run_id: OPEN });
    expect(page.posted.at(-1)).toMatchObject({ type: "review", reply_to: 2, review: { run_id: OPEN, pr: 101 } });

    page.pluginRequest.mockClear();
    await page.send({ type: "open-review", id: 3, run_id: "../../elsewhere" });
    await page.send({ type: "change-draft", id: 4, run_id: "--approve-anyway", change: { op: "approve" } });
    expect(page.pluginRequest).not.toHaveBeenCalled();
    expect(page.posted.slice(-2)).toEqual([
      { version: 1, type: "error", reply_to: 3, message: "Whooper does not list the review ../../elsewhere." },
      { version: 1, type: "error", reply_to: 4, message: "Whooper does not list the review --approve-anyway." },
    ]);
  });

  it("says why a failed review cannot be shown, to the page and above it", async () => {
    const page = setup();
    page.bridge.open(FAILED);
    await page.send({ type: "ready", id: 1 });
    expect(page.notices.at(-1)).toBe("Whooper's review failed: the agent stopped");
    await page.send({ type: "open-review", id: 2, run_id: FAILED });
    expect(page.posted.at(-1)).toEqual({ version: 1, type: "error", reply_to: 2, message: "Whooper's review failed: the agent stopped" });
  });

  it("maps each draft change to the draft action's arguments and answers with the drafts it prints", async () => {
    const page = setup();
    await page.send({ type: "ready", id: 1 });
    const changes: [Record<string, unknown>, string[]][] = [
      [{ op: "include", finding: "1" }, ["include", "1"]],
      [{ op: "leave-out", finding: "1" }, ["leave-out", "1"]],
      [{ op: "text", finding: "1", text: "**nit:** shorter" }, ["text", "1", "**nit:** shorter"]],
      [{ op: "approve" }, ["approve"]],
      [{ op: "comment" }, ["comment"]],
      [{ op: "message", text: "Thanks" }, ["message", "Thanks"]],
    ];
    for (const [index, [change, args]] of changes.entries()) {
      await page.send({ type: "change-draft", id: 10 + index, run_id: OPEN, change });
      expect(page.pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "draft", args: [OPEN, ...args], confirmedFingerprint: null });
      expect(page.posted.at(-1)).toMatchObject({ type: "drafts", run_id: OPEN, reply_to: 10 + index });
    }
    expect(page.posted.at(-2)).toMatchObject({ drafts: { event: "COMMENT" } });

    await page.send({ type: "change-draft", id: 20, run_id: OPEN, change: { op: "text", finding: "1", text: "" } });
    expect(page.posted.at(-1)).toEqual({ version: 1, type: "error", reply_to: 20, message: "whooper-draft: the text is empty" });
  });

  it("submits only through the confirmation and returns what happened to the page", async () => {
    const results: (PluginActionResult | null)[] = [
      actionResult({ stdout: "posted the review: https://example.invalid/review/1\n" }),
      null,
      actionResult({ exitCode: 10, stderr: "whooper-submit: nothing posted: the PR has new commits" }),
    ];
    const page = setup({ confirm: async () => results.shift() ?? null });
    await page.send({ type: "ready", id: 1 });

    await page.send({ type: "submit", id: 2, run_id: OPEN });
    expect(page.confirm).toHaveBeenCalledWith("submit", [OPEN]);
    expect(page.pluginRequest.mock.calls.some(([request]) => request.action === "submit")).toBe(false);
    expect(page.posted.at(-1)).toEqual({ version: 1, type: "submit-result", reply_to: 2, run_id: OPEN, posted: true, message: "posted the review: https://example.invalid/review/1" });

    await page.send({ type: "submit", id: 3, run_id: OPEN });
    expect(page.posted.at(-1)).toEqual({ version: 1, type: "submit-result", reply_to: 3, run_id: OPEN, posted: false, message: "cancelled" });

    await page.send({ type: "submit", id: 4, run_id: OPEN });
    expect(page.posted.at(-1)).toEqual({ version: 1, type: "error", reply_to: 4, message: "whooper-submit: nothing posted: the PR has new commits" });
  });

  it("runs any action the manifest marks confirm through the confirmation, and others directly", async () => {
    const actions = pageActions.map((action) => (action.name === "draft" ? { ...action, confirm: true } : { ...action, confirm: action.name === "submit" ? false : action.confirm }));
    const page = setup({ actions, confirm: async () => actionResult({ stdout: JSON.stringify({ event: "APPROVE", message: "", findings: {} }) }) });
    page.pluginRequest.mockImplementation(async (request) => actionResult({ stdout: request.action === "submit" ? "posted\n" : "{}" }));
    await page.send({ type: "ready", id: 1 });

    await page.send({ type: "change-draft", id: 2, run_id: OPEN, change: { op: "approve" } });
    expect(page.confirm).toHaveBeenCalledWith("draft", [OPEN, "approve"]);
    expect(page.pluginRequest.mock.calls.some(([request]) => request.action === "draft")).toBe(false);

    await page.send({ type: "submit", id: 3, run_id: OPEN });
    expect(page.confirm).toHaveBeenCalledTimes(1);
    expect(page.pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "submit", args: [OPEN], confirmedFingerprint: null });
  });

  it("answers an ask with what the ask action prints", async () => {
    const page = setup();
    await page.send({ type: "ready", id: 1 });

    await page.send({ type: "ask", id: 2, run_id: OPEN, question: "Why is this a nit?", finding: "1" });
    expect(page.pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "ask", args: [OPEN, "Why is this a nit?", "--finding", "1"], confirmedFingerprint: null });
    expect(page.posted.at(-1)).toEqual({ version: 1, type: "answer", reply_to: 2, run_id: OPEN, finding: "1", question: "Why is this a nit?", answer: "An invented answer." });

    await page.send({ type: "ask", id: 3, run_id: OPEN, question: "Why did it pass?", finding: null });
    expect(page.pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "ask", args: [OPEN, "Why did it pass?"], confirmedFingerprint: null });
  });

  it("sends the shown review's drafts or the review again when its files change, and a new status", async () => {
    const page = setup();
    page.bridge.filesChanged(null);
    page.bridge.statusChanged();
    expect(page.posted).toEqual([]);
    page.bridge.open(OPEN);
    await page.send({ type: "ready", id: 1 });
    page.posted.length = 0;
    page.pluginRequest.mockClear();

    page.bridge.filesChanged(["runs/20261125T100000Z-77/drafts.json", "poll/github.json"]);
    await page.send({ type: "nothing", id: 0 });
    expect(page.pluginRequest).not.toHaveBeenCalled();

    page.bridge.filesChanged([`runs/${OPEN}/drafts.json`]);
    await page.send({ type: "nothing", id: 0 });
    expect(page.pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "draft", args: [OPEN], confirmedFingerprint: null });
    expect(page.posted.at(-1)).toMatchObject({ type: "drafts", run_id: OPEN });
    expect(page.posted.at(-1)).not.toHaveProperty("reply_to");

    page.bridge.filesChanged([`runs/${OPEN}/drafts.json`, `runs/${OPEN}/asks/1.json`]);
    await page.send({ type: "nothing", id: 0 });
    expect(page.pluginRequest).toHaveBeenLastCalledWith({ plugin: "whooper", action: "show-review", args: [OPEN], confirmedFingerprint: null });
    expect(page.posted.at(-1)).toMatchObject({ type: "review", review: { run_id: OPEN } });

    page.bridge.filesChanged(null);
    await page.send({ type: "nothing", id: 0 });
    expect(page.posted.at(-1)).toMatchObject({ type: "review", review: { run_id: OPEN } });

    page.posted.length = 0;
    page.bridge.statusChanged();
    expect(page.posted).toEqual([]);
    const reviews = fixtureReviews().map((review) => (review.run_id === OPEN ? { ...review, needs_you: false, submitted: { event: "APPROVE" as const, at: "2026-11-26T16:00:00Z" } } : review));
    page.setStatus({ value: fixtureStatus("2026-11-26T15:45:00Z", { reviews }), error: null });
    page.bridge.statusChanged();
    expect(page.posted).toHaveLength(1);
    expect(page.posted[0]).toMatchObject({ type: "status", queue: expect.arrayContaining([expect.objectContaining({ run_id: OPEN, state: "submitted" })]) });
  });

  it("puts only waiting, running, failed, and submitted reviews in the page's queue", () => {
    const reviews = readReviews({ reviews: [...fixtureReviews(), { ...fixtureReviews()[0], run_id: "old", needs_you: false }, { run_id: "broken" }] });
    expect(reviews.map(({ run_id }) => run_id)).not.toContain("broken");
    expect(queueOf(reviews).map(({ run_id }) => run_id)).toEqual([OPEN, "20261126T140000Z-102", SUBMITTED, FAILED]);
  });
});
