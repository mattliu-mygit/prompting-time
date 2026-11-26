import type { PluginActions, PluginStatusRead } from "../../app/store";
import type { PluginActionResult, PluginSnapshot } from "../../bridge/types";
import { readReviews, submittedText, versionsOf, type ReviewEntry } from "../agents/reviews";
import { PROTOCOL_VERSION, readPageMessage, type DraftChange, type HostUpdate, type PageMessage, type QueueItem } from "./protocol";

/**
 * The actions a page of protocol 1 runs, by the names Whooper's manifest gives them. The status
 * comes from the store, which runs the status action.
 */
const SHOW_REVIEW = "show-review";
const DRAFT = "draft";
const SUBMIT = "submit";
const ASK = "ask";

export type PageBridgeOptions = {
  /** The plugin's latest snapshot: its title and actions, each with its confirm flag. */
  plugin(): PluginSnapshot;
  /** The plugin's latest status read, as the store holds it. */
  status(): PluginStatusRead | undefined;
  actions: PluginActions;
  /** Sends a message to the page's frame. */
  post(message: HostUpdate & { version: typeof PROTOCOL_VERSION; reply_to?: number }): void;
  /** Runs a confirm action after the user's confirmation: the run for real, or null when cancelled. */
  confirm(action: string, args: readonly string[]): Promise<PluginActionResult | null>;
  /** The page now shows this review. */
  onShowReview(runId: string): void;
  /** Why the page cannot show what the user opened, or null once it can. */
  onNotice(text: string | null): void;
};

export type PageBridge = {
  /** A message from the page's frame; one that does not fit the protocol is dropped. */
  receive(data: unknown): void;
  /** The review the user opened in Prompting Time, sent to the page once it is ready. */
  open(runId: string | null): void;
  /** The store read the plugin's status again. */
  statusChanged(): void;
  /** Files in the plugin's state folder changed: their paths, or null for any of them. */
  filesChanged(files: readonly string[] | null): void;
  dispose(): void;
};

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

function messageOf(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Prompting Time could not run the request.";
}

/**
 * The page's queue from the plugin's reviews: what waits on the user, what runs, and what ended. A
 * review waiting on a cause that blocks every review has no state in protocol 1, so it is left out.
 */
export function queueOf(reviews: readonly ReviewEntry[]): QueueItem[] {
  return reviews.flatMap((review): QueueItem[] => {
    const state: QueueItem["state"] | null = review.outcome === "waiting" ? null
      : review.outcome === "running" ? "reviewing"
      : review.outcome === "failed" ? "failed"
      : review.submitted ? "submitted"
      : review.needs_you ? "decide" : null;
    return state ? [{ run_id: review.run_id, pr: `${review.repo}#${review.pr}`, title: review.title, state, verdict: review.verdict, findings: review.findings }] : [];
  });
}

/** The draft action's arguments after the run ID for one change. */
function changeArgs(change: DraftChange): string[] {
  switch (change.op) {
    case "include":
    case "leave-out":
      return [change.op, change.finding];
    case "text":
      return ["text", change.finding, change.text];
    case "message":
      return ["message", change.text];
    default:
      return [change.op];
  }
}

/**
 * Prompting Time's side of a plugin page: it checks each message from the page, runs the plugin
 * action the request maps to, and answers. An action the manifest marks confirm runs only through
 * the confirmation. The page also hears, unasked, of a new status and of changes to the files of
 * the review it shows.
 */
export function createPageBridge(options: PageBridgeOptions): PageBridge {
  let ready = false;
  let disposed = false;
  let wanted: string | null = null;
  let shownRun: string | null = null;
  let lastStatus = "";
  let pendingRefresh: "drafts" | "review" | null = null;
  let refreshing = false;
  const title = () => options.plugin().title;

  function post(update: HostUpdate, replyTo?: number) {
    if (!disposed) options.post({ ...update, version: PROTOCOL_VERSION, ...(replyTo === undefined ? {} : { reply_to: replyTo }) });
  }

  function statusUpdate(): HostUpdate {
    const read = options.status();
    if (!read || read.error !== null || !isRecord(read.value)) throw new Error(read?.error ?? `${title()} has not reported its status yet.`);
    return { type: "status", status: read.value, queue: queueOf(readReviews(read.value)) };
  }

  /** The review when the plugin's status lists it; the page acts only on runs the plugin listed. */
  function listed(runId: string): ReviewEntry {
    const review = readReviews(options.status()?.value).find((entry) => entry.run_id === runId);
    if (!review) throw new Error(`${title()} does not list the review ${runId}.`);
    return review;
  }

  /** The review when it is the newest of its pull request's: only the newest can change or be submitted. */
  function newest(runId: string): ReviewEntry {
    const review = listed(runId);
    const versions = versionsOf(readReviews(options.status()?.value), review);
    if (versions.at(-1)?.run_id !== runId) {
      throw new Error(`Review ${versions.findIndex((entry) => entry.run_id === runId) + 1} of ${review.repo}#${review.pr} is an earlier version, so it is read-only: only the newest review, review ${versions.length}, can change or be submitted.`);
    }
    return review;
  }

  async function run(name: string, args: readonly string[]): Promise<PluginActionResult | null> {
    const plugin = options.plugin();
    const action = plugin.actions.find((item) => item.name === name);
    if (!action) throw new Error(`${plugin.title} has no ${name} action.`);
    if (action.confirm) return options.confirm(name, args);
    return options.actions.pluginRequest({ plugin: plugin.name, action: name, args: [...args], confirmedFingerprint: null });
  }

  function failure(result: PluginActionResult, name: string): Error {
    return new Error(result.stderr.trim() || result.stdout.trim() || `${name} exited with ${result.exitCode ?? "a signal"}.`);
  }

  async function runToEnd(name: string, args: readonly string[]): Promise<PluginActionResult> {
    const result = await run(name, args);
    if (result === null) throw new Error("Nothing ran: the confirmation was cancelled.");
    if (result.exitCode !== 0) throw failure(result, name);
    return result;
  }

  function printedObject(stdout: string, name: string): Record<string, unknown> {
    let value: unknown = null;
    try {
      value = JSON.parse(stdout);
    } catch {
      // Answered below.
    }
    if (!isRecord(value)) throw new Error(`${name} did not print JSON that Prompting Time reads.`);
    return value;
  }

  /** Reads the review with show-review and sends it; an opening also says what was submitted. */
  async function sendReview(runId: string, opening: boolean, replyTo?: number) {
    const review = listed(runId);
    const shown = printedObject((await runToEnd(SHOW_REVIEW, [runId])).stdout, SHOW_REVIEW);
    if (shown.version !== 1 || shown.run_id !== runId) throw new Error(`${SHOW_REVIEW} did not print review ${runId} in format version 1.`);
    if (!isRecord(shown.findings) || !isRecord(shown.drafts)) {
      throw new Error(typeof shown.failure === "string" && shown.failure.trim()
        ? `${title()}'s review failed: ${shown.failure.trim()}`
        : `${title()}'s review has no findings to show.`);
    }
    if (disposed) return;
    shownRun = runId;
    post({
      type: "review",
      review: {
        run_id: runId, repo: review.repo, pr: review.pr,
        title: typeof shown.title === "string" ? shown.title : review.title,
        author: typeof shown.author === "string" ? shown.author : null,
        findings: shown.findings, drafts: shown.drafts, patch: typeof shown.diff === "string" ? shown.diff : "",
      },
    }, replyTo);
    if (opening && review.submitted) post({ type: "submit-result", run_id: runId, posted: true, message: submittedText(review.submitted) });
    options.onNotice(null);
    options.onShowReview(runId);
  }

  function openWanted() {
    const runId = wanted;
    if (!ready || runId === null || runId === shownRun) return;
    sendReview(runId, true).catch((reason) => {
      if (!disposed && wanted === runId) options.onNotice(messageOf(reason));
    });
  }

  async function handle(message: PageMessage) {
    switch (message.type) {
      case "ready": {
        // A page that loads again starts with nothing on screen.
        ready = true;
        shownRun = null;
        let update: HostUpdate;
        try {
          update = statusUpdate();
        } catch (reason) {
          options.onNotice(messageOf(reason));
          throw reason;
        }
        lastStatus = JSON.stringify(update);
        post(update, message.id);
        openWanted();
        break;
      }
      case "open-review":
        await sendReview(message.run_id, true, message.id);
        break;
      case "change-draft": {
        const { run_id } = newest(message.run_id);
        const result = await runToEnd(DRAFT, [run_id, ...changeArgs(message.change)]);
        post({ type: "drafts", run_id, drafts: printedObject(result.stdout, DRAFT) }, message.id);
        break;
      }
      case "submit": {
        const { run_id } = newest(message.run_id);
        const result = await run(SUBMIT, [run_id]);
        if (result === null) post({ type: "submit-result", run_id, posted: false, message: "cancelled" }, message.id);
        else if (result.exitCode === 0) post({ type: "submit-result", run_id, posted: true, message: result.stdout.trim() || "submitted" }, message.id);
        else throw failure(result, SUBMIT);
        break;
      }
      case "ask": {
        const { run_id } = listed(message.run_id);
        const finding = message.finding === null ? [] : ["--finding", message.finding];
        const result = await runToEnd(ASK, [run_id, message.question, ...finding]);
        post({ type: "answer", run_id, finding: message.finding, question: message.question, answer: result.stdout.trim() }, message.id);
        break;
      }
    }
  }

  /** Sends what changed for the review on screen, one read at a time; a review read covers drafts. */
  async function drainRefreshes() {
    refreshing = true;
    while (pendingRefresh && !disposed && shownRun) {
      const kind = pendingRefresh;
      const runId = shownRun;
      pendingRefresh = null;
      try {
        if (kind === "review") await sendReview(runId, false);
        else {
          const result = await runToEnd(DRAFT, [runId]);
          if (shownRun === runId) post({ type: "drafts", run_id: runId, drafts: printedObject(result.stdout, DRAFT) });
        }
      } catch {
        // The page keeps what it has; the next change reads again.
      }
    }
    refreshing = false;
  }

  return {
    receive(data) {
      const message = readPageMessage(data);
      if (!message || disposed) return;
      handle(message).catch((reason) => post({ type: "error", message: messageOf(reason) }, message.id));
    },
    open(runId) {
      wanted = runId;
      openWanted();
    },
    statusChanged() {
      if (!ready || disposed) return;
      try {
        const update = statusUpdate();
        const text = JSON.stringify(update);
        if (text === lastStatus) return;
        lastStatus = text;
        post(update);
      } catch {
        // Nothing new to send.
      }
    },
    filesChanged(files) {
      if (!ready || disposed || !shownRun) return;
      const folder = `runs/${shownRun}/`;
      const changed = files === null ? null : files.filter((file) => file.startsWith(folder));
      if (changed?.length === 0) return;
      const kind = changed?.every((file) => file === `${folder}drafts.json`) ? "drafts" : "review";
      pendingRefresh = pendingRefresh === "review" ? "review" : kind;
      if (!refreshing) void drainRefreshes();
    },
    dispose() {
      disposed = true;
    },
  };
}
