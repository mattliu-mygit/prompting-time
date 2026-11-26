import { readPreference, savePreference } from "../../app/preferences";
import type { ReviewEntry } from "../agents/reviews";

/**
 * What this device remembers about the user's reviews: the version of each pull request's review
 * they last opened, and the versions whose "Changes since you last looked" notice they hid. Both
 * live in localStorage through preferences, so a missing or full storage only forgets them.
 */

/** The review version the user last opened, the pull request's head then, and when. */
export type LastLook = { run: string; head: string | null; at: string };

const LAST_OPENED = "prompting-time.review-last-opened";
const SINCE_HIDDEN = "prompting-time.review-since-hidden";
/**
 * How many pull requests each record keeps, the oldest dropped first. 300 covers months of reviews
 * at a few a day; raise it if notices come back for pull requests reopened after that long.
 */
const KEPT = 300;

/** The pull request a review belongs to, such as "acme/widgets#101". */
export const pullRequestKey = (review: Pick<ReviewEntry, "repo" | "pr">) => `${review.repo}#${review.pr}`;

function readRecord<T>(key: string, fits: (value: unknown) => value is T): Record<string, T> {
  try {
    const saved: unknown = JSON.parse(readPreference(key) ?? "{}");
    if (typeof saved !== "object" || saved === null || Array.isArray(saved)) return {};
    return Object.fromEntries(Object.entries(saved).filter((entry): entry is [string, T] => fits(entry[1])));
  } catch {
    return {};
  }
}

/** Saves the record with `key` set last, so the oldest entries are the first beyond KEPT. */
function saveRecord<T>(name: string, record: Record<string, T>, key: string, value: T | null) {
  const entries = Object.entries(record).filter(([name]) => name !== key);
  if (value !== null) entries.push([key, value]);
  savePreference(name, JSON.stringify(Object.fromEntries(entries.slice(-KEPT))));
}

const isLastLook = (value: unknown): value is LastLook => {
  const look = value as LastLook;
  return typeof value === "object" && value !== null && typeof look.run === "string" && (look.head === null || typeof look.head === "string") && typeof look.at === "string";
};

/** The version of the review's pull request the user opened before, or null. */
export function lastLook(review: ReviewEntry): LastLook | null {
  return readRecord(LAST_OPENED, isLastLook)[pullRequestKey(review)] ?? null;
}

/** Remembers that the user opened this version of the review now, and returns what they opened before. */
export function rememberLook(review: ReviewEntry, now = new Date()): LastLook | null {
  const record = readRecord(LAST_OPENED, isLastLook);
  const key = pullRequestKey(review);
  const before = record[key] ?? null;
  saveRecord(LAST_OPENED, record, key, { run: review.run_id, head: review.pr_head_sha, at: now.toISOString() });
  return before;
}

/** The version of the pull request that × hides the notice for: the review and the pull request's head, so a new commit or review brings it back. */
export const sinceVersion = (review: ReviewEntry) => `${review.run_id}@${review.pr_head_sha ?? ""}`;

export function sinceHidden(review: ReviewEntry): boolean {
  return readRecord(SINCE_HIDDEN, (value): value is string => typeof value === "string")[pullRequestKey(review)] === sinceVersion(review);
}

/** Hides the notice for this version of the pull request, or shows it again. */
export function hideSince(review: ReviewEntry, hidden: boolean) {
  saveRecord(SINCE_HIDDEN, readRecord(SINCE_HIDDEN, (value): value is string => typeof value === "string"), pullRequestKey(review), hidden ? sinceVersion(review) : null);
}
