import { flush } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { fixtureConnections, fixtureConnectionsWalkthrough, fixtureReviews, fixtureWalkthrough } from "../../test/plugins";
import type { ReviewEntry } from "../agents/reviews";
import { layoutConnections } from "./Connections";
import { hideSince, lastLook, rememberLook, sinceHidden } from "./memory";
import { readConnections, readShownReview } from "./shown";
import { createReviewViews, REVIEW_VIEWS, runViewCommand, viewCommands, viewHasData, viewListed } from "./views";

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

  it("reads how the changed code connects, leaving out nodes and edges that don't fit", () => {
    expect(readShownReview(JSON.stringify(fixtureConnectionsWalkthrough(RUN)), RUN).connections).toEqual(fixtureConnections());
    const { nodes, edges } = fixtureConnections();
    const read = readConnections({
      nodes: [...nodes, { ...nodes[0]!, name: "a second node with the first one's id" }, { ...nodes[1]!, id: "field", kind: "field" }, { ...nodes[1]!, id: "no-count", lines_changed: -1 }],
      edges: [...edges, { from: "create", to: "missing", kind: "calls", found_by: "gopls" }, { from: "create", to: "pattern", kind: "calls", found_by: "the model" }],
      limits: ["One limit", 3],
    })!;
    expect(read.nodes.map((node) => node.name)).toEqual(nodes.map((node) => node.name));
    expect(read.edges).toEqual(edges);
    expect(read.limits).toEqual(["One limit"]);
    // None at all, or none with a node that fits, reads as no map.
    expect(readConnections(null)).toBeNull();
    expect(readConnections({ nodes: [{ id: "x" }], edges: [], limits: ["gopls isn't installed"] })).toBeNull();
  });
});

describe("the map of how the changed code connects", () => {
  it("puts callers left of what they call, and places a cycle and a function that calls itself", () => {
    const placed = layoutConnections(readConnections(fixtureConnections())!);
    const x = (id: string) => placed.nodes.find((node) => node.id === id)!.x;
    expect(x("handle")).toBeLessThan(x("create"));
    expect(x("create")).toBeLessThan(x("validate"));
    expect(x("validate")).toBeLessThan(x("pattern"));
    expect(placed.nodes.map((node) => node.id)).toEqual(["slugify", "handle", "signup", "create", "validate", "pattern"]);

    const node = (id: string) => ({ ...fixtureConnections().nodes[0]!, id, name: id });
    const loop = layoutConnections(readConnections({
      nodes: [node("a"), node("b"), node("c")],
      edges: [{ from: "a", to: "b", kind: "calls", found_by: "gopls" }, { from: "b", to: "a", kind: "calls", found_by: "gopls" }, { from: "c", to: "c", kind: "calls", found_by: "gopls" }],
      limits: [],
    })!);
    expect(loop.nodes).toHaveLength(3);
    expect(loop.nodes.every((item) => Number.isFinite(item.x) && Number.isFinite(item.y))).toBe(true);
  });
});

describe("review views", () => {
  it("has data only where the review printed some, and keeps the views turned off on this device", () => {
    const shown = readShownReview(JSON.stringify(fixtureWalkthrough(RUN, { co_change: [] })), RUN);
    expect([viewHasData("since", shown), viewHasData("calls", shown), viewHasData("co-change", shown), viewHasData("connections", shown)]).toEqual([true, true, false, false]);
    // The Views menu lists a view without data, saying so, except How the changed code connects.
    expect(REVIEW_VIEWS.filter((view) => viewListed(view, shown)).map((view) => view.id)).toEqual(["since", "calls", "co-change"]);
    const mapped = readShownReview(JSON.stringify(fixtureConnectionsWalkthrough(RUN)), RUN);
    expect(REVIEW_VIEWS.filter((view) => viewListed(view, mapped)).map((view) => view.id)).toEqual(["since", "calls", "co-change", "connections"]);
    const views = createReviewViews();
    expect(viewCommands(views)[0]!.items.map((item) => item.label)).toEqual(["Turn off Changes since you last looked", "Turn off Calls before and after", "Turn off Files that usually change with these", "Turn off How the changed code connects"]);
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
