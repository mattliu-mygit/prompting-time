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
 * skipped the change. `author` is the pull request's author's login, and `removed_by_user` says the
 * user took the review out of the lists (its files stay, and Whooper no longer needs the user for
 * it). `safe_to_approve` is the plugin's own judgement that the run is safe to approve: Whooper
 * calls a run safe when it reviewed the whole change, says approve once CI passes, and found only
 * nits; null when the status doesn't say. The fields after `needs_you` came later in format version
 * 1, so an entry without them reads as having none.
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
  author: string | null;
  removed_by_user: boolean;
  safe_to_approve: boolean | null;
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

type LaterFields = "head_sha" | "pr_head_sha" | "cause" | "skipped_by_user" | "author" | "removed_by_user" | "safe_to_approve";

function fitsReview(value: unknown): value is Omit<ReviewEntry, LaterFields> & Partial<ReviewEntry> {
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
    && optional(value.skipped_by_user, (skipped) => typeof skipped === "boolean")
    && optional(value.author, stringOrNull)
    && optional(value.removed_by_user, (removed) => typeof removed === "boolean")
    && optional(value.safe_to_approve, (safe) => safe === null || typeof safe === "boolean");
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
    author: review.author ?? null,
    removed_by_user: review.removed_by_user ?? false,
    safe_to_approve: review.safe_to_approve ?? null,
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

/** Where the sidebar lists a pull request's newest run. */
export type ReviewCategory = "ready" | "look" | "failed" | "reviewing" | "waiting" | "done" | "removed";

/**
 * Where the sidebar lists a pull request's newest run: in no list once the user removed it; under
 * "Needs you" as ready to approve ("ready": the plugin judges the run safe to approve, or, when its
 * status doesn't say, the plugin says approve once CI passes and read the whole change), as needing
 * a look ("look": any other review waiting on a decision), or as failed for a cause of its own pull
 * request's ("failed"); on the row of a cause that blocks every review while it waits or failed on
 * one ("waiting"); under Reviewing while it runs, or waits without such a cause; and in Done once
 * the user finished with it. Without the plugin's judgement, a review whose findings are more than
 * nits lists as ready when its verdict says approve, since the verdict alone doesn't say.
 */
export function reviewCategory(review: ReviewEntry): ReviewCategory {
  if (review.removed_by_user) return "removed";
  if (readyToDecide(review)) return (review.safe_to_approve ?? (review.verdict === "approve_once_ci_passes" && !review.skipped)) ? "ready" : "look";
  if (isFinished(review)) return "done";
  if (review.outcome === "running") return "reviewing";
  const blocking = review.cause?.scope === "all";
  if (review.outcome === "waiting") return blocking ? "waiting" : "reviewing";
  return blocking ? "waiting" : "failed";
}

const newestEnd = (left: PluginReview, right: PluginReview) => reviewTime(right.review).localeCompare(reviewTime(left.review));

/** The reviews under "Needs you", each part newest first: what the sidebar lists, and Command-J steps through, in this order. */
export function needsYou(reviews: readonly PluginReview[]): Record<"ready" | "look" | "failed", PluginReview[]> {
  const of = (category: ReviewCategory) => reviews.filter(({ review }) => reviewCategory(review) === category).sort(newestEnd);
  return { ready: of("ready"), look: of("look"), failed: of("failed") };
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
    if (!cause || reviewCategory(review) !== "waiting") continue;
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
  const shown = readShown(stdout, runId);
  const asks: unknown[] = Array.isArray(shown.asks) ? shown.asks : [];
  return {
    failure: typeof shown.failure === "string" && shown.failure.trim() ? shown.failure.trim() : null,
    answers: asks.flatMap((ask) => (isRecord(ask) && typeof ask.question === "string" && typeof ask.answer === "string" ? [{ question: ask.question, answer: ask.answer }] : [])),
  };
}

/**
 * The kinds of the run's findings in what the show-review action printed for run `runId`, such as
 * ["nit", "question"], or null when it printed no findings.
 */
export function readFindingKinds(stdout: string, runId: string): string[] | null {
  const findings = readShown(stdout, runId).findings;
  if (!isRecord(findings) || !Array.isArray(findings.findings)) return null;
  return findings.findings.flatMap((finding: unknown) => (isRecord(finding) && typeof finding.kind === "string" ? [finding.kind] : []));
}

/** What the show-review action printed for run `runId`, in format version 1, as Whooper's whooper-show prints it. */
function readShown(stdout: string, runId: string): Record<string, unknown> {
  let shown: unknown = null;
  try {
    shown = JSON.parse(stdout);
  } catch {
    // Answered below.
  }
  if (!isRecord(shown) || shown.version !== 1 || shown.run_id !== runId) throw new Error(`show-review did not print review ${runId} in format version 1.`);
  return shown;
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

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;

/**
 * The gray line under a review's title in the sidebar: the pull request, its author, and where it
 * stands, such as "widgets#101 · @sam · 2 findings", "widgets#55 · @sam · The agent exited" for a
 * failure, "widgets#104 · reviewing since 3:05 PM", or "gadgets#77 · commented Nov 25". A removed
 * review's line says where it stood.
 */
export function reviewRowMeta(review: ReviewEntry, now = new Date()): string {
  const who = review.author ? `${shortPullRequest(review)} · @${review.author}` : shortPullRequest(review);
  return `${who} · ${standing(review, now)}`;
}

function standing(review: ReviewEntry, now: Date): string {
  if (review.submitted) return `${review.submitted.event === "APPROVE" ? "approved" : "commented"} ${shortDate(review.submitted.at, now)}`;
  if (review.skipped_by_user) return "skipped";
  switch (review.outcome) {
    case "running": return `reviewing since ${shortDate(review.started_at, now)}`;
    case "waiting": return review.cause ? `waiting · ${review.cause.summary}` : "waiting for the next round";
    case "failed": return review.cause?.summary ?? "review failed";
    case "reviewed":
      // Whooper stops needing the user for a review they removed, which still has its findings.
      if (!review.needs_you && !review.removed_by_user) return "not submitted";
      if (review.skipped) return "skipped, look first";
      return review.findings > 0 ? plural(review.findings, "finding") : "no findings";
  }
}

/** The words of a search, lowercase, as reviewMatches takes them. */
export function searchTokens(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

/**
 * Whether every word of a search finds the review: a number, with or without "#", starts its pull
 * request's number (or is in its title), so "#4502", "4502", and "450" find #4502; any other word,
 * with or without "@", is in its title, its author's login, its repo, or its short name such as
 * widgets#101.
 */
export function reviewMatches(review: ReviewEntry, tokens: readonly string[]): boolean {
  const title = (review.title ?? "").toLowerCase();
  const words = `${title} ${review.author ?? ""} ${review.repo} ${shortPullRequest(review)}`.toLowerCase();
  return tokens.every((token) => {
    const number = token.replace(/^#/, "");
    if (/^\d+$/.test(number)) return String(review.pr).startsWith(number) || title.includes(number);
    return words.includes(token.replace(/^@/, ""));
  });
}

/** The verdict in words, such as "approve once CI passes". */
export function verdictWords(verdict: string | null): string {
  if (verdict === "approve_once_ci_passes") return "approve once CI passes";
  if (verdict === "look_first") return "look first";
  return verdict ? verdict.replaceAll("_", " ") : "waiting for you";
}

/** Whether the review waits on the user's decision and says approve once CI passes, but the plugin judges it not safe to approve. */
export function notSafeToApprove(review: ReviewEntry): boolean {
  return readyToDecide(review) && review.verdict === "approve_once_ci_passes" && review.safe_to_approve === false;
}

/** Whooper's kinds of finding other than a nit, in the order the reasons name them, with the words for one and for several. */
const CONCERNS: ReadonlyArray<readonly [kind: string, one: string, several: string]> = [
  ["caller_visible_bug", "a bug", "bugs"],
  ["ci_failure", "a CI failure", "CI failures"],
  ["question", "a question", "questions"],
  ["other", "a suggestion", "suggestions"],
];

/** "a", "a and b", or "a, b, and c". */
function listWords(words: readonly string[]): string {
  return words.length < 3 ? words.join(" and ") : `${words.slice(0, -1).join(", ")}, and ${words.at(-1)}`;
}

/**
 * Why the plugin judges a review that says approve once CI passes not safe to approve, as Whooper
 * judges it: the review skipped the change, so it didn't read the whole change, or it found more
 * than nits. `kinds` are the kinds of the run's findings, which show-review prints; without them,
 * the words say only that it found more than nits. For example, a run with a nit and a question
 * "has a question", and one with two questions and a bug "has a bug and 2 questions".
 */
export function unsafeReasons(review: ReviewEntry, kinds: readonly string[] | null): string {
  const reasons = review.skipped ? ["didn't read the whole change"] : [];
  if (kinds === null) {
    if (review.findings > 0) reasons.push("has findings that aren't nits");
  } else {
    const concerns = kinds.filter((kind) => kind !== "nit");
    const others = concerns.filter((kind) => !CONCERNS.some(([known]) => known === kind)).length;
    const words = [
      ...CONCERNS.flatMap(([kind, one, several]) => {
        const count = concerns.filter((item) => item === kind).length;
        return count === 0 ? [] : [count === 1 ? one : `${count} ${several}`];
      }),
      ...(others === 0 ? [] : [others === 1 ? "another finding" : `${others} other findings`]),
    ];
    if (words.length > 0) reasons.push(`has ${listWords(words)}`);
  }
  return reasons.length > 0 ? reasons.join(", ") : "not safe to approve";
}

/** "approve once CI passes · has a question": the verdict, and why the plugin judges the review not safe to approve. */
export function unsafeVerdict(review: ReviewEntry, kinds: readonly string[] | null = null): string {
  return `${verdictWords(review.verdict)} · ${unsafeReasons(review, kinds)}`;
}

/**
 * What the review's row says about where it stands, and its dot: "Reviewing…", the verdict while it
 * waits on a decision, with why the plugin judges it not safe to approve when the verdict says
 * approve ("approve once CI passes · has findings that aren't nits"), "Approved · 2:25 PM",
 * "Waiting · Docker isn't running", or "Review failed" in red.
 */
export function reviewState(review: ReviewEntry, now = new Date()): { words: string; status: Status } {
  if (review.removed_by_user) return { words: "Removed", status: "idle" };
  if (review.submitted) return { words: `${review.submitted.event === "APPROVE" ? "Approved" : "Commented"} · ${shortDate(review.submitted.at, now)}`, status: "idle" };
  if (review.skipped_by_user) return { words: "Skipped", status: "idle" };
  if (review.outcome === "running") return { words: "Reviewing…", status: "running" };
  if (review.outcome === "waiting") return { words: `Waiting · ${review.cause?.summary ?? "blocked"}`, status: "idle" };
  if (review.outcome === "failed") return { words: "Review failed", status: "error" };
  if (notSafeToApprove(review)) return { words: unsafeVerdict(review), status: "attention" };
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
