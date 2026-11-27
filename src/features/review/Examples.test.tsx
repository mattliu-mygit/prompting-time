import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PluginActionRequest } from "../../bridge/types";
import { actionResult, fixtureConnectionsWalkthrough, fixtureExamples, fixturePlugin, fixtureReviews, fixtureStatus, pageActions } from "../../test/plugins";
import { render } from "../../test/render";
import { ReviewScreen } from "../agents/ReviewScreen";
import { createReviewViews } from "./views";

const plugin = fixturePlugin({ actions: pageActions });
const review = { ...fixtureReviews()[0]!, head_sha: "2252c0f0000", pr_head_sha: "2252c0f0000", cause: null, skipped_by_user: false };
const RUN = review.run_id;
const NAME = "Example inputs, before and after";

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
const line = () => screen.queryByRole("group", { name: NAME });
const press = (key: string) => {
  fireEvent.keyDown(document.body, { key });
  flush();
};
/** Each row of a table as its cells' text. */
const cells = (table: HTMLElement) => within(table).getAllByRole("row").slice(1).map((row) => within(row).getAllByRole("cell").map((cell) => cell.textContent));

async function openAllInputs(shown?: object) {
  renderReview(shown);
  await walkthrough();
  fireEvent.click(within(line()!).getByRole("button", { name: "Open all inputs" }));
  flush();
  return screen.getByRole("region", { name: NAME });
}

afterEach(() => {
  localStorage.clear();
});

describe("example inputs, before and after", () => {
  it("stays hidden when show-review prints no examples: no line and no item in Views", async () => {
    renderReview(fixtureConnectionsWalkthrough(RUN, { examples: null }));
    await walkthrough();
    expect(line()).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^Views/ }));
    await screen.findByRole("menuitemcheckbox", { name: /^How the changed code connects/ });
    expect(screen.queryByRole("menuitemcheckbox", { name: new RegExp(NAME) })).not.toBeInTheDocument();
  });

  it("names the inputs whose result changed in one line under What this PR does, from rows run at base and head only", async () => {
    renderReview();
    await walkthrough();
    const found = line()!;
    const what = screen.getByRole("region", { name: "What this PR does" });
    expect(what).toContainElement(found);
    expect(what.querySelector("table")!.compareDocumentPosition(found) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // ValidateName's "a--b" differs too, but only the head ran it, so nothing was compared.
    expect(found).toHaveTextContent(`${NAME}projects.Create gives a different result for "a b". Ran at base and head.`);
    expect(within(found).getByRole("button", { name: "Open all inputs" })).toBeInTheDocument();
  });

  it("names each function's changed inputs, and says when no result changed or nothing could be compared", async () => {
    const { functions, rows } = fixtureExamples();
    const clamp = { function: "limits.Clamp", input: "101", before: "101", after: "100", status: "different", label: "ran_base_and_head", note: null };
    renderReview(fixtureConnectionsWalkthrough(RUN, { examples: { functions, rows: [...rows, { ...rows[1]!, input: "\"ab\"" }, clamp], limits: [] } }));
    await walkthrough();
    expect(line()).toHaveTextContent('projects.Create gives a different result for "a b" and "ab", and limits.Clamp for 101.');
  });

  it("says no result changed for the inputs run at base and head", async () => {
    const rows = fixtureExamples().rows.filter((row) => row.status !== "different");
    renderReview(fixtureConnectionsWalkthrough(RUN, { examples: { ...fixtureExamples(), rows } }));
    await walkthrough();
    expect(line()).toHaveTextContent(`${NAME}No result changed for the 3 inputs run at base and head.`);
  });

  it("says when no input ran at both base and head", async () => {
    const rows = fixtureExamples().rows.filter((row) => row.label !== "ran_base_and_head");
    renderReview(fixtureConnectionsWalkthrough(RUN, { examples: { ...fixtureExamples(), rows } }));
    await walkthrough();
    expect(line()).toHaveTextContent(`${NAME}No input ran at both base and head, so none could be compared.`);
  });

  it("opens a table per function in the walkthrough's place: changed first, each result compared in words, and how each row ran", async () => {
    const view = await openAllInputs();
    expect(document.querySelector(".walkthrough")).toHaveAttribute("hidden");
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: NAME }));
    const create = within(view).getByRole("region", { name: "projects.Create internal/projects/create.go:10" });
    expect(within(create).getAllByRole("columnheader").map((header) => header.textContent)).toEqual(["Input", "Before", "After", "Compared", "How it ran"]);
    expect(cells(within(create).getByRole("table"))).toEqual([
      ['"a b"', '&Project{Name: "a b", Bucket: "a b"}', 'error: name "a b" can\'t name a bucket', "changed", "Ran at base and head"],
      ['"abc"', '&Project{Name: "abc", Bucket: "abc"}', '&Project{Name: "abc", Bucket: "abc"}', "same", "Ran at base and head"],
      ['"x"', '&Project{Name: "x"}', '&Project{Name: "x"}', "varies between runs", "Ran at base and head · the two runs at base gave different results"],
      ['""', "–none", "–none", "run failed", "Ran at base and head · the run at base timed out"],
    ]);
    // Only a result that changed between base and head is orange.
    expect(within(create).getByText("changed")).toHaveAttribute("data-changed");
    expect(within(create).getByText("same")).not.toHaveAttribute("data-changed");

    const validate = within(view).getByRole("region", { name: "projects.ValidateName internal/projects/name.go:9" });
    // Same and different compare two runs, so a row that ran at the head only, or neither, has none.
    expect(cells(within(validate).getByRole("table"))).toEqual([
      ['"a--b"', "–none", "nil", "–nothing to compare", "Ran at head only · new in this PR"],
      ['"Q3"', "–none", "–none", "–nothing to compare", "Not compared · the test didn't build"],
    ]);
    const limits = within(view).getByRole("region", { name: "What these runs may miss" });
    expect(within(limits).getAllByRole("listitem").map((item) => item.textContent)).toEqual(fixtureExamples().limits);
  });

  it("steps through the inputs with j and k, and Escape clears the current one, then goes back with the focus on Open all inputs", async () => {
    const view = await openAllInputs();
    const current = () => [...view.querySelectorAll('tr[aria-current="true"]')].map((row) => row.querySelector("td")!.textContent);
    press("j");
    expect(current()).toEqual(['"a b"']);
    press("j");
    expect(current()).toEqual(['"abc"']);
    press("k");
    press("k");
    // Around the start to the last input, in the second table.
    expect(current()).toEqual(['"Q3"']);

    press("Escape");
    expect(current()).toEqual([]);
    expect(screen.getByRole("region", { name: NAME })).toBe(view);
    press("Escape");
    expect(document.querySelector(".walkthrough")).not.toHaveAttribute("hidden");
    expect(document.activeElement).toBe(within(line()!).getByRole("button", { name: "Open all inputs" }));
  });

  it("goes back to the walkthrough when Views turns the view off while it shows", async () => {
    await openAllInputs();
    fireEvent.click(screen.getByRole("button", { name: /^Views/ }));
    fireEvent.click(await screen.findByRole("menuitemcheckbox", { name: new RegExp(`^${NAME}`) }));
    await waitFor(() => expect(document.querySelector(".walkthrough")).not.toHaveAttribute("hidden"));
    expect(line()).not.toBeInTheDocument();
  });
});
