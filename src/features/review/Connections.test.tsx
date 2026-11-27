import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PluginActionRequest } from "../../bridge/types";
import { actionResult, fixtureConnections, fixtureConnectionsWalkthrough, fixturePlugin, fixtureReviews, fixtureStatus, fixtureWalkthrough, pageActions } from "../../test/plugins";
import { render } from "../../test/render";
import { ReviewScreen } from "../agents/ReviewScreen";
import { createReviewViews } from "./views";

const plugin = fixturePlugin({ actions: pageActions });
const review = { ...fixtureReviews()[0]!, head_sha: "2252c0f0000", pr_head_sha: "2252c0f0000", cause: null, skipped_by_user: false };
const RUN = review.run_id;

/** Opens review RUN over a fake Whooper whose show-review prints `shown`. */
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
const summary = () => screen.queryByRole("region", { name: "How the changed code connects" });
const node = (name: RegExp) => screen.getByRole("button", { name });
const panel = () => screen.getByRole("complementary");
const press = (key: string) => {
  fireEvent.keyDown(document.body, { key });
  flush();
};

async function openMap() {
  renderReview();
  await walkthrough();
  fireEvent.click(screen.getByRole("button", { name: "Open the map" }));
  flush();
  return screen.getByRole("group", { name: "Map of the changed code" });
}

afterEach(() => {
  localStorage.clear();
});

describe("how the changed code connects", () => {
  it("stays hidden when show-review prints no connections: no summary, no part in the navigator, and no item in Views", async () => {
    renderReview(fixtureWalkthrough(RUN, { connections: null }));
    const nav = await walkthrough();
    expect(summary()).not.toBeInTheDocument();
    expect(within(nav).queryByRole("button", { name: "How the changed code connects" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^Views/ }));
    await screen.findByRole("menuitemcheckbox", { name: /^Calls before and after/ });
    expect(screen.queryByRole("menuitemcheckbox", { name: /How the changed code connects/ })).not.toBeInTheDocument();
  });

  it("sums the map up where the layers start, and turns off from the Views menu, which counts it as off", async () => {
    renderReview();
    const nav = await walkthrough();
    const found = summary()!;
    const divider = screen.getByText("The change, layer by layer").closest("p")!;
    expect(divider.compareDocumentPosition(found) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(found.compareDocumentPosition(screen.getByRole("region", { name: "Layer 1: The name rule" })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(found).toHaveTextContent("4 functions and variables change here: 2 added, 1 changed, and 1 removed.");
    expect(found).toHaveTextContent("2 unchanged callers: api.handleCreate and api.signup.");
    expect(found).toHaveTextContent("1 of 4 links was found by name search, so it may point at a different function or type with the same name.");
    expect(within(nav).getByRole("button", { name: "How the changed code connects" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^Views/ }));
    const item = await screen.findByRole("menuitemcheckbox", { name: /^How the changed code connects/ });
    expect(item).toHaveAttribute("aria-checked", "true");
    fireEvent.click(item);
    await waitFor(() => expect(summary()).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: /^Views · 1 off/ })).toBeInTheDocument();
  });

  it("opens the map in the walkthrough's place, and Back finds the walkthrough with the focus on Open the map", async () => {
    await openMap();
    expect(document.querySelector(".walkthrough")).toHaveAttribute("hidden");
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "How the changed code connects" }));
    expect(screen.getByRole("button", { name: "Walkthrough" })).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(screen.getByRole("button", { name: "Back to the walkthrough" }));
    flush();
    expect(screen.queryByRole("group", { name: "Map of the changed code" })).not.toBeInTheDocument();
    expect(document.querySelector(".walkthrough")).not.toHaveAttribute("hidden");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Open the map" }));
  });

  it("says each node's change in words, and a click selects it: its panel shows where it is and its links, and a link selects the other end", async () => {
    await openMap();
    expect(node(/^projects\.slugify, function, removed, 4 lines$/)).toHaveTextContent("removed · 4 lines");
    expect(node(/^api\.handleCreate, function, unchanged$/)).toHaveTextContent("unchanged");
    fireEvent.click(node(/^projects\.ValidateName,/));
    flush();
    expect(node(/^projects\.ValidateName,/)).toHaveAttribute("aria-pressed", "true");
    expect(panel()).toHaveAccessibleName("projects.ValidateName: where it is and its links");
    expect(panel()).toHaveTextContent("function · added · 3 lines changed");
    expect(within(panel()).getByRole("button", { name: /^internal\/projects\/name\.go:9/ })).toHaveAttribute("aria-expanded", "false");
    const callers = within(panel()).getByRole("region", { name: "Called or used by" });
    const [create, signup] = within(callers).getAllByRole("listitem");
    expect(create).toHaveTextContent("projects.Create calls projects.ValidateName");
    expect(create).toHaveTextContent("found by gopls");
    expect(signup).toHaveTextContent("api.signup calls projects.ValidateName");
    expect(within(panel()).getByRole("region", { name: "Calls or uses" })).toHaveTextContent("projects.ValidateName uses projects.namePattern");

    fireEvent.click(within(signup!).getByRole("button", { name: "api.signup" }));
    flush();
    expect(node(/^api\.signup,/)).toHaveAttribute("aria-pressed", "true");
    expect(panel()).toHaveTextContent("internal/api/users.go:30 · unchanged, so the diff has none of its lines");
    expect(within(panel()).queryByRole("button", { name: /Open the diff/ })).not.toBeInTheDocument();
  });

  it("draws what only a search for a name found dashed, and says found by name search in the legend and the links", async () => {
    await openMap();
    expect(document.querySelectorAll('.map-edge[data-found-by="name_search"]')).toHaveLength(1);
    expect(document.querySelectorAll('.map-edge[data-found-by="gopls"]')).toHaveLength(3);
    const legend = screen.getByRole("list", { name: "What the map's marks mean" });
    expect(legend).toHaveTextContent("calls or uses, found by gopls");
    expect(legend).toHaveTextContent("found by name search: may be a different function or type with the same name");
    expect(legend.querySelector('.map-line[data-found-by="name_search"]')).not.toBeNull();

    fireEvent.click(node(/^api\.signup,/));
    flush();
    const link = within(within(panel()).getByRole("region", { name: "Calls or uses" })).getByRole("listitem");
    expect(link).toHaveAttribute("data-found-by", "name_search");
    expect(link).toHaveTextContent("found by name search, so api.signup may call a different function named projects.ValidateName");
  });

  it("lists what the map may miss in Whooper's words", async () => {
    await openMap();
    const limits = screen.getByRole("region", { name: "What this map may miss" });
    expect(within(limits).getAllByRole("listitem").map((item) => item.textContent)).toEqual(fixtureConnections().limits);
  });

  it("steps through the changed nodes with j and k, selects any node with Enter, and Escape clears the selection, then goes back", async () => {
    await openMap();
    const pressed = () => screen.getAllByRole("button", { pressed: true }).filter((element) => element.classList.contains("map-node")).map((element) => element.getAttribute("data-node"));
    press("j");
    expect(pressed()).toEqual(["slugify"]);
    press("j");
    expect(pressed()).toEqual(["create"]);
    press("k");
    press("k");
    // Around the start to the last changed node; the unchanged callers between them are skipped.
    expect(pressed()).toEqual(["pattern"]);

    const caller = node(/^api\.handleCreate,/);
    expect(caller).toHaveAttribute("tabindex", "0");
    caller.focus();
    fireEvent.keyDown(caller, { key: "Enter" });
    flush();
    expect(pressed()).toEqual(["handle"]);

    press("Escape");
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Map of the changed code" })).toBeInTheDocument();
    press("Escape");
    expect(screen.queryByRole("group", { name: "Map of the changed code" })).not.toBeInTheDocument();
    expect(document.querySelector(".walkthrough")).not.toHaveAttribute("hidden");
  });

  it("opens the diff at a node's line from its location, on the old side for a removed function, and says when the diff starts later", async () => {
    const scrolled = vi.fn();
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled(this.getAttribute("data-column-number"), this.getAttribute("data-line-type"));
    };
    const connections = fixtureConnections();
    // Create's first line is outside the patch, which starts at line 10.
    connections.nodes[2] = { ...connections.nodes[2]!, line: 4 };
    renderReview(fixtureConnectionsWalkthrough(RUN, { connections }));
    await walkthrough();
    fireEvent.click(screen.getByRole("button", { name: "Open the map" }));
    flush();
    press("j");
    const open = within(panel()).getByRole("button", { name: /^internal\/projects\/create\.go:15/ });
    fireEvent.click(open);
    flush();
    expect(open).toHaveAttribute("aria-expanded", "true");
    await waitFor(() => expect(scrolled).toHaveBeenCalledWith("15", "change-deletion"), { timeout: 10_000 });

    // The diff stays open as j moves the selection to Create.
    press("j");
    expect(panel()).toHaveTextContent("The diff doesn't show line 4; it opens at line 10, the next line it shows.");
    await waitFor(() => expect(scrolled).toHaveBeenLastCalledWith("10", expect.anything()), { timeout: 10_000 });
    // @ts-expect-error jsdom has no scrollIntoView; the test put one in for itself.
    delete Element.prototype.scrollIntoView;
  });
});
