import { describe, expect, it } from "vitest";
import { fixtureReviews } from "../../test/plugins";
import { blockingCauses, needsYou, readFindingKinds, readReviews, reviewCategory, reviewMatches, reviewRowMeta, reviewState, searchTokens, unsafeReasons, type ReviewEntry } from "./reviews";

const docker = { id: "docker", summary: "Docker isn't running", scope: "all" as const };
const agentExit = { id: "agent_no_findings", summary: "The agent exited without findings", scope: "pr" as const };
const now = new Date(2026, 10, 26, 16);
/** One invented review as the status lists it, a finished review of acme/widgets#101 that waits on the user unless overridden. */
const review = (overrides: Record<string, unknown> = {}): ReviewEntry => readReviews({ reviews: [{ ...fixtureReviews()[0]!, ...overrides }] })[0]!;

describe("reviews", () => {
  it("reads the author, the user's removal, and whether the plugin judges the run safe to approve, and an entry without them as having none", () => {
    expect(review()).toMatchObject({ author: "sam-fixture", removed_by_user: false, safe_to_approve: true });
    expect(readReviews({ reviews: [fixtureReviews()[1]] })[0]).toMatchObject({ author: null, removed_by_user: false, safe_to_approve: false });
    expect(review({ removed_by_user: true })).toMatchObject({ removed_by_user: true });
    expect(review({ safe_to_approve: undefined })).toMatchObject({ safe_to_approve: null });
    expect(review({ safe_to_approve: null })).toMatchObject({ safe_to_approve: null });
    expect(readReviews({ reviews: [{ ...fixtureReviews()[0]!, author: 7 }] })).toEqual([]);
    expect(readReviews({ reviews: [{ ...fixtureReviews()[0]!, safe_to_approve: "yes" }] })).toEqual([]);
  });

  it("puts a review that waits on a decision under Ready to approve or Needs a look by the plugin's safe_to_approve, or by its verdict when the status doesn't say", () => {
    expect(reviewCategory(review({ safe_to_approve: true }))).toBe("ready");
    // Whooper says approve once CI passes, but a finding that is not a nit makes the run not safe to approve.
    expect(reviewCategory(review({ safe_to_approve: false }))).toBe("look");
    // A status from before the field: approve once CI passes, with the whole change read, is ready.
    for (const safe of [undefined, null]) {
      expect(reviewCategory(review({ safe_to_approve: safe }))).toBe("ready");
      expect(reviewCategory(review({ safe_to_approve: safe, skipped: true }))).toBe("look");
      expect(reviewCategory(review({ safe_to_approve: safe, verdict: "look_first" }))).toBe("look");
    }
    // The judgement sorts only what waits on a decision.
    expect(reviewCategory(review({ safe_to_approve: true, submitted: { event: "APPROVE", at: "2026-11-26T15:30:00Z" }, needs_you: false }))).toBe("done");
    expect(reviewCategory(review({ safe_to_approve: true, outcome: "running", needs_you: false }))).toBe("reviewing");
  });

  it("says why a review that says approve once CI passes is not safe to approve, from the kinds of its findings", () => {
    const unsafe = review({ safe_to_approve: false, findings: 2 });
    expect(unsafeReasons(unsafe, ["nit", "question"])).toBe("has a question");
    expect(unsafeReasons(unsafe, ["question", "nit", "caller_visible_bug", "question"])).toBe("has a bug and 2 questions");
    expect(unsafeReasons(unsafe, ["other", "ci_failure", "made_up", "question"])).toBe("has a CI failure, a question, a suggestion, and another finding");
    // Before show-review answers, the status says only that the findings are more than nits.
    expect(unsafeReasons(unsafe, null)).toBe("has findings that aren't nits");
    expect(unsafeReasons(review({ safe_to_approve: false, skipped: true, findings: 0 }), null)).toBe("didn't read the whole change");
    expect(unsafeReasons(review({ safe_to_approve: false, findings: 0 }), [])).toBe("not safe to approve");
    expect(reviewState(unsafe, now)).toEqual({ words: "approve once CI passes · has findings that aren't nits", status: "attention" });
    // A review the plugin judges safe, or a status without the judgement, keeps the verdict alone.
    expect(reviewState(review({ safe_to_approve: true }), now).words).toBe("approve once CI passes");
    expect(reviewState(review({ safe_to_approve: null }), now).words).toBe("approve once CI passes");
    expect(reviewState(review({ safe_to_approve: false, verdict: "look_first" }), now).words).toBe("look first");
  });

  it("reads the kinds of a run's findings from what show-review printed", () => {
    const shown = (findings: unknown, runId = "run-1") => JSON.stringify({ version: 1, run_id: runId, findings });
    expect(readFindingKinds(shown({ status: "reviewed", findings: [{ id: "1", kind: "nit" }, { id: "2", kind: "question" }, { id: "3" }] }), "run-1")).toEqual(["nit", "question"]);
    expect(readFindingKinds(shown(null), "run-1")).toBeNull();
    expect(() => readFindingKinds(shown({ findings: [] }, "run-2"), "run-1")).toThrow("show-review did not print review run-1 in format version 1.");
    expect(() => readFindingKinds("not json", "run-1")).toThrow();
  });

  it("places each review in the list where it waits", () => {
    expect(reviewCategory(review())).toBe("ready");
    expect(reviewCategory(review({ outcome: "failed", verdict: null, cause: agentExit }))).toBe("failed");
    expect(reviewCategory(review({ outcome: "failed", verdict: null, cause: null }))).toBe("failed");
    expect(reviewCategory(review({ outcome: "failed", verdict: null, cause: docker }))).toBe("waiting");
    expect(reviewCategory(review({ outcome: "waiting", needs_you: false, cause: docker }))).toBe("waiting");
    expect(reviewCategory(review({ outcome: "waiting", needs_you: false, cause: null }))).toBe("reviewing");
    expect(reviewCategory(review({ outcome: "running", needs_you: false }))).toBe("reviewing");
    expect(reviewCategory(review({ submitted: { event: "APPROVE", at: "2026-11-26T15:30:00Z" }, needs_you: false }))).toBe("done");
    expect(reviewCategory(review({ skipped_by_user: true }))).toBe("done");
    expect(reviewCategory(review({ removed_by_user: true, needs_you: false }))).toBe("removed");
  });

  it("lists Needs you's parts newest first, and leaves removed reviews out of a cause's", () => {
    const at = (hours: number) => new Date(2026, 10, 26, hours).toISOString();
    const reviews = [
      review({ run_id: "a", pr: 1, ended_at: at(9) }),
      review({ run_id: "b", pr: 2, ended_at: at(11) }),
      review({ run_id: "c", pr: 3, verdict: "look_first", safe_to_approve: false, ended_at: at(10) }),
      review({ run_id: "d", pr: 4, outcome: "failed", verdict: null, cause: null, ended_at: at(8) }),
    ].map((entry) => ({ plugin: "whooper", review: entry }));
    const parts = needsYou(reviews);
    expect(parts.ready.map(({ review }) => review.run_id)).toEqual(["b", "a"]);
    expect(parts.look.map(({ review }) => review.run_id)).toEqual(["c"]);
    expect(parts.failed.map(({ review }) => review.run_id)).toEqual(["d"]);

    const waiting = [
      review({ run_id: "w1", pr: 5, outcome: "waiting", needs_you: false, cause: docker }),
      review({ run_id: "w2", pr: 6, outcome: "waiting", needs_you: false, cause: docker, removed_by_user: true }),
    ];
    expect(blockingCauses(waiting, null).map((cause) => cause.reviews.map((entry) => entry.run_id))).toEqual([["w1"]]);
  });

  it("says who opened the pull request and where its review stands", () => {
    expect(reviewRowMeta(review(), now)).toBe("widgets#101 · @sam-fixture · 2 findings");
    expect(reviewRowMeta(review({ findings: 1, author: null }), now)).toBe("widgets#101 · 1 finding");
    expect(reviewRowMeta(review({ findings: 0 }), now)).toBe("widgets#101 · @sam-fixture · no findings");
    expect(reviewRowMeta(review({ outcome: "failed", verdict: null, cause: agentExit }), now)).toBe("widgets#101 · @sam-fixture · The agent exited without findings");
    expect(reviewRowMeta(review({ outcome: "waiting", cause: docker }), now)).toBe("widgets#101 · @sam-fixture · waiting · Docker isn't running");
    expect(reviewRowMeta(review({ skipped_by_user: true }), now)).toBe("widgets#101 · @sam-fixture · skipped");
    expect(reviewRowMeta(review({ submitted: { event: "COMMENT", at: "2026-11-24T15:30:00Z" } }), now)).toBe("widgets#101 · @sam-fixture · commented Nov 24");
    expect(reviewRowMeta(review({ removed_by_user: true }), now)).toBe("widgets#101 · @sam-fixture · 2 findings");
  });

  it("finds a review by its number with or without #, the words of its title, its author, and its repo", () => {
    const entry = review({ pr: 4502, title: "Separate the cache configuration" });
    const finds = (query: string) => reviewMatches(entry, searchTokens(query));
    expect(["#4502", "4502", "450", "cache separate", "CACHE", "@sam-fixture", "sam", "acme/widgets", "widgets#4502", "widgets 4502"].filter((query) => !finds(query))).toEqual([]);
    expect(["#4503", "502", "@someone", "acme/gadgets", "cache gadgets"].filter(finds)).toEqual([]);
    expect(searchTokens("  #4502   Cache ")).toEqual(["#4502", "cache"]);
  });
});
