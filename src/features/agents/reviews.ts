import type { Status } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginsSnapshot } from "../../app/store";

/**
 * One review in the `reviews` of `whooper-status --json`: Whooper's runs of the last 30 days,
 * newest first. `pr` is the pull request's number in `repo`, and times are ISO 8601. Whooper
 * decides `needs_you`: whether the review waits on the user.
 */
export type ReviewEntry = {
  run_id: string;
  repo: string;
  pr: number;
  title: string | null;
  url: string | null;
  started_at: string;
  ended_at: string | null;
  outcome: "running" | "reviewed" | "failed";
  verdict: string | null;
  skipped: boolean;
  findings: number;
  submitted: { event: "APPROVE" | "COMMENT"; at: string } | null;
  needs_you: boolean;
};

/** A review and the plugin that lists it. */
export type PluginReview = { plugin: string; review: ReviewEntry };

const OUTCOMES = new Set(["running", "reviewed", "failed"]);

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const stringOrNull = (value: unknown) => value === null || typeof value === "string";

function fitsReview(value: unknown): value is ReviewEntry {
  if (!isRecord(value)) return false;
  const submitted = value.submitted;
  return typeof value.run_id === "string" && value.run_id !== ""
    && typeof value.repo === "string" && typeof value.pr === "number"
    && stringOrNull(value.title) && stringOrNull(value.url)
    && typeof value.started_at === "string" && stringOrNull(value.ended_at)
    && typeof value.outcome === "string" && OUTCOMES.has(value.outcome)
    && stringOrNull(value.verdict) && typeof value.skipped === "boolean"
    && typeof value.findings === "number" && typeof value.needs_you === "boolean"
    && (submitted === null || (isRecord(submitted) && (submitted.event === "APPROVE" || submitted.event === "COMMENT") && typeof submitted.at === "string"));
}

/** The reviews a plugin's status lists, leaving out any entry that does not fit the format. */
export function readReviews(status: unknown): ReviewEntry[] {
  return isRecord(status) && Array.isArray(status.reviews) ? status.reviews.filter(fitsReview) : [];
}

/** The plugins' reviews by plugin, from each running plugin's latest status. */
export function pluginReviews(plugins: PluginsSnapshot): Record<string, ReviewEntry[]> {
  return Object.fromEntries(plugins.items.map((plugin: PluginSnapshot) => [plugin.name, readReviews(plugins.statuses[plugin.name]?.value)]));
}

/** The reviews waiting on the user, in each plugin's order, for "Needs you". */
export function reviewsNeedingYou(reviews: Record<string, ReviewEntry[]>): PluginReview[] {
  return Object.entries(reviews).flatMap(([plugin, entries]) => entries.filter((review) => review.needs_you).map((review) => ({ plugin, review })));
}

/** The reviews that ended, newest first, for the history list. */
export function pastReviews(reviews: readonly ReviewEntry[]): ReviewEntry[] {
  return reviews.filter((review) => review.outcome !== "running");
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

function verdictWords(verdict: string | null): string {
  if (verdict === "approve_once_ci_passes") return "approve once CI passes";
  if (verdict === "look_first") return "look first";
  return verdict ? verdict.replaceAll("_", " ") : "waiting for you";
}

/** What the review's row says about where it stands, and its dot. */
export function reviewState(review: ReviewEntry): { words: string; status: Status } {
  if (review.outcome === "running") return { words: "Whooper is reviewing", status: "running" };
  if (review.outcome === "failed") return { words: "review failed", status: review.needs_you ? "error" : "idle" };
  if (review.submitted) return { words: review.submitted.event === "APPROVE" ? "approved" : "commented", status: "idle" };
  if (review.needs_you) return { words: review.skipped ? "skipped, look first" : verdictWords(review.verdict), status: "attention" };
  return { words: review.skipped ? "skipped" : "not submitted", status: "idle" };
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
