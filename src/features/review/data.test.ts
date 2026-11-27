import { flush } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { fixtureConnections, fixtureConnectionsWalkthrough, fixtureExamples, fixtureReviews, fixtureTestGaps, fixtureWalkthrough } from "../../test/plugins";
import type { ReviewEntry } from "../agents/reviews";
import { layoutConnections } from "./Connections";
import { hideSince, lastLook, rememberLook, sinceHidden } from "./memory";
import { readConnections, readExamples, readShownReview, readTestGaps } from "./shown";
import { countsText, lineList } from "./TestGaps";
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

describe("changes no test would catch", () => {
  it("reads the lines no test ran, the changes no test noticed or tests caught, and their counts, and keeps a status this version doesn't know", () => {
    const shown = readShownReview(JSON.stringify(fixtureConnectionsWalkthrough(RUN)), RUN).testGaps!;
    expect(shown.lines).toEqual(fixtureTestGaps().lines.map((line) => ({ change: null, test: null, ...line })));
    expect(shown.summary).toEqual({ changed_lines: 15, run: 2, not_run: 3, survived: 1, caught: 1 });
    const read = readTestGaps({
      ...fixtureTestGaps(),
      lines: [
        { path: "a.go", line: 3, status: "not_run" },
        // A repeated line is left out.
        { path: "a.go", line: 3, status: "survived", change: "`<` to `<=`" },
        { path: "a.go", line: 4, status: "survived", change: "`<` to `<=`", test: 3 },
        { path: "a.go", line: 5, status: "caught", change: "`+` to `-`", test: "TestAdd" },
        { path: "a.go", line: 6, status: "caught", change: "`+` to `-`", test: "" },
        // A change made on purpose without its change in words is left out: only the change says what the tests missed.
        { path: "a.go", line: 7, status: "survived" },
        { path: "a.go", line: 8, status: "caught", change: "", test: "TestAdd" },
        { path: "a.go", line: 9, status: "timed_out", change: "`*` to `/`" },
        { path: "a.go", line: 0, status: "not_run" },
        { path: "a.go", line: 10 },
        "a.go:11",
      ],
      limits: ["One limit", 3],
    })!;
    expect(read.lines).toEqual([
      { path: "a.go", line: 3, status: "not_run", change: null, test: null },
      { path: "a.go", line: 4, status: "survived", change: "`<` to `<=`", test: null },
      { path: "a.go", line: 5, status: "caught", change: "`+` to `-`", test: "TestAdd" },
      { path: "a.go", line: 6, status: "caught", change: "`+` to `-`", test: null },
      { path: "a.go", line: 9, status: "timed_out", change: "`*` to `/`", test: null },
    ]);
    expect(read.limits).toEqual(["One limit"]);
    // Counts of changes are each a count or null; a run from before Whooper changed lines on purpose has none.
    expect(readTestGaps({ ...fixtureTestGaps(), summary: { changed_lines: 15, run: 2, not_run: 3, survived: -1, caught: "2" } })!.summary)
      .toEqual({ changed_lines: 15, run: 2, not_run: 3, survived: null, caught: null });
    expect(readTestGaps({ ...fixtureTestGaps(), summary: { changed_lines: 15, run: 2, not_run: 3 } })!.summary).toMatchObject({ survived: null, caught: null });
    // A run the plugin couldn't make still reads, with why.
    expect(readTestGaps({ measured: false, reason: "go isn't on the PATH", tool: "go test -coverprofile", lines: [], summary: { changed_lines: 0, run: 0, not_run: 0 }, limits: [] }))
      .toMatchObject({ measured: false, reason: "go isn't on the PATH" });
    // Without `measured`, `tool`, or counts, it reads as none.
    for (const broken of [null, { ...fixtureTestGaps(), measured: "yes" }, { ...fixtureTestGaps(), tool: null }, { ...fixtureTestGaps(), summary: { changed_lines: 15, run: -1, not_run: 4 } }]) {
      expect(readTestGaps(broken)).toBeNull();
    }
  });

  it("names lines in order, a run of three or more as a range, and says the counts in words", () => {
    const counts = (changed_lines: number, run: number, not_run: number, survived: number | null = null, caught: number | null = null) => ({ changed_lines, run, not_run, survived, caught });
    expect(lineList([40, 39])).toBe("39, 40");
    expect(lineList([50, 39, 40, 41, 42, 39])).toBe("39–42, 50");
    expect(countsText(counts(21, 3, 4))).toBe("Tests ran 3 of 21 changed lines, and no test ran 4. The run counted the other 14 as neither, such as comments.");
    expect(countsText(counts(14, 14, 0))).toBe("Tests ran 14 of 14 changed lines.");
    expect(countsText(counts(1, 0, 1))).toBe("Tests ran 0 of 1 changed line, and no test ran 1.");
    expect(countsText(counts(21, 4, 5, 2, 5))).toBe("Tests ran 4 of 21 changed lines, and no test ran 5. The run counted the other 12 as neither, such as comments. Tests caught 5 of 7 changes Whooper made on purpose.");
    expect(countsText(counts(3, 3, 0, 0, 1))).toBe("Tests ran 3 of 3 changed lines. Tests caught 1 of 1 change Whooper made on purpose.");
    // No change made, or only one of the two counts, says nothing of changes.
    expect(countsText(counts(3, 3, 0, 0, 0))).toBe("Tests ran 3 of 3 changed lines.");
    expect(countsText(counts(3, 3, 0, 2, null))).toBe("Tests ran 3 of 3 changed lines.");
  });
});

describe("example inputs, before and after", () => {
  it("reads the rows that say how they ran, leaves out one with a status or label this version doesn't know, and reads none as no examples", () => {
    expect(readShownReview(JSON.stringify(fixtureConnectionsWalkthrough(RUN)), RUN).examples).toEqual(fixtureExamples());
    const { rows } = fixtureExamples();
    const read = readExamples({
      ...fixtureExamples(),
      rows: [...rows, { ...rows[1]!, label: "predicted" }, { ...rows[1]!, status: "timed_out" }, { ...rows[1]!, input: 3 }, { ...rows[1]!, before: 3, note: false }],
    })!;
    expect(read.rows).toEqual([...rows, { ...rows[1]!, before: null, note: null }]);
    expect(readExamples({ functions: fixtureExamples().functions, rows: [{ ...rows[0]!, label: "predicted" }], limits: [] })).toBeNull();
    expect(readExamples(null)).toBeNull();
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
    expect(REVIEW_VIEWS.map((view) => viewHasData(view.id, shown))).toEqual([true, false, false, true, false, false]);
    // The Views menu lists a view without data, saying so, except the three that show in full:
    // example inputs, changes no test would catch, and how the changed code connects.
    expect(REVIEW_VIEWS.filter((view) => viewListed(view, shown)).map((view) => view.id)).toEqual(["since", "calls", "co-change"]);
    const goReview = readShownReview(JSON.stringify(fixtureConnectionsWalkthrough(RUN)), RUN);
    expect(REVIEW_VIEWS.filter((view) => viewListed(view, goReview)).map((view) => view.id)).toEqual(["since", "inputs", "test-gaps", "calls", "co-change", "connections"]);
    // A run of the tests that couldn't measure is still data: its view says why.
    const unmeasured = readShownReview(JSON.stringify(fixtureWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), measured: false, reason: "no Go files", lines: [] } })), RUN);
    expect(REVIEW_VIEWS.filter((view) => viewListed(view, unmeasured)).map((view) => view.id)).toContain("test-gaps");
    const views = createReviewViews();
    expect(viewCommands(views)[0]!.items.map((item) => item.label)).toEqual([
      "Turn off Changes since you last looked",
      "Turn off Example inputs, before and after",
      "Turn off Changes no test would catch",
      "Turn off Calls before and after",
      "Turn off Files that usually change with these",
      "Turn off How the changed code connects",
    ]);
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
