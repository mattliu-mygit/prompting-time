import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type { ChildPageSnapshot, SelectedPath } from "../../app/store";
import type { ConversationSummary } from "../../bridge/types";
import { axeViolations, render, settle } from "../../test/render";
import { fixturePlugin, fixtureStatus, fixtureV2Plugin, fixtureV2Status, fixtureWhooperItems } from "../../test/plugins";
import { NO_STATUS_V2_WORK, statusV2Work, type StatusV2Work } from "../agents/statusV2";
import { AllWork, groupWork } from "./AllWork";
import type { HiddenEntry, NeedsYouFilter } from "./needsYou";

function node(id: string, title: string, parentId: string | null = null, overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return { id, title, parentId, hasChildren: false, summary: null,
    contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null, thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
    capabilities: { canSend: parentId === null, canInterrupt: parentId === null, canArchive: parentId === null, canRoute: parentId === null, unavailableReason: parentId ? "Recorded activity only." : null },
    routingProfile: "balanced", workspaceId: null, archived: false, projectRoot: "/work/alpha",
    currentRunId: "run-1", provider: "codex", runStatus: "running", rollupStatus: "active", ...overrides };
}
const root = node("root", "Auth refactor", null, { hasChildren: true });
const child = node("child", "API reviewer", "root", { hasChildren: true, provider: "claude" });
const grandchild = node("grandchild", "Schema researcher", "child", { runStatus: "completed", rollupStatus: "completed" });
const sibling = node("sibling", "Release notes", null, { projectRoot: null, provider: "claude", runStatus: "queued" });
function page(ids: string[], overrides: Partial<ChildPageSnapshot> = {}): ChildPageSnapshot {
  return { pages: [{ ids, cursor: null, order: 0 }], nextCursor: null, loading: false, error: null, evicted: false, ...overrides };
}
type Options = {
  conversations?: ConversationSummary[];
  conversationsById?: Record<string, ConversationSummary>;
  childPagesById?: Record<string, ChildPageSnapshot>;
  expandedById?: Record<string, boolean>;
  selectedId?: string | null;
  selectedPath?: SelectedPath | null;
  selectedEntry?: string | null;
  /** The sections a past launch left open, saved before rendering; null leaves what is saved. By default the chat sections this file uses are open. */
  open?: Record<string, boolean> | null;
  /** What plugins that print status format 2 add. */
  pluginWork?: StatusV2Work;
  hidden?: HiddenEntry[];
  filter?: NeedsYouFilter;
};
const SECTIONS = "prompting-time.sidebar-sections";

/** Renders All work with its disclosure and selection owned by signals, as App owns them through the store. */
function renderWork(options: Options = {}) {
  const open = options.open === undefined ? { "chats:/work/alpha": true, "chats:/work/beta": true, "chats:no-folder": true } : options.open;
  if (open !== null) window.localStorage.setItem(SECTIONS, JSON.stringify(open));
  const [conversations, setConversations] = createSignal(options.conversations ?? [root, sibling]);
  const [expandedById, setExpanded] = createSignal(options.expandedById ?? { root: true, child: true });
  const [selectedId, setSelected] = createSignal(options.selectedId === undefined ? "root" : options.selectedId);
  const [selectedPath, setSelectedPath] = createSignal(options.selectedPath ?? null);
  const [childPagesById, setChildPages] = createSignal(options.childPagesById ?? { root: page(["child"]), child: page(["grandchild"]) });
  const [query, setQuery] = createSignal("");
  const onSelect = vi.fn((id: string) => setSelected(id));
  const onToggle = vi.fn((id: string) => setExpanded((current) => ({ ...current, [id]: !current[id] })));
  const onLoadChildPage = vi.fn();
  const onOpenEntry = vi.fn();
  const onEntryAction = vi.fn();
  // Hide, Show again, and the filter change what App would pass back.
  const [hidden, setHidden] = createSignal<HiddenEntry[]>(options.hidden ?? []);
  const [filter, setFilter] = createSignal<NeedsYouFilter>(options.filter ?? "all");
  const onHide = vi.fn((entry: HiddenEntry) => setHidden((current) => [...current, entry]));
  const onShowAgain = vi.fn((key: string) => setHidden((current) => current.filter((entry) => entry.key !== key)));
  const onFilter = vi.fn((next: NeedsYouFilter) => setFilter(next));
  const view = render(() => (
    <AllWork
      selectedEntry={options.selectedEntry ?? null}
      query={query()}
      pluginWork={options.pluginWork ?? NO_STATUS_V2_WORK}
      actionTitle={(_plugin, action) => `${action[0]!.toUpperCase()}${action.slice(1)}`}
      onOpenEntry={onOpenEntry}
      onEntryAction={onEntryAction}
      hidden={hidden()}
      onHide={onHide}
      onShowAgain={onShowAgain}
      filter={filter()}
      onFilter={onFilter}
      pluginTitle={(plugin) => plugin}
      conversations={conversations()}
      conversationsById={options.conversationsById ?? { root, child, grandchild, sibling }}
      childPagesById={childPagesById()}
      expandedById={expandedById()}
      selectedId={selectedId()}
      selectedPath={selectedPath()}
      onSelect={onSelect}
      onToggle={onToggle}
      onLoadChildPage={onLoadChildPage}
    />
  ));
  /** Types into the sidebar's search, as App passes what it holds. */
  const search = (text: string) => {
    setQuery(text);
    flush();
  };
  return { ...view, onSelect, onToggle, onLoadChildPage, onOpenEntry, onEntryAction, onHide, onShowAgain, onFilter, hidden, setConversations, setExpanded, setSelected, setSelectedPath, setChildPages, search };
}
const press = (element: HTMLElement, key: string) => {
  fireEvent.keyDown(element, { key });
  flush();
};

describe("AllWork", () => {
  const today = new Date();
  const todayAt = (hours: number) => new Date(today.getFullYear(), today.getMonth(), today.getDate(), hours).toISOString();
  const whooperWork = (items: unknown[] = fixtureWhooperItems(), overrides: Record<string, unknown> = {}) => statusV2Work({
    loaded: true, error: null, items: [fixturePlugin()], statuses: { whooper: { value: fixtureStatus("Running", { items, ...overrides }), error: null } },
  });
  const names = (tree: HTMLElement) => [...tree.querySelectorAll<HTMLElement>(":scope > [role=treeitem]")].map((item) => item.getAttribute("aria-label"));
  const toggles = () => [...document.querySelectorAll<HTMLButtonElement>(".work-group-toggle")].map((button) => `${button.textContent} ${button.getAttribute("aria-expanded")}`);
  /** The headings of the open sections (".work-open") or of the folded ones stacked at the bottom (".work-folded"), in order. */
  const headings = (place: ".work-open" | ".work-folded") => [...document.querySelectorAll<HTMLButtonElement>(`${place} .work-group-toggle`)].map((button) => button.textContent);
  const waitingRoot = { ...root, id: "waiting", title: "Waiting conversation", projectRoot: null, rollupStatus: "needsAttention" as const };

  it("lists one Needs you in Command-J's order, each item with why it needs you and its plugin, and leaves the other groups to the plugin's page", async () => {
    const view = renderWork({ conversations: [waitingRoot, sibling], conversationsById: { waiting: waitingRoot, sibling }, pluginWork: whooperWork(), open: null });

    // Chats start folded; Reviewing and Done live in Whooper's page, not here.
    expect(toggles()).toEqual(["Needs you 4 true", "Chats · no folder 1 false"]);
    expect(names(screen.getByRole("tree", { name: "Needs you" }))).toEqual([
      "separate cache configuration, Ready to approve · Whooper",
      "add the tags package, Needs a look · Whooper",
      "move the projection, Review failed · Whooper",
      "Waiting conversation, Codex, waiting for you",
    ]);
    expect(screen.queryByRole("treeitem", { name: /stream uploads in chunks|rename the queue settings/ })).not.toBeInTheDocument();
    const ready = screen.getByRole("treeitem", { name: /^separate cache configuration/ });
    expect(ready).toHaveClass("work-entry");
    // Each row keeps its own dot beside the words that say it, a waiting chat's included.
    expect(ready.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "success");
    expect(screen.getByRole("treeitem", { name: /^add the tags package/ }).querySelector(".pt-status-dot")).toHaveAttribute("data-status", "attention");
    expect(screen.getByRole("treeitem", { name: /^move the projection/ }).querySelector(".pt-status-dot")).toHaveAttribute("data-status", "error");
    expect(screen.getByRole("treeitem", { name: /^Waiting conversation/ }).querySelector(".pt-status-dot")).toHaveAttribute("data-status", "attention");

    fireEvent.click(ready);
    expect(view.onOpenEntry).toHaveBeenCalledWith("whooper", "item:whooper:run-101", "/reviews/run-101");
    expect(view.onSelect).not.toHaveBeenCalled();
    expect(await axeViolations()).toEqual([]);
  });

  it("opens a folded section and keeps which are open across launches", () => {
    const view = renderWork({ conversations: [sibling], pluginWork: whooperWork(), open: {} });
    fireEvent.click(screen.getByRole("button", { name: "Needs you 3" }));
    fireEvent.click(screen.getByRole("button", { name: "Chats · no folder 1" }));
    flush();
    expect(names(screen.getByRole("tree", { name: "Chats · no folder" }))).toEqual(["Release notes, Claude, queued"]);
    expect(JSON.parse(window.localStorage.getItem(SECTIONS)!)).toEqual({ "needs-you": false, "chats:no-folder": true });

    view.unmount();
    renderWork({ conversations: [sibling], pluginWork: whooperWork(), open: null });
    // The open sections come back above the stack, and the folded ones in it, each in list order.
    expect(headings(".work-open")).toEqual(["Chats · no folder 1"]);
    expect(headings(".work-folded")).toEqual(["Needs you 3"]);
  });

  it("stacks the folded sections' headings below the open ones, outside their scrolling rows, and lists every item of a long Needs you", async () => {
    const many = Array.from({ length: 12 }, (_, index) => ({ ...fixtureWhooperItems()[0]!, id: `run-${200 + index}`, title: `ready ${index}`, updated_at: todayAt(20 - index) }));
    renderWork({ conversations: [sibling], pluginWork: whooperWork([...many, ...fixtureWhooperItems()]), open: {} });

    expect(names(screen.getByRole("tree", { name: "Needs you" }))).toHaveLength(15);
    expect(headings(".work-open")).toEqual(["Needs you 15"]);
    expect(headings(".work-folded")).toEqual(["Chats · no folder 1"]);
    // The rows scroll inside Needs you's body; the stack follows the open sections, outside it.
    const stack = document.querySelector(".work-folded")!;
    expect(screen.getByRole("tree", { name: "Needs you" }).closest(".work-section-body")).not.toBeNull();
    expect(stack.closest(".work-section-body")).toBeNull();
    expect(document.querySelector(".work-open")!.compareDocumentPosition(stack) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(await axeViolations()).toEqual([]);
  });

  it("splits the space above the stack between the open sections, each with its own rows, and moves across them with the arrow keys", () => {
    renderWork({ conversations: [sibling], pluginWork: whooperWork(), open: { "chats:no-folder": true }, selectedId: null });
    expect(headings(".work-open")).toEqual(["Needs you 3", "Chats · no folder 1"]);
    expect(document.querySelector(".work-folded")).toBeNull();
    const bodies = [...document.querySelectorAll(".work-open > .work-section > .work-section-body")];
    expect(bodies.map((body) => body.querySelectorAll("[role=treeitem]").length)).toEqual([3, 1]);

    const last = screen.getByRole("treeitem", { name: /^move the projection/ });
    last.focus();
    press(last, "ArrowDown");
    expect(document.activeElement).toHaveAttribute("aria-label", expect.stringContaining("Release notes"));
    expect(bodies[1]!.contains(document.activeElement)).toBe(true);
  });

  it("opens a section from the stack and folds it back with its heading, which keeps the focus as it moves", async () => {
    renderWork({ conversations: [sibling], pluginWork: whooperWork(), open: {} });
    const folded = screen.getByRole("button", { name: "Chats · no folder 1" });
    expect(folded).toHaveAttribute("aria-expanded", "false");
    // Enter or Space on the focused heading, which is a button, clicks it.
    folded.focus();
    fireEvent.click(folded);
    flush();
    await settle();
    const opened = screen.getByRole("button", { name: "Chats · no folder 1" });
    expect(opened).toHaveAttribute("aria-expanded", "true");
    expect(opened.closest(".work-open")).not.toBeNull();
    expect(opened).toHaveFocus();
    expect(headings(".work-open")).toEqual(["Needs you 3", "Chats · no folder 1"]);

    fireEvent.click(opened);
    flush();
    await settle();
    const again = screen.getByRole("button", { name: "Chats · no folder 1" });
    expect(again).toHaveAttribute("aria-expanded", "false");
    expect(again.closest(".work-folded")).not.toBeNull();
    expect(again).toHaveFocus();
    expect(screen.queryByRole("tree", { name: "Chats · no folder" })).not.toBeInTheDocument();
  });

  it("finds blockers by title, Needs you items by title, detail, or project folder, and chats by title or folder", async () => {
    const blocker = { id: "docker", title: "Docker isn't running", dot: "error", route: "/causes/docker", action: "round" };
    const inProject = { ...fixtureWhooperItems()[1]!, id: "run-60", title: "speed up the index", detail: "widgets#60", updated_at: "2026-11-26T14:00:00Z", project: "/work/gadgets" };
    const view = renderWork({ conversations: [root, sibling], pluginWork: whooperWork([...fixtureWhooperItems(), inProject], { blockers: [blocker] }), open: {} });
    const results = () => screen.getByRole("region", { name: "Search results" });
    const found = () => screen.getAllByRole("tree").map((tree) => `${document.getElementById(tree.getAttribute("aria-labelledby")!)?.textContent}: ${names(tree).join(" | ")}`);

    view.search("#101");
    expect(found()).toEqual(["Needs you: separate cache configuration, Ready to approve · Whooper"]);
    // Rows in results keep their own dots.
    expect(within(results()).getByRole("treeitem").querySelector(".pt-status-dot")).toHaveAttribute("data-status", "success");
    expect(within(results()).getByRole("status")).toHaveTextContent("1 result · Esc clears");
    view.search("@sam-fixture");
    expect(found()).toEqual(["Needs you: separate cache configuration, Ready to approve · Whooper"]);
    // Done and Reviewing live in Whooper's page, so the sidebar's search leaves them out.
    view.search("queue settings");
    expect(within(results()).getByRole("status")).toHaveTextContent("Nothing matches · Esc clears");
    view.search("docker");
    expect(found()).toEqual(["Needs you: Docker isn't running, blocks every job · Whooper"]);
    view.search("gadgets");
    expect(found()).toEqual([
      "Needs you: add the tags package, Needs a look · Whooper | speed up the index, Needs a look · Whooper | move the projection, Review failed · Whooper",
    ]);
    // The results scroll on their own, and a new search shows them from the top.
    results().scrollTop = 40;
    view.search("alpha");
    expect(results().scrollTop).toBe(0);
    expect(found()).toEqual(["Chats: Auth refactor, alpha, Codex, working"]);
    // Chats of every status mix here, so a chat keeps its dot.
    expect(within(results()).getByRole("treeitem").querySelector(".pt-status-dot")).toHaveAttribute("data-status", "running");
    expect(screen.queryByRole("button", { name: /^Collapse/ })).not.toBeInTheDocument();
    view.search("release no folder");
    expect(found()).toEqual(["Chats: Release notes, no folder, Claude, queued"]);
    view.search("nothing like this");
    expect(within(results()).getByRole("status")).toHaveTextContent("Nothing matches · Esc clears");
    expect(await axeViolations()).toEqual([]);

    view.search("");
    expect(screen.queryByRole("region", { name: "Search results" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Needs you 5" })).toBeVisible();
  });

  describe("hiding and filtering Needs you", () => {
    const evaluation = node("waiting", "Evaluation rows", null, { runStatus: "waiting", rollupStatus: "needsAttention" });
    const needsYouNames = () => names(screen.getByRole("tree", { name: "Needs you" }));
    const chips = () => [...document.querySelectorAll<HTMLButtonElement>(".work-filter")].map((chip) => `${chip.textContent}${chip.getAttribute("aria-pressed") === "true" ? " (chosen)" : ""}`);
    const click = (element: HTMLElement) => {
      fireEvent.click(element);
      flush();
    };
    const hideFromMenu = async (title: string) => {
      fireEvent.click(screen.getByRole("button", { name: `Actions for ${title}` }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "Hide" }));
      await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
      flush();
    };

    it("hides an item from its menu until Show again, keeping it in a folded Hidden line", async () => {
      const view = renderWork({ conversations: [evaluation], conversationsById: { waiting: evaluation }, pluginWork: whooperWork(), open: {} });
      await hideFromMenu("add the tags package");
      expect(view.onHide).toHaveBeenCalledExactlyOnceWith({ key: "item:whooper:run-77", plugin: "whooper", version: "2026-11-26T14:30:00Z" }, "add the tags package");
      expect(view.onEntryAction).not.toHaveBeenCalled();
      expect(screen.getByRole("button", { name: "Needs you 3" })).toBeVisible();
      expect(needsYouNames()).not.toContain("add the tags package, Needs a look · Whooper");

      // The fold says what it holds and how many, and opens on a click.
      const fold = screen.getByRole("button", { name: "Hidden · 1" });
      expect(fold).toHaveAttribute("aria-expanded", "false");
      expect(screen.queryByRole("list", { name: "Hidden" })).not.toBeInTheDocument();
      click(fold);
      expect(within(screen.getByRole("list", { name: "Hidden" })).getByRole("listitem")).toHaveTextContent("add the tags packageNeeds a look · WhooperShow again");
      expect(await axeViolations()).toEqual([]);
      click(screen.getByRole("button", { name: "Show add the tags package again" }));
      expect(view.onShowAgain).toHaveBeenCalledExactlyOnceWith("item:whooper:run-77");
      expect(needsYouNames()).toContain("add the tags package, Needs a look · Whooper");
      expect(screen.queryByRole("button", { name: /^Hidden/ })).not.toBeInTheDocument();
    });

    it("shows a hidden item again once it changes, and a blocker once its words or dot do", () => {
      const blockers = [{ id: "docker", title: "Docker isn't running", dot: "error", route: "/setup" }];
      renderWork({
        conversations: [], pluginWork: whooperWork(fixtureWhooperItems(), { blockers }), open: {},
        hidden: [
          { key: "item:whooper:run-101", plugin: "whooper", version: "2026-11-25T08:00:00Z" },
          { key: "item:whooper:run-77", plugin: "whooper", version: "2026-11-26T14:30:00Z" },
          { key: "blocker:whooper:docker", plugin: "whooper", version: "attention Docker isn't running" },
        ],
      });
      expect(needsYouNames()).toEqual([
        "Docker isn't running, blocks every job · Whooper",
        "separate cache configuration, Ready to approve · Whooper",
        "move the projection, Review failed · Whooper",
      ]);
      expect(screen.getByRole("button", { name: "Hidden · 1" })).toBeVisible();
    });

    it("hides a chat waiting on you, which then shows with its project's chats", async () => {
      const view = renderWork({ conversations: [evaluation, sibling], conversationsById: { waiting: evaluation, sibling }, open: { "chats:/work/alpha": true } });
      // Only chats wait, so there is nothing to filter.
      expect(chips()).toEqual([]);
      await hideFromMenu("Evaluation rows");
      expect(view.onHide).toHaveBeenCalledExactlyOnceWith({ key: "waiting", plugin: null, version: "" }, "Evaluation rows");
      expect(screen.queryByRole("tree", { name: "Needs you" })).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Needs you 0" })).toBeVisible();
      expect(screen.getByText("Nothing else needs you")).toBeVisible();
      expect(names(screen.getByRole("tree", { name: "Chats · alpha" }))).toEqual(["Evaluation rows, Codex, waiting for you"]);
      // Outside Needs you, a chat has no menu.
      expect(screen.queryByRole("button", { name: "Actions for Evaluation rows" })).not.toBeInTheDocument();
      click(screen.getByRole("button", { name: "Hidden · 1" }));
      expect(screen.getByRole("list", { name: "Hidden" })).toHaveTextContent("Evaluation rowsCodex · waiting for you");
    });

    it("filters Needs you by chats or plugin with a count on each chip, and searches only what the filter lists", async () => {
      const v2 = statusV2Work({
        loaded: true, error: null, items: [fixturePlugin(), fixtureV2Plugin()],
        statuses: { whooper: { value: fixtureStatus("Running", { items: fixtureWhooperItems() }), error: null }, "fixture-runs": { value: fixtureV2Status(), error: null } },
      });
      const view = renderWork({ conversations: [evaluation], conversationsById: { waiting: evaluation }, pluginWork: v2, open: { "chats:/work/alpha": true } });
      expect(chips()).toEqual(["All 7 (chosen)", "Chats 1", "Fixture runs 3", "Whooper 3"]);

      click(screen.getByRole("button", { name: "Chats 1" }));
      expect(view.onFilter).toHaveBeenCalledWith("chats");
      expect(needsYouNames()).toEqual(["Evaluation rows, Codex, waiting for you"]);
      // The heading still counts everything that needs you.
      expect(screen.getByRole("button", { name: "Needs you 7" })).toBeVisible();

      click(screen.getByRole("button", { name: "Fixture runs 3" }));
      expect(needsYouNames()).toEqual([
        "Codex is signed out, blocks every job · Fixture runs",
        "Review this stack of PRs · acme/widgets#60, Run failed · Fixture runs",
        "Tighten upload retries, Review needed · Fixture runs",
      ]);
      // The chat the filter leaves out shows with its project's chats.
      expect(names(screen.getByRole("tree", { name: "Chats · alpha" }))).toEqual(["Evaluation rows, Codex, waiting for you"]);

      view.search("tags");
      const results = screen.getByRole("region", { name: "Search results" });
      expect(within(results).getByRole("status")).toHaveTextContent("Nothing matches in fixture-runs · Esc clears");
      view.search("evaluation");
      expect(within(results).queryAllByRole("treeitem")).toEqual([]);
      view.search("retries");
      expect(within(results).getAllByRole("treeitem").map((row) => row.getAttribute("aria-label"))).toEqual(["Tighten upload retries, Review needed · Fixture runs"]);
      view.search("");

      // Hiding the filter's last entries leaves its chip, at 0, and says so.
      click(screen.getByRole("button", { name: "Chats 1" }));
      await hideFromMenu("Evaluation rows");
      expect(chips()).toEqual(["All 6", "Chats 0 (chosen)", "Fixture runs 3", "Whooper 3"]);
      expect(screen.getByText("No chat waits on you")).toBeVisible();
      expect(await axeViolations()).toEqual([]);
    });
  });

  describe("a plugin that prints status format 2", () => {
    const work = (status: Record<string, unknown> = fixtureV2Status()) => statusV2Work({
      loaded: true, error: null, items: [fixtureV2Plugin()], statuses: { "fixture-runs": { value: status, error: null } },
    });
    const failedTitle = "Review this stack of PRs · acme/widgets#60";

    it("draws its blockers first under Needs you, then its Needs you items, and leaves its other groups to its page", async () => {
      const view = renderWork({ conversations: [], pluginWork: work(), selectedEntry: "item:fixture-runs:run-review-2", open: {} });

      expect(toggles()).toEqual(["Needs you 3 true"]);
      expect(names(screen.getByRole("tree", { name: "Needs you" }))).toEqual([
        "Codex is signed out, blocks every job · Fixture runs",
        `${failedTitle}, Run failed · Fixture runs`,
        "Tighten upload retries, Review needed · Fixture runs",
      ]);
      const blocker = screen.getByRole("treeitem", { name: "Codex is signed out, blocks every job · Fixture runs" });
      expect(blocker.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "error");
      const failed = screen.getByRole("treeitem", { name: `${failedTitle}, Run failed · Fixture runs` });
      expect(failed.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "error");
      const review = screen.getByRole("treeitem", { name: "Tighten upload retries, Review needed · Fixture runs" });
      expect(review).toHaveAttribute("aria-selected", "true");
      expect(review.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "attention");
      expect(screen.queryByRole("treeitem", { name: /Score yesterday's sessions|Compare judge panels/ })).not.toBeInTheDocument();

      fireEvent.click(failed);
      expect(view.onOpenEntry).toHaveBeenCalledExactlyOnceWith("fixture-runs", "item:fixture-runs:run-74c91b1d", "/runs/run-74c91b1d");
      press(blocker, "Enter");
      expect(view.onOpenEntry).toHaveBeenLastCalledWith("fixture-runs", "blocker:fixture-runs:codex-signed-out", "/providers");
      expect(await axeViolations()).toEqual([]);
    });

    it("runs an item's action with --item and its id, and a blocker's with no arguments, from the row's menu without opening the row", async () => {
      const view = renderWork({ conversations: [], pluginWork: work(), open: {} });

      fireEvent.click(screen.getByRole("button", { name: `Actions for ${failedTitle}` }));
      expect(await screen.findByRole("menuitem", { name: "Rerun" })).toBeInTheDocument();
      fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
      await waitFor(() => expect(view.onEntryAction).toHaveBeenCalledExactlyOnceWith("fixture-runs", "delete", ["--item", "run-74c91b1d"], failedTitle));
      await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

      fireEvent.click(screen.getByRole("button", { name: "Actions for Codex is signed out" }));
      fireEvent.click(await screen.findByRole("menuitem", { name: "Check-providers" }));
      await waitFor(() => expect(view.onEntryAction).toHaveBeenLastCalledWith("fixture-runs", "check-providers", [], "Codex is signed out"));
      expect(view.onOpenEntry).not.toHaveBeenCalled();
    });

    it("shows no Needs you when nothing needs you", () => {
      renderWork({ conversations: [], pluginWork: work({ ...fixtureV2Status(), blockers: [], items: [fixtureV2Status().items[2]] }), open: {} });
      expect(toggles()).toEqual([]);
      expect(screen.queryByRole("tree")).not.toBeInTheDocument();
    });
  });

  it("moves between item and conversation rows with the arrow keys, past folded sections, and opens an item with Enter", () => {
    const view = renderWork({ conversations: [sibling], pluginWork: whooperWork(fixtureWhooperItems().filter(({ id }) => id === "run-101" || id === "run-55")), selectedId: null });
    const [first, second] = screen.getAllByRole("treeitem");
    first!.focus();
    press(first!, "ArrowDown");
    expect(document.activeElement).toBe(second);
    press(second!, "ArrowRight");
    press(second!, "Enter");
    expect(view.onOpenEntry).toHaveBeenCalledWith("whooper", "item:whooper:run-55", "/reviews/run-55");
    press(second!, "End");
    expect(document.activeElement).toHaveAttribute("aria-label", expect.stringContaining(sibling.title));
    press(document.activeElement as HTMLElement, "ArrowUp");
    expect(document.activeElement).toBe(second);
    expect(view.onToggle).not.toHaveBeenCalled();
  });

  it("shows root, child and grandchild with their depth, status words, and selection", async () => {
    renderWork();
    const rows = screen.getAllByRole("treeitem");
    expect(rows.map((row) => row.getAttribute("aria-label"))).toEqual([
      "Auth refactor, Codex, working", "API reviewer, Claude, working",
      "Schema researcher, Codex, completed", "Release notes, Claude, queued",
    ]);
    expect(rows.slice(0, 3).map((row) => row.getAttribute("aria-level"))).toEqual(["1", "2", "3"]);
    expect(rows.slice(0, 3).map((row) => row.style.getPropertyValue("--work-depth"))).toEqual(["0", "1", "2"]);
    expect(rows[0]).toHaveAttribute("aria-selected", "true");
    expect(rows[0]).toHaveTextContent("Codex · working");
    expect(rows[0]!.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "running");
    expect(await axeViolations()).toEqual([]);
  });

  it("puts roots waiting on you under Needs you first, once, then projects and conversations without one", () => {
    const waiting = node("waiting", "Evaluation rows", null, { runStatus: "waiting", rollupStatus: "needsAttention" });
    const blocked = node("blocked", "Tag names", null, { projectRoot: "/work/beta", rollupStatus: "needsAttention", runStatus: "running" });
    renderWork({ conversations: [root, waiting, sibling, blocked], conversationsById: { root, child, grandchild, sibling, waiting, blocked } });
    expect(screen.getAllByRole("tree").map((tree) => tree.getAttribute("aria-labelledby") && document.getElementById(tree.getAttribute("aria-labelledby")!)?.textContent))
      .toEqual(["Needs you", "Chats · alpha", "Chats · no folder"]);
    const needsYou = screen.getByRole("tree", { name: "Needs you" });
    expect([...needsYou.querySelectorAll("[role=treeitem]")].map((row) => row.getAttribute("aria-label"))).toEqual([
      "Evaluation rows, Codex, waiting for you", "Tag names, Codex, waiting for you",
    ]);
    // Beside "waiting for you", each row draws its orange dot.
    expect([...needsYou.querySelectorAll(".pt-status-dot")].map((dot) => dot.getAttribute("data-status"))).toEqual(["attention", "attention"]);
    expect(screen.queryByRole("tree", { name: "Chats · beta" })).not.toBeInTheDocument();
    expect(groupWork([sibling, root]).map(({ label }) => label)).toEqual(["alpha", "no folder"]);
  });

  it("groups chats by project, takes the caller's filtered roots, and shows no Needs you while nothing needs you", () => {
    const view = renderWork();
    expect(screen.getByRole("tree", { name: "Chats · alpha" })).toBeVisible();
    expect(screen.getByRole("tree", { name: "Chats · no folder" })).toBeVisible();
    view.setConversations([sibling]);
    flush();
    expect(screen.queryByRole("tree", { name: "Chats · alpha" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("treeitem")).toHaveLength(1);
    view.setConversations([]);
    flush();
    expect(screen.queryByRole("button", { name: /^Needs you/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("tree")).not.toBeInTheDocument();
  });

  it("names projects with the same folder name by their full paths", () => {
    renderWork({ conversations: [node("a", "First", null, { projectRoot: "/one/compiler" }), node("b", "Second", null, { projectRoot: "/two/compiler" })], open: { "chats:/one/compiler": true, "chats:/two/compiler": true } });
    expect(screen.getByRole("tree", { name: "Chats · compiler — /one/compiler" })).toBeVisible();
    expect(screen.getByRole("tree", { name: "Chats · compiler — /two/compiler" })).toBeVisible();
  });

  it("selects each conversation by its own ID without the disclosure button selecting", () => {
    const view = renderWork();
    fireEvent.click(screen.getByRole("treeitem", { name: /Schema researcher/ }));
    flush();
    expect(view.onSelect).toHaveBeenLastCalledWith("grandchild");
    expect(screen.getByRole("treeitem", { name: /Schema researcher/ })).toHaveAttribute("aria-selected", "true");
    fireEvent.click(screen.getByRole("button", { name: "Collapse Auth refactor" }));
    expect(view.onSelect).toHaveBeenCalledTimes(1);
    expect(view.onToggle).toHaveBeenCalledExactlyOnceWith("root");
  });

  it("hides and restores descendants from the caller's disclosure across remounts", () => {
    const view = renderWork({ expandedById: { root: false, child: true } });
    expect(screen.queryByRole("treeitem", { name: /API reviewer/ })).not.toBeInTheDocument();
    view.setExpanded({ root: true, child: true });
    flush();
    expect(screen.getByRole("treeitem", { name: /Schema researcher/ })).toBeVisible();
    view.unmount();
    renderWork({ expandedById: { root: false, child: true } });
    expect(screen.getByRole("treeitem", { name: /Auth refactor/ })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("treeitem", { name: /API reviewer/ })).not.toBeInTheDocument();
  });

  it("uses the arrow keys, Home, End, Enter and Space in visible depth-first order", () => {
    const view = renderWork({ expandedById: { root: false } });
    const rootRow = screen.getByRole("treeitem", { name: /Auth refactor/ });
    rootRow.focus();
    press(rootRow, "ArrowRight");
    expect(rootRow).toHaveAttribute("aria-expanded", "true");
    expect(screen.queryByRole("treeitem", { name: /Schema researcher/ })).not.toBeInTheDocument();
    press(rootRow, "ArrowRight");
    const childRow = screen.getByRole("treeitem", { name: /API reviewer/ });
    expect(childRow).toHaveFocus();
    press(childRow, "ArrowRight");
    press(childRow, "ArrowDown");
    const grandchildRow = screen.getByRole("treeitem", { name: /Schema researcher/ });
    expect(grandchildRow).toHaveFocus();
    press(grandchildRow, "Enter");
    expect(view.onSelect).toHaveBeenLastCalledWith("grandchild");
    press(grandchildRow, "ArrowLeft");
    expect(childRow).toHaveFocus();
    press(childRow, " ");
    expect(view.onSelect).toHaveBeenLastCalledWith("child");
    press(childRow, "ArrowLeft");
    expect(childRow).toHaveAttribute("aria-expanded", "false");
    press(childRow, "End");
    const last = screen.getByRole("treeitem", { name: /Release notes/ });
    expect(last).toHaveFocus();
    press(last, "ArrowUp");
    expect(childRow).toHaveFocus();
    press(childRow, "Home");
    expect(rootRow).toHaveFocus();
    expect(screen.getAllByRole("treeitem").filter((row) => row.tabIndex === 0)).toEqual([rootRow]);
  });

  it("leaves closed branches unread and sends paging, retry and reload to their parent", () => {
    const view = renderWork({ expandedById: { root: false }, childPagesById: { root: page([], { nextCursor: "next", error: "Page unavailable", evicted: true }) } });
    expect(screen.queryByRole("button", { name: "Retry conversations" })).not.toBeInTheDocument();
    expect(view.onLoadChildPage).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    flush();
    expect(view.onToggle).toHaveBeenCalledExactlyOnceWith("root");
    expect(screen.getByRole("alert")).toHaveTextContent("Page unavailable");
    fireEvent.click(screen.getByRole("button", { name: "Retry conversations" }));
    expect(view.onLoadChildPage).toHaveBeenLastCalledWith("root", true);
    fireEvent.click(screen.getByRole("button", { name: "Load more conversations" }));
    expect(view.onLoadChildPage).toHaveBeenLastCalledWith("root", false);
    fireEvent.click(screen.getByRole("button", { name: "Reload conversations" }));
    expect(view.onLoadChildPage).toHaveBeenLastCalledWith("root", true);
    expect(view.onSelect).not.toHaveBeenCalled();
  });

  it("disables paging while loading and keeps the eviction note visible", () => {
    renderWork({ childPagesById: { root: page([], { nextCursor: "next", loading: true, evicted: true }) } });
    expect(screen.getByRole("status")).toHaveTextContent("Loading conversations");
    expect(screen.getByRole("button", { name: "Load more conversations" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reload conversations" })).toBeDisabled();
    expect(screen.getByRole("note")).toHaveTextContent("outside this bounded view");
  });

  it("draws only retained child pages while keeping the selected path reachable", () => {
    const children = Array.from({ length: 200 }, (_, index) => node(`child-${index}`, `Child ${index}`, "root"));
    const view = renderWork({ conversations: [root], conversationsById: { root, ...Object.fromEntries(children.map((item) => [item.id, item])) },
      childPagesById: { root: page(children.slice(0, 80).map((item) => item.id), { nextCursor: "80" }) }, expandedById: { root: true } });
    expect(screen.getAllByRole("treeitem")).toHaveLength(81);
    expect(screen.queryByRole("treeitem", { name: /^Child 199,/ })).not.toBeInTheDocument();
    view.setSelected("child-199");
    view.setSelectedPath({ ids: ["root", "child-199"], truncated: false });
    flush();
    expect(screen.getByRole("treeitem", { name: /^Child 199,/ })).toHaveAttribute("aria-selected", "true");
  });

  it("keeps deep ancestry in aria-level while capping the visual indentation", () => {
    const nodes = Array.from({ length: 64 }, (_, index) => node(`depth-${index}`, `Depth ${index}`, index ? `depth-${index - 1}` : null, { hasChildren: index < 63 }));
    const ids = nodes.map((item) => item.id);
    renderWork({ conversations: [nodes[0]!], conversationsById: Object.fromEntries(nodes.map((item) => [item.id, item])),
      childPagesById: {}, selectedPath: { ids, truncated: false }, expandedById: Object.fromEntries(ids.map((id) => [id, true])) });
    const last = screen.getByRole("treeitem", { name: /^Depth 63,/ });
    expect(last).toHaveAttribute("aria-level", "64");
    expect(last.style.getPropertyValue("--work-depth")).toBe("8");
    expect(screen.getAllByRole("treeitem")).toHaveLength(64);
  });

  it("ignores duplicate, other parents' and archived child records", () => {
    const archived = { ...child, id: "archived", archived: true };
    renderWork({ conversations: [root], conversationsById: { root, child, grandchild, archived, sibling },
      childPagesById: { root: page(["child", "child", "grandchild", "sibling", "archived", "missing"]) }, expandedById: { root: true } });
    expect(screen.getAllByRole("treeitem")).toHaveLength(2);
    expect(screen.getByRole("treeitem", { name: /API reviewer/ })).toBeVisible();
  });
});
