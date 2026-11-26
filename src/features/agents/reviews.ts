import type { Status } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginsSnapshot } from "../../app/store";

/** Why a review failed or waits, as the plugin names it: a cause that blocks every review ("all"), such as Docker not running, or one pull request's ("pr"). */
export type ReviewCause = { id: string; summary: string; scope: "all" | "pr" };

/**
 * One review in the `reviews` of `whooper-status --json`: one run of Whooper's review of a pull
 * request in the last 30 days, newest first. `pr` is the pull request's number in `repo`, and times
 * are ISO 8601. Whooper decides `needs_you`: whether the review waits on the user. A rerun is a new
 * run of the same pull request: the newest is the review the lists show, the rest are its earlier
 * versions. `head_sha` is the commit the run reviewed and `pr_head_sha` the pull request's head as
 * Whooper last saw it. A run "waiting" did not start because a cause that blocks every review
 * stopped its round. `skipped_by_user` says the user skipped it; `skipped` that Whooper's review
 * skipped the change. The fields after `needs_you` came later in format version 1, so an entry
 * without them reads as having none.
 */
export type ReviewEntry = {
  run_id: string;
  repo: string;
  pr: number;
  title: string | null;
  url: string | null;
  started_at: string;
  ended_at: string | null;
  outcome: "running" | "reviewed" | "failed" | "waiting";
  verdict: string | null;
  skipped: boolean;
  findings: number;
  submitted: { event: "APPROVE" | "COMMENT"; at: string } | null;
  needs_you: boolean;
  head_sha: string | null;
  pr_head_sha: string | null;
  cause: ReviewCause | null;
  skipped_by_user: boolean;
};

/** A review and the plugin that lists it. */
export type PluginReview = { plugin: string; review: ReviewEntry };

const OUTCOMES = new Set(["running", "reviewed", "failed", "waiting"]);

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const stringOrNull = (value: unknown) => value === null || typeof value === "string";
const optional = (value: unknown, fits: (value: unknown) => boolean) => value === undefined || fits(value);

/** A cause the plugin printed, or null when it does not fit. */
export function readCause(value: unknown): ReviewCause | null {
  return isRecord(value) && typeof value.id === "string" && value.id !== "" && typeof value.summary === "string" && (value.scope === "all" || value.scope === "pr")
    ? { id: value.id, summary: value.summary, scope: value.scope }
    : null;
}

function fitsReview(value: unknown): value is Omit<ReviewEntry, "head_sha" | "pr_head_sha" | "cause" | "skipped_by_user"> & Partial<ReviewEntry> {
  if (!isRecord(value)) return false;
  const submitted = value.submitted;
  return typeof value.run_id === "string" && value.run_id !== ""
    && typeof value.repo === "string" && typeof value.pr === "number"
    && stringOrNull(value.title) && stringOrNull(value.url)
    && typeof value.started_at === "string" && stringOrNull(value.ended_at)
    && typeof value.outcome === "string" && OUTCOMES.has(value.outcome)
    && stringOrNull(value.verdict) && typeof value.skipped === "boolean"
    && typeof value.findings === "number" && typeof value.needs_you === "boolean"
    && (submitted === null || (isRecord(submitted) && (submitted.event === "APPROVE" || submitted.event === "COMMENT") && typeof submitted.at === "string"))
    && optional(value.head_sha, stringOrNull) && optional(value.pr_head_sha, stringOrNull)
    && optional(value.cause, (cause) => cause === null || readCause(cause) !== null)
    && optional(value.skipped_by_user, (skipped) => typeof skipped === "boolean");
}

/** The reviews a plugin's status lists, leaving out any entry that does not fit the format. */
export function readReviews(status: unknown): ReviewEntry[] {
  if (!isRecord(status) || !Array.isArray(status.reviews)) return [];
  return status.reviews.filter(fitsReview).map((review) => ({
    ...review,
    head_sha: review.head_sha ?? null,
    pr_head_sha: review.pr_head_sha ?? null,
    cause: readCause(review.cause),
    skipped_by_user: review.skipped_by_user ?? false,
  }));
}

/** The plugins' reviews by plugin, from each plugin's latest status. */
export function pluginReviews(plugins: PluginsSnapshot): Record<string, ReviewEntry[]> {
  return Object.fromEntries(plugins.items.map((plugin: PluginSnapshot) => [plugin.name, readReviews(plugins.statuses[plugin.name]?.value)]));
}

const pullRequestOf = (review: ReviewEntry) => `${review.repo}#${review.pr}`;
const newestFirst = (left: ReviewEntry, right: ReviewEntry) => right.started_at.localeCompare(left.started_at);

/** The newest run of each pull request, newest first: the reviews the lists show. */
export function newestRuns(reviews: readonly ReviewEntry[]): ReviewEntry[] {
  const seen = new Set<string>();
  return [...reviews].sort(newestFirst).filter((review) => {
    const key = pullRequestOf(review);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** Every run of the review's pull request, oldest first, so the first is "Review 1". */
export function versionsOf(reviews: readonly ReviewEntry[], review: ReviewEntry): ReviewEntry[] {
  return reviews.filter((entry) => pullRequestOf(entry) === pullRequestOf(review)).sort((left, right) => newestFirst(right, left));
}

/** Whether the user is done with the review: submitted, skipped, or finished without needing them. */
export function isFinished(review: ReviewEntry): boolean {
  return review.submitted !== null || review.skipped_by_user || (review.outcome === "reviewed" && !review.needs_you);
}

/** A finished review that waits on the user's decision: what "Needs you" lists. */
export function readyToDecide(review: ReviewEntry): boolean {
  return review.outcome === "reviewed" && review.needs_you && !isFinished(review);
}

/**
 * Where All work lists a pull request's newest run: under "Needs you" while it waits on a decision,
 * in Done once the user finished with it before today, and otherwise in its repository's group:
 * running, waiting, failed, or finished today.
 */
export function reviewPlace(review: ReviewEntry, now = new Date()): "needs-you" | "project" | "done" {
  if (readyToDecide(review)) return "needs-you";
  if (isFinished(review) && new Date(reviewTime(review)).toDateString() !== now.toDateString()) return "done";
  return "project";
}

/** A cause that blocks every review, with the newest runs it stopped, waiting or failed, newest first. */
export type BlockingCause = { id: string; summary: string; since: string | null; reviews: ReviewEntry[] };

/**
 * One entry per cause that blocks every review: the cause the status reports in `blocked`, even
 * with no review waiting yet, and each other cause of a waiting or failed newest run, in order of
 * first appearance.
 */
export function blockingCauses(reviews: readonly ReviewEntry[], blocked: { id: string; summary: string; since: string | null } | null): BlockingCause[] {
  const causes = new Map<string, BlockingCause>();
  if (blocked) causes.set(blocked.id, { id: blocked.id, summary: blocked.summary, since: blocked.since, reviews: [] });
  for (const review of newestRuns(reviews)) {
    const cause = review.cause;
    if (!cause || cause.scope !== "all" || review.skipped_by_user || (review.outcome !== "waiting" && review.outcome !== "failed")) continue;
    const entry = causes.get(cause.id) ?? { id: cause.id, summary: cause.summary, since: null, reviews: [] };
    entry.reviews.push(review);
    causes.set(cause.id, entry);
  }
  return [...causes.values()];
}

/** The key of one plugin's blocking cause, such as "cause:whooper:docker". */
export function causeKey(plugin: string, cause: string): string {
  return `cause:${plugin}:${cause}`;
}

/** "36 reviews waiting", "1 review waiting", or "no reviews waiting". */
export function waitingText(count: number): string {
  return count === 0 ? "no reviews waiting" : `${count} ${count === 1 ? "review" : "reviews"} waiting`;
}

/** An earlier question about a review and the plugin's answer, from the `asks` that show-review prints. */
export type ReviewAnswer = { question: string; answer: string };

/** What the screen for a failed review shows of it: why it failed, and the answers to questions so far. */
export type FailedReviewRecord = {
  /** failure.txt, the reason the plugin gave when it stopped the review; null when it recorded none. */
  failure: string | null;
  answers: ReviewAnswer[];
};

/**
 * A failed review's record in what the show-review action printed for run `runId`, format version
 * 1, as Whooper's whooper-show prints it: `failure` and `asks`, oldest first. An ask that does not
 * fit is left out.
 */
export function readFailedReview(stdout: string, runId: string): FailedReviewRecord {
  let shown: unknown = null;
  try {
    shown = JSON.parse(stdout);
  } catch {
    // Answered below.
  }
  if (!isRecord(shown) || shown.version !== 1 || shown.run_id !== runId) throw new Error(`show-review did not print review ${runId} in format version 1.`);
  const asks: unknown[] = Array.isArray(shown.asks) ? shown.asks : [];
  return {
    failure: typeof shown.failure === "string" && shown.failure.trim() ? shown.failure.trim() : null,
    answers: asks.flatMap((ask) => (isRecord(ask) && typeof ask.question === "string" && typeof ask.answer === "string" ? [{ question: ask.question, answer: ask.answer }] : [])),
  };
}

/** The key of one plugin's review among every plugin's, such as "review:whooper:<run id>". */
export function reviewKey(plugin: string, runId: string): string {
  return `review:${plugin}:${runId}`;
}

export function reviewTitle(review: ReviewEntry): string {
  return review.title ?? `${review.repo}#${review.pr}`;
}

/** The pull request as rows name it: the repo's last part and the number, such as widgets#101. */
export function shortPullRequest(review: ReviewEntry): string {
  return `${review.repo.split("/").at(-1)}#${review.pr}`;
}

/** The verdict in words, such as "approve once CI passes". */
export function verdictWords(verdict: string | null): string {
  if (verdict === "approve_once_ci_passes") return "approve once CI passes";
  if (verdict === "look_first") return "look first";
  return verdict ? verdict.replaceAll("_", " ") : "waiting for you";
}

/**
 * What the review's row says about where it stands, and its dot: "Reviewing…", the verdict while it
 * waits on a decision, "Approved · 2:25 PM", "Waiting · Docker isn't running", or "Review failed"
 * in red.
 */
export function reviewState(review: ReviewEntry, now = new Date()): { words: string; status: Status } {
  if (review.submitted) return { words: `${review.submitted.event === "APPROVE" ? "Approved" : "Commented"} · ${shortDate(review.submitted.at, now)}`, status: "idle" };
  if (review.skipped_by_user) return { words: "Skipped", status: "idle" };
  if (review.outcome === "running") return { words: "Reviewing…", status: "running" };
  if (review.outcome === "waiting") return { words: `Waiting · ${review.cause?.summary ?? "blocked"}`, status: "idle" };
  if (review.outcome === "failed") return { words: "Review failed", status: "error" };
  if (review.needs_you) return { words: review.skipped ? "skipped, look first" : verdictWords(review.verdict), status: "attention" };
  return { words: "Not submitted", status: "idle" };
}

/** A commit as the review header shows it: its first 7 characters. */
export function shortSha(sha: string | null): string | null {
  return sha ? sha.slice(0, 7) : null;
}

/** A date as the history shows it: "3:12 PM" today, "Nov 24" this year, "Nov 24, 2025" before. */
export function shortDate(iso: string, now = new Date()): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  if (date.toDateString() === now.toDateString()) return date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return date.toLocaleDateString([], { month: "short", day: "numeric", ...(date.getFullYear() === now.getFullYear() ? {} : { year: "numeric" }) });
}

/** When the review last changed: its submission, else its end, else its start. */
export function reviewTime(review: ReviewEntry): string {
  return review.submitted?.at ?? review.ended_at ?? review.started_at;
}

/** What the user submitted, as the page's submitted line says it: "approved on Nov 24". */
export function submittedText(submitted: NonNullable<ReviewEntry["submitted"]>, now = new Date()): string {
  return `${submitted.event === "APPROVE" ? "approved" : "commented"} ${shortDate(submitted.at, now).includes(":") ? "at" : "on"} ${shortDate(submitted.at, now)}`;
}
