import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PluginActionRequest } from "../../bridge/types";
import { actionResult, fixtureConnectionsWalkthrough, fixturePlugin, fixtureReviews, fixtureStatus, fixtureTestGaps, pageActions } from "../../test/plugins";
import { render } from "../../test/render";
import { ReviewScreen } from "../agents/ReviewScreen";
import { readTestGaps } from "./shown";
import { TestGapsView } from "./TestGaps";
import { createReviewViews } from "./views";

const plugin = fixturePlugin({ actions: pageActions });
const review = { ...fixtureReviews()[0]!, head_sha: "2252c0f0000", pr_head_sha: "2252c0f0000", cause: null, skipped_by_user: false };
const RUN = review.run_id;
const NAME = "Changes no test would catch";

/** Opens review RUN, the Go review, over a fake Whooper whose show-review prints `shown`. */
function renderReview(shown: object = fixtureConnectionsWalkthrough(RUN)) {
  const pluginRequest = vi.fn(async (sent: PluginActionRequest) => (sent.action === "show-review" ? actionResult({ stdout: JSON.stringify(shown) }) : actionResult({ stdout: "{}" })));
  // One store of views for the render: a prop written as a call would make a new one on every read.
  const views = createReviewViews();
  render(() => (
    <ReviewScreen
      plugin={plugin}
      status={{ value: fixtureStatus("2026-11-26T23:45:00Z", { reviews: [review] }), error: null }}
      runId={RUN}
      actions={{ pluginRequest, savePluginSettings: vi.fn() }}
      controls={{ refreshPlugins: vi.fn(async () => {}) }}
      onPluginFilesChange={() => () => {}}
      onOpenReview={vi.fn()}
      onOpenCause={vi.fn()}
      triage={{ skip: vi.fn(async () => {}), remove: vi.fn(async () => {}), restore: vi.fn(async () => {}) }}
      onSubmitted={vi.fn()}
      views={views}
      onNotice={vi.fn()}
    />
  ));
}

const walkthrough = () => screen.findByRole("navigation", { name: "Walkthrough" });
const block = () => screen.queryByRole("region", { name: NAME });
/** The first place to look, create.go's lines 11 to 13, or place `number` counting from 1; the second is name.go's line 6. */
const place = (number = 1) => document.querySelectorAll<HTMLElement>(".walk-places li")[number - 1]!;
/** test_gaps as Whooper printed it before it changed lines on purpose: only the lines no test ran, and no counts of changes. */
const notRunOnly = () => ({
  ...fixtureTestGaps(),
  lines: [
    { path: "internal/projects/create.go", line: 11, status: "not_run" },
    { path: "internal/projects/create.go", line: 12, status: "not_run" },
    { path: "internal/projects/create.go", line: 14, status: "not_run" },
    { path: "internal/projects/name.go", line: 10, status: "not_run" },
  ],
  summary: { changed_lines: 15, run: 2, not_run: 4 },
});
const codeIn = (element: Element) => [...element.querySelectorAll("code")].map((code) => code.textContent);
const press = (key: string) => {
  fireEvent.keyDown(document.body, { key });
  flush();
};
const follows = (first: Node, second: Node) => Boolean(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING);

async function openAllLines(shown?: object) {
  renderReview(shown);
  await walkthrough();
  fireEvent.click(within(block()!).getByRole("button", { name: "Open all lines" }));
  flush();
  return screen.getByRole("region", { name: NAME });
}

afterEach(() => {
  localStorage.clear();
});

describe("changes no test would catch", () => {
  it("stays hidden when show-review prints no test_gaps: nothing at the places, no block, and no item in Views", async () => {
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: null }));
    await walkthrough();
    expect(block()).not.toBeInTheDocument();
    expect(place()).not.toHaveTextContent("No test ran");
    fireEvent.click(screen.getByRole("button", { name: /^Views/ }));
    await screen.findByRole("menuitemcheckbox", { name: /^How the changed code connects/ });
    expect(screen.queryByRole("menuitemcheckbox", { name: new RegExp(NAME) })).not.toBeInTheDocument();
  });

  it("names a place's changed lines no test ran under it, labeled as measured, and the lines elsewhere after the places", async () => {
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: notRunOnly() }));
    await walkthrough();
    expect(place()).toHaveTextContent("No test ran 2 changed lines here: 11, 12");
    expect(place()).toHaveTextContent("Measured: no test ran these lines · go test -coverprofile");

    const found = block()!;
    expect(follows(place(), found)).toBe(true);
    expect(follows(found, screen.getByRole("heading", { name: "What Whooper checked and didn't" }))).toBe(true);
    expect(found).toHaveTextContent("Tests ran 2 of 15 changed lines, and no test ran 4. The run counted the other 9 as neither, such as comments.");
    expect(found).toHaveTextContent("Outside the places to look, no test ran 2 changed lines:");
    expect(within(found).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["internal/projects/create.go 14", "internal/projects/name.go 10"]);
    expect(found).toHaveTextContent("Measured: no test ran these lines · go test -coverprofile");

    // The place's reason under its lines says it too.
    press("j");
    await waitFor(() => expect(document.querySelector(".place-card")).toHaveTextContent("No test ran 2 changed lines here: 11, 12"), { timeout: 10_000 });
    expect(document.querySelector(".place-card")).toHaveTextContent("Measured: no test ran these lines");
  });

  it("says one line, labeled this line, for a place with one, counting only lines of the place's own file", async () => {
    const lines = [{ path: "internal/projects/create.go", line: 12, status: "not_run" }, { path: "internal/projects/name.go", line: 13, status: "not_run" }];
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), lines, summary: { changed_lines: 15, run: 5, not_run: 2 } } }));
    await walkthrough();
    // name.go's line 13 has the number of a line in the place, create.go's 11 to 13, but not its file.
    expect(place()).toHaveTextContent("No test ran 1 changed line here: 12");
    expect(place()).toHaveTextContent("Measured: no test ran this line");
    expect(within(block()!).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["internal/projects/name.go 13"]);
    expect(block()).toHaveTextContent("Measured: no test ran this line");
  });

  it("says only the counts when no line is outside the places", async () => {
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), lines: [{ path: "internal/projects/create.go", line: 12, status: "not_run" }], summary: { changed_lines: 15, run: 5, not_run: 1 } } }));
    await walkthrough();
    expect(block()).toHaveTextContent("Tests ran 5 of 15 changed lines, and no test ran 1.");
    expect(block()).toHaveTextContent("Measured · go test -coverprofile");
    expect(within(block()!).queryByRole("listitem")).not.toBeInTheDocument();
  });

  it("names each change no test noticed at its place after the place's lines no test ran, labeled as measured, and opens both with j", async () => {
    const lines = [
      { path: "internal/projects/create.go", line: 12, status: "not_run" },
      { path: "internal/projects/create.go", line: 11, status: "survived", change: "`err != nil` to `err == nil`" },
      { path: "internal/projects/create.go", line: 14, status: "not_run" },
      { path: "internal/projects/name.go", line: 6, status: "survived", change: "`{1,61}` to `{1,62}`" },
    ];
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), lines, summary: { changed_lines: 15, run: 3, not_run: 2, survived: 2, caught: 0 } } }));
    await walkthrough();
    const [notRun, unnoticed] = [...place().querySelectorAll(".place-gaps > div")];
    expect(notRun).toHaveTextContent("No test ran 1 changed line here: 12Measured: no test ran this line · go test -coverprofile");
    expect(unnoticed).toHaveTextContent("Tests ran line 11 but none noticed when Whooper changed err != nil to err == nilMeasured: no test noticed this change");
    // The change keeps Whooper's code in code type.
    expect(codeIn(unnoticed!)).toEqual(["err != nil", "err == nil"]);
    expect(place(2)).toHaveTextContent("Tests ran line 6 but none noticed when Whooper changed {1,61} to {1,62}Measured: no test noticed this change");
    expect(place(2)).not.toHaveTextContent("No test ran");
    // Both places are inside the lines, so nothing is left for after the places but the counts.
    expect(block()).toHaveTextContent("Tests caught 0 of 2 changes Whooper made on purpose.");
    expect(within(block()!).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["internal/projects/create.go 14"]);

    // Each place's reason under its lines says the same.
    press("j");
    await waitFor(() => expect(document.querySelector(".place-card")).toHaveTextContent("Tests ran line 11 but none noticed when Whooper changed err != nil to err == nil"), { timeout: 10_000 });
    expect(document.querySelector(".place-card")).toHaveTextContent("No test ran 1 changed line here: 12");
    press("j");
    await waitFor(() => expect(document.querySelector(".place-card")).toHaveTextContent("Tests ran line 6 but none noticed when Whooper changed {1,61} to {1,62}"), { timeout: 10_000 });
    expect(document.querySelector(".place-card")).toHaveTextContent("Measured: no test noticed this change");
  });

  it("counts the changes tests caught after the places, without listing any of them, at a place or after", async () => {
    const lines = [
      ...notRunOnly().lines.slice(0, 3),
      { path: "internal/projects/name.go", line: 6, status: "caught", change: "`{1,61}` to `{0,61}`", test: "TestValidateName" },
      { path: "internal/projects/name.go", line: 10, status: "caught", change: "`namePattern.MatchString(name)` to `!namePattern.MatchString(name)`", test: "TestValidateName" },
    ];
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), lines, summary: { changed_lines: 15, run: 2, not_run: 3, survived: 0, caught: 2 } } }));
    await walkthrough();
    expect(block()).toHaveTextContent("Tests ran 2 of 15 changed lines, and no test ran 3. The run counted the other 10 as neither, such as comments. Tests caught 2 of 2 changes Whooper made on purpose.");
    // Line 6 is the second place's line and line 10 is after the places: neither place nor the block names either, its change, or its test.
    expect(place(2).querySelector(".place-gaps")).toBeNull();
    const said = [...document.querySelectorAll(".place-gaps"), block()!].map((element) => element.textContent).join(" ");
    for (const word of ["line 6", "line 10", "{0,61}", "!namePattern", "TestValidateName", "caught by"]) expect(said).not.toContain(word);
    expect(within(block()!).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["internal/projects/create.go 14"]);
    expect(block()).toHaveTextContent("Measured: no test ran this line · go test -coverprofile");
  });

  it("says nothing of changes when the run doesn't count them, and lists the changes no test noticed outside the places", async () => {
    const lines = [
      { path: "internal/projects/create.go", line: 14, status: "survived", change: "`newProject(name, name)` to `newProject(name, \"\")`" },
      { path: "internal/projects/name.go", line: 10, status: "survived", change: "`namePattern.MatchString(name)` to `true`" },
      { path: "internal/projects/name.go", line: 8, status: "caught", change: "`a` to `b`", test: "TestValidateName" },
    ];
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), lines, summary: { changed_lines: 15, run: 5, not_run: 0 } } }));
    await walkthrough();
    expect(block()).toHaveTextContent("Tests ran 5 of 15 changed lines. The run counted the other 10 as neither, such as comments.");
    expect(block()).not.toHaveTextContent("Tests caught");
    expect(block()).toHaveTextContent("Outside the places to look, tests ran 2 changed lines but none noticed when Whooper changed them:");
    expect(within(block()!).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "internal/projects/create.go 14: newProject(name, name) to newProject(name, \"\")",
      "internal/projects/name.go 10: namePattern.MatchString(name) to true",
    ]);
    expect(block()).toHaveTextContent("Measured: no test noticed these changes");
    expect(block()).not.toHaveTextContent("Measured · go test");
    expect(block()).not.toHaveTextContent("TestValidateName");
  });

  it("names each run of the tests in code type when the plugin measured with several", async () => {
    const tool = "go test -coverprofile ./internal/projects/...; npx vitest run --coverage src/projects";
    const runs = ["go test -coverprofile ./internal/projects/...", "npx vitest run --coverage src/projects"];
    const view = await openAllLines(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), tool } }));
    const measured = view.querySelector(".gaps-counts")!;
    expect(measured).toHaveTextContent(`Measured with ${runs[0]} and ${runs[1]}. Tests ran 2 of 15 changed lines`);
    expect(codeIn(measured)).toEqual(runs);
    const create = within(view).getByRole("region", { name: "internal/projects/create.go" });
    expect(create).toHaveTextContent(`Measured: no test ran these lines · ${runs[0]} and ${runs[1]}`);

    press("Escape");
    expect(document.querySelector(".walkthrough")).not.toHaveAttribute("hidden");
    const label = place().querySelector(".place-evidence:last-child")!;
    expect(label).toHaveTextContent(`Measured: no test ran these lines · ${runs[0]} and ${runs[1]}`);
    expect(codeIn(label)).toEqual(runs);
  });

  it("says why in one quiet line when the run couldn't measure, with nothing at the places and nothing to open", async () => {
    // Lines the run lists anyway are not measured, so nothing says no test ran or noticed them.
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), measured: false, reason: "go isn't on the PATH" } }));
    await walkthrough();
    const line = screen.getByText("Not measured: go isn't on the PATH");
    expect(line).toHaveClass("walk-view-quiet");
    expect(line).toHaveTextContent(`${NAME}Not measured: go isn't on the PATH`);
    expect(block()).not.toBeInTheDocument();
    expect(place()).not.toHaveTextContent("No test ran");
    expect(place(2)).not.toHaveTextContent("noticed");
    expect(screen.queryByRole("button", { name: "Open all lines" })).not.toBeInTheDocument();
  });

  it("opens every line by file in the walkthrough's place: the counts in words, each file's lines with how that is known and its diff, and what the measure may miss", async () => {
    const view = await openAllLines(fixtureConnectionsWalkthrough(RUN, { test_gaps: notRunOnly() }));
    expect(document.querySelector(".walkthrough")).toHaveAttribute("hidden");
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: NAME }));
    expect(view).toHaveTextContent("Measured with go test -coverprofile. Tests ran 2 of 15 changed lines, and no test ran 4.");
    const create = within(view).getByRole("region", { name: "internal/projects/create.go" });
    expect(create).toHaveTextContent("No test ran 3 changed lines: 11, 12, 14");
    expect(create).toHaveTextContent("Measured: no test ran these lines · go test -coverprofile");
    expect(create.querySelector(".pt-diff")).not.toBeNull();
    const name = within(view).getByRole("region", { name: "internal/projects/name.go" });
    expect(name).toHaveTextContent("No test ran 1 changed line: 10");
    expect(name).toHaveTextContent("Measured: no test ran this line");
    const limits = within(view).getByRole("region", { name: "What this measure may miss" });
    expect(within(limits).getAllByRole("listitem").map((item) => item.textContent)).toEqual(fixtureTestGaps().limits);
  });

  it("groups each file's lines in full: no test ran, then no test noticed, then caught with the test that caught it", async () => {
    // Listed out of order, as a plugin may: the view groups them by what the runs found.
    const lines = [
      { path: "internal/projects/create.go", line: 14, status: "caught", change: "`newProject(name, name)` to `nil, nil`", test: "TestCreate" },
      { path: "internal/projects/create.go", line: 12, status: "not_run" },
      { path: "internal/projects/create.go", line: 11, status: "survived", change: "`err != nil` to `err == nil`" },
      { path: "internal/projects/name.go", line: 10, status: "caught", change: "`namePattern.MatchString(name)` to `!namePattern.MatchString(name)`" },
      { path: "internal/projects/name.go", line: 6, status: "survived", change: "`{1,61}` to `{1,62}`" },
    ];
    const view = await openAllLines(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), lines, summary: { changed_lines: 15, run: 4, not_run: 1, survived: 2, caught: 2 } } }));
    expect(view.querySelector(".gaps-counts")).toHaveTextContent("Measured with go test -coverprofile. Tests ran 4 of 15 changed lines, and no test ran 1. The run counted the other 10 as neither, such as comments. Tests caught 2 of 4 changes Whooper made on purpose.");

    const create = within(view).getByRole("region", { name: "internal/projects/create.go" });
    const groups = [...create.querySelectorAll(".gaps-group")];
    expect(groups.map((group) => group.textContent)).toEqual([
      "No test ran 1 changed line: 12Measured: no test ran this line · go test -coverprofile",
      "Tests ran line 11 but none noticed when Whooper changed err != nil to err == nilMeasured: no test noticed this change",
      "Tests caught this change Whooper made on purpose:Line 14: newProject(name, name) to nil, nil, caught by TestCreate",
    ]);
    expect(codeIn(groups[2]!)).toEqual(["newProject(name, name)", "nil, nil", "TestCreate"]);
    // The diff comes after the words that name its lines.
    expect(follows(groups[2]!, create.querySelector(".pt-diff")!)).toBe(true);

    // A file with no line no test ran; a caught change whose test the plugin doesn't name says only the change.
    const name = within(view).getByRole("region", { name: "internal/projects/name.go" });
    expect([...name.querySelectorAll(".gaps-group")].map((group) => group.textContent)).toEqual([
      "Tests ran line 6 but none noticed when Whooper changed {1,61} to {1,62}Measured: no test noticed this change",
      "Tests caught this change Whooper made on purpose:Line 10: namePattern.MatchString(name) to !namePattern.MatchString(name)",
    ]);
    expect(name).not.toHaveTextContent("No test ran");
    expect(name).not.toHaveTextContent("caught by");
    // This version knows each of these statuses, so none is a status it can't explain.
    expect(view).not.toHaveTextContent("can't explain");
  });

  it("shows a line with a status this version doesn't know in the plugin's word, without counting it as not run, not noticed, or caught", async () => {
    const lines = [
      ...fixtureTestGaps().lines,
      { path: "internal/projects/create.go", line: 13, status: "timed_out", change: "`}` to `return nil, nil }`" },
      { path: "internal/projects/types.go", line: 4, status: "not_viable" },
    ];
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), lines } }));
    await walkthrough();
    // Line 13 is in the place's lines, but only lines no test ran and changes no test noticed count there.
    expect(place()).toHaveTextContent("No test ran 2 changed lines here: 11, 12");
    expect(place()).not.toHaveTextContent("line 13");
    expect(block()).toHaveTextContent("Outside the places to look, no test ran 1 changed line:");
    expect(block()).toHaveTextContent("Tests caught 1 of 2 changes Whooper made on purpose.");
    expect(within(block()!).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["internal/projects/create.go 14"]);
    fireEvent.click(within(block()!).getByRole("button", { name: "Open all lines" }));
    flush();
    const view = screen.getByRole("region", { name: NAME });
    const create = within(view).getByRole("region", { name: "internal/projects/create.go" });
    expect(create).toHaveTextContent("No test ran 3 changed lines: 11, 12, 14");
    expect(create).toHaveTextContent("The run also marked line 13 (timed_out), which this version of Prompting Time can't explain.");
    expect(create).not.toHaveTextContent("noticed");
    const other = within(view).getByRole("region", { name: "internal/projects/types.go" });
    expect(other).toHaveTextContent("The run also marked line 4 (not_viable), which this version of Prompting Time can't explain.");
    expect(other).not.toHaveTextContent("No test ran");
    expect(other).not.toHaveTextContent("Tests caught");
    expect(other).toHaveTextContent("internal/projects/types.go isn't in the review's diff.");
  });

  it("shows why, and no lines, in full for a run that couldn't measure", () => {
    render(() => <TestGapsView testGaps={readTestGaps({ ...fixtureTestGaps(), measured: false, reason: "the tests didn't build" })!} patch={null} pluginTitle={plugin.title} onBack={vi.fn()} />);
    const view = screen.getByRole("region", { name: NAME });
    expect(view).toHaveTextContent("Not measured: the tests didn't build");
    expect(view).not.toHaveTextContent("No test ran");
    // The view's head says what it shows; below it, nothing claims a change no test noticed.
    expect(view.querySelector(".full-view-column")).not.toHaveTextContent("noticed");
    expect(view.querySelector(".gaps-file")).toBeNull();
    press("j");
    expect(view.querySelector("[data-current]")).toBeNull();
  });

  it("steps through the files with j and k, and Escape clears the current file, then goes back with the focus on Open all lines", async () => {
    const view = await openAllLines();
    const current = () => [...view.querySelectorAll(".gaps-file[data-current]")].map((element) => element.querySelector("h4")!.textContent);
    press("j");
    expect(current()).toEqual(["internal/projects/create.go"]);
    press("j");
    expect(current()).toEqual(["internal/projects/name.go"]);
    press("j");
    // Around the end to the first file.
    expect(current()).toEqual(["internal/projects/create.go"]);
    press("k");
    expect(current()).toEqual(["internal/projects/name.go"]);

    press("Escape");
    expect(current()).toEqual([]);
    expect(screen.getByRole("region", { name: NAME })).toBe(view);
    press("Escape");
    expect(document.querySelector(".walkthrough")).not.toHaveAttribute("hidden");
    expect(document.activeElement).toBe(within(block()!).getByRole("button", { name: "Open all lines" }));
  });

  it("goes back to the walkthrough when Views turns the view off while it shows", async () => {
    await openAllLines();
    fireEvent.click(screen.getByRole("button", { name: /^Views/ }));
    fireEvent.click(await screen.findByRole("menuitemcheckbox", { name: new RegExp(`^${NAME}`) }));
    await waitFor(() => expect(document.querySelector(".walkthrough")).not.toHaveAttribute("hidden"));
    expect(block()).not.toBeInTheDocument();
    expect(place()).not.toHaveTextContent("No test ran");
    expect(place(2)).not.toHaveTextContent("noticed");
    expect(screen.getByRole("button", { name: /^Views · 1 off/ })).toBeInTheDocument();
  });
});
