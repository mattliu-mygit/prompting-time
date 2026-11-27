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
const place = () => document.querySelector<HTMLElement>(".walk-places li")!;
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
    renderReview();
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

  it("says why in one quiet line when the run couldn't measure, with nothing at the places and nothing to open", async () => {
    // Lines the run lists anyway are not measured, so nothing says no test ran them.
    renderReview(fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), measured: false, reason: "go isn't on the PATH" } }));
    await walkthrough();
    const line = screen.getByText("Not measured: go isn't on the PATH");
    expect(line).toHaveClass("walk-view-quiet");
    expect(line).toHaveTextContent(`${NAME}Not measured: go isn't on the PATH`);
    expect(block()).not.toBeInTheDocument();
    expect(place()).not.toHaveTextContent("No test ran");
    expect(screen.queryByRole("button", { name: "Open all lines" })).not.toBeInTheDocument();
  });

  it("opens every line by file in the walkthrough's place: the counts in words, each file's lines with how that is known and its diff, and what the measure may miss", async () => {
    const view = await openAllLines();
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

  it("shows a line with a status this version doesn't know in the plugin's word, without counting it as not run", async () => {
    const lines = [...fixtureTestGaps().lines, { path: "internal/projects/create.go", line: 13, status: "survived" }, { path: "internal/projects/types.go", line: 4, status: "caught" }];
    const shown = fixtureConnectionsWalkthrough(RUN, { test_gaps: { ...fixtureTestGaps(), lines } });
    renderReview(shown);
    await walkthrough();
    // Line 13 is in the place's lines, but only lines no test ran count there.
    expect(place()).toHaveTextContent("No test ran 2 changed lines here: 11, 12");
    expect(block()).toHaveTextContent("Outside the places to look, no test ran 2 changed lines:");
    fireEvent.click(within(block()!).getByRole("button", { name: "Open all lines" }));
    flush();
    const view = screen.getByRole("region", { name: NAME });
    expect(within(view).getByRole("region", { name: "internal/projects/create.go" })).toHaveTextContent("The run also marked line 13 (survived), which this version of Prompting Time can't explain.");
    const other = within(view).getByRole("region", { name: "internal/projects/types.go" });
    expect(other).toHaveTextContent("The run also marked line 4 (caught), which this version of Prompting Time can't explain.");
    expect(other).not.toHaveTextContent("No test ran");
    expect(other).toHaveTextContent("internal/projects/types.go isn't in the review's diff.");
  });

  it("shows why, and no lines, in full for a run that couldn't measure", () => {
    render(() => <TestGapsView testGaps={readTestGaps({ ...fixtureTestGaps(), measured: false, reason: "the tests didn't build" })!} patch={null} onBack={vi.fn()} />);
    const view = screen.getByRole("region", { name: NAME });
    expect(view).toHaveTextContent("Not measured: the tests didn't build");
    expect(view).not.toHaveTextContent("No test ran");
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
    expect(screen.getByRole("button", { name: /^Views · 1 off/ })).toBeInTheDocument();
  });
});
