import { fireEvent, screen, within } from "@testing-library/dom";
import { createSignal, flush } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type { ChildPageSnapshot, SelectedPath } from "../../app/store";
import type { ConversationSummary } from "../../bridge/types";
import { axeViolations, render } from "../../test/render";
import { fixtureReviews } from "../../test/plugins";
import { blockingCauses, newestRuns, readReviews, type PluginReview, type ReviewEntry } from "../agents/reviews";
import { AllWork, groupWork, type PluginCause } from "./AllWork";

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
  reviews?: PluginReview[];
  causes?: PluginCause[];
  selectedReview?: string | null;
  /** The sections a past launch left open, saved before rendering; null leaves what is saved. By default the chat sections this file uses are open. */
  open?: Record<string, boolean> | null;
  /** Whether the plugin has a remove action, which puts Remove on review rows. */
  canRemove?: boolean;
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
  const onOpenReview = vi.fn();
  const onOpenCause = vi.fn();
  const onRemoveReview = vi.fn();
  const view = render(() => (
    <AllWork
      reviews={options.reviews ?? []}
      causes={options.causes ?? []}
      selectedReview={options.selectedReview ?? null}
      query={query()}
      onOpenReview={onOpenReview}
      onOpenCause={onOpenCause}
      canRemove={() => options.canRemove ?? false}
      onRemoveReview={onRemoveReview}
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
  return { ...view, onSelect, onToggle, onLoadChildPage, onOpenReview, onOpenCause, onRemoveReview, setConversations, setExpanded, setSelected, setSelectedPath, setChildPages, search };
}
const press = (element: HTMLElement, key: string) => {
  fireEvent.keyDown(element, { key });
  flush();
};

describe("AllWork", () => {
  /** The fixture's reviews, today's being those of Nov 26, 2026, plus one to look at, one waiting on Docker, one skipped, and one removed. */
  const docker = { id: "docker", summary: "Docker isn't running", scope: "all" as const };
  const today = new Date();
  const todayAt = (hours: number) => new Date(today.getFullYear(), today.getMonth(), today.getDate(), hours).toISOString();
  const entries = (): ReviewEntry[] => readReviews({ reviews: [
    ...fixtureReviews().map((review) => ({ ...review, ...(review.started_at.startsWith("2026-11-26") ? { started_at: todayAt(9), ended_at: review.ended_at && todayAt(10) } : {}) })),
    { ...fixtureReviews()[0]!, run_id: "look-103", pr: 103, title: "cap the upload retry delay", verdict: "look_first", findings: 1, author: null, started_at: todayAt(6), ended_at: todayAt(7) },
    { ...fixtureReviews()[1]!, run_id: "waiting-60", pr: 60, repo: "acme/gadgets", title: "speed up the gadget index", outcome: "waiting", cause: docker, started_at: todayAt(8), ended_at: null },
    { ...fixtureReviews()[2]!, run_id: "skipped-9", pr: 9, repo: "acme/sprockets", title: "drop the sprocket shim", submitted: null, skipped_by_user: true, started_at: todayAt(7), ended_at: todayAt(8) },
    { ...fixtureReviews()[0]!, run_id: "removed-8", pr: 8, title: "keep focus in the date picker", removed_by_user: true, needs_you: false, started_at: todayAt(5), ended_at: todayAt(6) },
  ] });
  const pluginEntries = (): PluginReview[] => newestRuns(entries()).map((review) => ({ plugin: "whooper", review }));
  const causes = (): PluginCause[] => blockingCauses(entries(), null).map((cause) => ({ plugin: "whooper", cause }));
  const names = (tree: HTMLElement) => [...tree.querySelectorAll<HTMLElement>(":scope > [role=treeitem]")].map((item) => item.getAttribute("aria-label"));
  const toggles = () => [...document.querySelectorAll<HTMLButtonElement>(".work-group-toggle")].map((button) => `${button.textContent} ${button.getAttribute("aria-expanded")}`);
  const waitingRoot = { ...root, id: "waiting", title: "Waiting conversation", projectRoot: null, rollupStatus: "needsAttention" as const };

  it("folds every section but Needs you to its heading with a count, and splits Needs you by what each review needs", async () => {
    const view = renderWork({ conversations: [waitingRoot, sibling], conversationsById: { waiting: waitingRoot, sibling }, reviews: pluginEntries(), causes: causes(), open: null });

    expect(toggles()).toEqual([
      "Needs you 4 true", "Reviewing 1 false", "Waiting · Docker isn't running 1 false", "Chats · no folder 1 false", "Done 2 false",
    ]);
    expect(screen.getAllByRole("tree").map((tree) => document.getElementById(tree.getAttribute("aria-labelledby")!)?.textContent))
      .toEqual(["Ready to approve", "Needs a look", "Review failed", "Chats waiting on you"]);
    expect(names(screen.getByRole("tree", { name: "Ready to approve" }))).toEqual(["separate cache configuration, widgets#101 · @sam-fixture · 2 findings"]);
    expect(names(screen.getByRole("tree", { name: "Needs a look" }))).toEqual(["cap the upload retry delay, widgets#103 · 1 finding"]);
    expect(names(screen.getByRole("tree", { name: "Review failed" }))).toEqual(["move the projection, gadgets#55 · review failed"]);
    expect(names(screen.getByRole("tree", { name: "Chats waiting on you" }))).toEqual([expect.stringContaining("Waiting conversation")]);
    const ready = screen.getByRole("treeitem", { name: /^separate cache configuration/ });
    expect(ready).toHaveClass("work-review");
    expect(ready.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "attention");
    expect(screen.getByRole("treeitem", { name: /^move the projection/ }).querySelector(".pt-status-dot")).toHaveAttribute("data-status", "error");
    expect(screen.queryByRole("treeitem", { name: /keep focus in the date picker/ })).not.toBeInTheDocument();

    fireEvent.click(ready);
    expect(view.onOpenReview).toHaveBeenCalledWith("whooper", "20261126T150000Z-101");
    expect(view.onSelect).not.toHaveBeenCalled();
    expect(await axeViolations()).toEqual([]);
  });

  it("opens a folded section, keeps which are open across launches, and opens a cause from its Waiting row", () => {
    const view = renderWork({ conversations: [sibling], reviews: pluginEntries(), causes: causes(), open: {} });
    fireEvent.click(screen.getByRole("button", { name: "Waiting · Docker isn't running 1" }));
    flush();
    const cause = screen.getByRole("treeitem", { name: "Docker isn't running, 1 review waiting" });
    expect(cause.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "error");
    fireEvent.click(cause);
    expect(view.onOpenCause).toHaveBeenCalledWith("whooper", "docker");
    fireEvent.click(screen.getByRole("button", { name: "Needs you 3" }));
    fireEvent.click(screen.getByRole("button", { name: "Done 2" }));
    flush();
    expect(names(screen.getByRole("tree", { name: "Done" }))).toEqual([
      "drop the sprocket shim, sprockets#9 · skipped",
      expect.stringMatching(/^add the tags package, gadgets#77 · commented /),
    ]);
    expect(JSON.parse(window.localStorage.getItem(SECTIONS)!)).toEqual({ "waiting:cause:whooper:docker": true, "needs-you": false, done: true });

    view.unmount();
    renderWork({ conversations: [sibling], reviews: pluginEntries(), causes: causes(), open: null });
    expect(toggles()).toEqual(["Needs you 3 false", "Reviewing 1 false", "Waiting · Docker isn't running 1 true", "Chats · no folder 1 false", "Done 2 true"]);
    expect(screen.getByRole("tree", { name: "Waiting · Docker isn't running" })).toBeVisible();
  });

  it("shows five items per part of Needs you until Show N more, and the selected review even past them", () => {
    const many: PluginReview[] = Array.from({ length: 7 }, (_, index) => ({ plugin: "whooper", review: { ...entries()[0]!, run_id: `run-${index}`, pr: 200 + index, title: `ready ${index}`, ended_at: todayAt(20 - index) } }));
    const view = renderWork({ conversations: [], reviews: many });
    expect(names(screen.getByRole("tree", { name: "Ready to approve" }))).toHaveLength(5);
    expect(screen.getByRole("heading", { name: "Ready to approve · 7" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Show 2 more" }));
    flush();
    expect(names(screen.getByRole("tree", { name: "Ready to approve" }))).toHaveLength(7);
    expect(screen.queryByRole("button", { name: /more$/ })).not.toBeInTheDocument();

    view.unmount();
    renderWork({ conversations: [], reviews: many, selectedReview: "review:whooper:run-6" });
    expect(screen.getByRole("treeitem", { name: /^ready 6,/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("button", { name: /more$/ })).not.toBeInTheDocument();
  });

  it("finds a pull request by number, title words, author, or repo in every state, tags removed ones, and finds chats by title or folder", async () => {
    const view = renderWork({ conversations: [root, sibling], reviews: pluginEntries(), causes: causes(), open: {} });
    const results = () => screen.getByRole("region", { name: "Search results" });
    const found = () => screen.getAllByRole("tree").map((tree) => `${document.getElementById(tree.getAttribute("aria-labelledby")!)?.textContent}: ${names(tree).join(" | ")}`);

    view.search("#101");
    expect(found()).toEqual(["Needs you: separate cache configuration, widgets#101 · @sam-fixture · 2 findings"]);
    expect(within(results()).getByRole("status")).toHaveTextContent("1 result · Esc clears");
    view.search("101");
    expect(found()).toEqual(["Needs you: separate cache configuration, widgets#101 · @sam-fixture · 2 findings"]);
    view.search("sprocket shim");
    expect(found()).toEqual(["Done: drop the sprocket shim, sprockets#9 · skipped"]);
    view.search("@sam-fixture");
    expect(found()).toEqual([
      "Needs you: separate cache configuration, widgets#101 · @sam-fixture · 2 findings",
      "Removed: keep focus in the date picker, removed, widgets#8 · @sam-fixture · 2 findings",
    ]);
    const removed = screen.getByRole("treeitem", { name: /^keep focus in the date picker, removed/ });
    expect(removed).toHaveTextContent("removed");
    expect(removed).toHaveAttribute("data-removed");
    view.search("acme/gadgets");
    expect(found()).toEqual([
      "Needs you: move the projection, gadgets#55 · review failed",
      "Waiting: speed up the gadget index, gadgets#60 · waiting · Docker isn't running",
      expect.stringMatching(/^Done: add the tags package, gadgets#77 · commented /),
    ]);
    view.search("alpha");
    expect(found()).toEqual(["Chats: Auth refactor, alpha, Codex, working"]);
    expect(screen.queryByRole("button", { name: /^Collapse/ })).not.toBeInTheDocument();
    view.search("release no folder");
    expect(found()).toEqual(["Chats: Release notes, no folder, Claude, queued"]);
    view.search("nothing like this");
    expect(within(results()).getByRole("status")).toHaveTextContent("No PRs or chats match · Esc clears");
    expect(await axeViolations()).toEqual([]);

    view.search("");
    expect(screen.queryByRole("region", { name: "Search results" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Needs you 3" })).toBeVisible();
  });

  it("puts Remove on each review's row when the plugin can remove, without opening the review", () => {
    const view = renderWork({ conversations: [], reviews: pluginEntries(), canRemove: true });
    fireEvent.click(screen.getByRole("button", { name: "Remove widgets#101" }));
    expect(view.onRemoveReview).toHaveBeenCalledExactlyOnceWith("whooper", "20261126T150000Z-101");
    expect(view.onOpenReview).not.toHaveBeenCalled();
    view.search("keep focus");
    expect(screen.getByRole("treeitem", { name: /^keep focus in the date picker, removed/ })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Remove widgets#8" })).not.toBeInTheDocument();
  });

  it("moves between review and conversation rows with the arrow keys, past folded sections, and opens a review with Enter", () => {
    const view = renderWork({ conversations: [sibling], reviews: pluginEntries().filter(({ review }) => review.pr === 101 || review.pr === 55 || review.pr === 102), selectedId: null });
    const [first, second] = screen.getAllByRole("treeitem");
    first!.focus();
    press(first!, "ArrowDown");
    expect(document.activeElement).toBe(second);
    press(second!, "ArrowRight");
    press(second!, "Enter");
    expect(view.onOpenReview).toHaveBeenCalledWith("whooper", "20261124T090000Z-55");
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
      .toEqual(["Chats waiting on you", "Chats · alpha", "Chats · no folder"]);
    const needsYou = screen.getByRole("tree", { name: "Chats waiting on you" });
    expect([...needsYou.querySelectorAll("[role=treeitem]")].map((row) => row.getAttribute("aria-label"))).toEqual([
      "Evaluation rows, Codex, waiting for you", "Tag names, Codex, waiting for you",
    ]);
    expect(needsYou.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "attention");
    expect(screen.queryByRole("tree", { name: "Chats · beta" })).not.toBeInTheDocument();
    expect(groupWork([sibling, root]).map(({ label }) => label)).toEqual(["alpha", "no folder"]);
  });

  it("groups chats by project, takes the caller's filtered roots, and says when nothing needs you", () => {
    const view = renderWork();
    expect(screen.getByRole("tree", { name: "Chats · alpha" })).toBeVisible();
    expect(screen.getByRole("tree", { name: "Chats · no folder" })).toBeVisible();
    view.setConversations([sibling]);
    flush();
    expect(screen.queryByRole("tree", { name: "Chats · alpha" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("treeitem")).toHaveLength(1);
    view.setConversations([]);
    flush();
    expect(screen.getByText("Nothing needs you right now.")).toBeVisible();
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
