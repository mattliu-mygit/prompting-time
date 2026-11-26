import { flush } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { fixtureReviews, fixtureWalkthrough } from "../../test/plugins";
import type { ReviewEntry } from "../agents/reviews";
import { hideSince, lastLook, rememberLook, sinceHidden } from "./memory";
import { readShownReview } from "./shown";
import { createReviewViews, runViewCommand, viewCommands, viewHasData } from "./views";

const review = { ...fixtureReviews()[0]!, head_sha: "1111111", pr_head_sha: "2222222", cause: null, skipped_by_user: false, removed_by_user: false } as ReviewEntry;
const RUN = review.run_id;

afterEach(() => localStorage.clear());

describe("readShownReview", () => {
  it("reads the overview, the commits since the review, and the files that usually change with these", () => {
    const shown = readShownReview(JSON.stringify(fixtureWalkthrough(RUN)), RUN);
    expect(shown.overview?.places.map((place) => [place.id, place.path, place.start_line, place.end_line])).toEqual([["p1", "src/retry.rs", 6, 8], ["p2", "src/client.rs", 2, 3]]);
    expect(shown.overview?.flow?.calls.map((call) => call.status)).toEqual(["removed", "added", "changed"]);
    expect(shown.commitsSinceReview?.map((commit) => commit.sha)).toEqual(["aaaa1111bbbb", "cccc2222dddd"]);
    expect(shown.coChange).toEqual([{ path: "src/server.rs", partner: "src/client.rs", ratio: 0.8, commits: 10 }]);
    expect(shown.drafts?.event).toBe("APPROVE");
  });

  it("leaves out what doesn't fit, and reads a missing overview and commits as none", () => {
    const shown = readShownReview(JSON.stringify(fixtureWalkthrough(RUN, {
      overview: { ...fixtureWalkthrough(RUN).overview, places: [{ id: "bad", title: "No lines" }], flow: { title: "t", input: "i", calls: [] } },
      commits_since_review: null,
    })), RUN);
    expect(shown.overview?.places).toEqual([]);
    expect(shown.overview?.flow).toBeNull();
    expect(shown.commitsSinceReview).toBeNull();
    expect(readShownReview(JSON.stringify(fixtureWalkthrough(RUN, { overview: { summary: 3 } })), RUN).overview).toBeNull();
  });
});

describe("review views", () => {
  it("has data only where the review printed some, and keeps the views turned off on this device", () => {
    const shown = readShownReview(JSON.stringify(fixtureWalkthrough(RUN, { co_change: [] })), RUN);
    expect([viewHasData("since", shown), viewHasData("calls", shown), viewHasData("co-change", shown), viewHasData("connections", shown)]).toEqual([true, true, false, false]);
    const views = createReviewViews();
    expect(viewCommands(views)[0]!.items.map((item) => item.label)).toEqual(["Turn off Changes since you last looked", "Turn off Calls before and after", "Turn off Files that usually change with these"]);
    expect(runViewCommand(views, "review-view:calls")).toBe(true);
    // Solid applies the write on its next flush.
    flush();
    expect(views.isOn("calls")).toBe(false);
    expect(createReviewViews().isOn("calls")).toBe(false);
    expect(runViewCommand(views, "command:new")).toBe(false);
  });
});

describe("what this device remembers of reviews", () => {
  it("remembers the version opened last and returns the one before", () => {
    expect(rememberLook(review, new Date("2026-11-26T10:00:00Z"))).toBeNull();
    expect(rememberLook({ ...review, run_id: "newer" }, new Date("2026-11-27T10:00:00Z"))).toEqual({ run: RUN, head: "2222222", at: "2026-11-26T10:00:00.000Z" });
    expect(lastLook(review)?.run).toBe("newer");
  });

  it("hides the since notice for one version of the pull request, until its head moves", () => {
    hideSince(review, true);
    expect(sinceHidden(review)).toBe(true);
    expect(sinceHidden({ ...review, pr_head_sha: "3333333" })).toBe(false);
    hideSince(review, false);
    expect(sinceHidden(review)).toBe(false);
  });
});
