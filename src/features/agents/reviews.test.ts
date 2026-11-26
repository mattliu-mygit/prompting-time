import { describe, expect, it } from "vitest";
import { fixtureReviews } from "../../test/plugins";
import { blockingCauses, needsYou, readReviews, reviewCategory, reviewRowMeta, type ReviewEntry } from "./reviews";

const docker = { id: "docker", summary: "Docker isn't running", scope: "all" as const };
const agentExit = { id: "agent_no_findings", summary: "The agent exited without findings", scope: "pr" as const };
const now = new Date(2026, 10, 26, 16);
/** One invented review as the status lists it, a finished review of acme/widgets#101 that waits on the user unless overridden. */
const review = (overrides: Record<string, unknown> = {}): ReviewEntry => readReviews({ reviews: [{ ...fixtureReviews()[0]!, ...overrides }] })[0]!;

describe("reviews", () => {
  it("reads the author and the user's removal, and an entry without them as having none", () => {
    expect(review()).toMatchObject({ author: "sam-fixture", removed_by_user: false });
    expect(readReviews({ reviews: [fixtureReviews()[1]] })[0]).toMatchObject({ author: null, removed_by_user: false });
    expect(review({ removed_by_user: true })).toMatchObject({ removed_by_user: true });
    expect(readReviews({ reviews: [{ ...fixtureReviews()[0]!, author: 7 }] })).toEqual([]);
  });

  it("places each review in the list where it waits", () => {
    expect(reviewCategory(review())).toBe("ready");
    expect(reviewCategory(review({ skipped: true }))).toBe("look");
    expect(reviewCategory(review({ verdict: "look_first" }))).toBe("look");
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
      review({ run_id: "c", pr: 3, verdict: "look_first", ended_at: at(10) }),
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
});
