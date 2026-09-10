import { fireEvent, render, screen } from "@testing-library/react";
import { useState, type ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";
import type { ChildPageSnapshot } from "../../app/store";
import type { ConversationSummary } from "../../bridge/types";
import { ConversationTree } from "./ConversationTree";

function node(id: string, title: string, parentId: string | null = null, overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return { id, title, parentId, hasChildren: false, summary: null,
    thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
    capabilities: { canSend: parentId === null, canInterrupt: parentId === null, canArchive: parentId === null, canRoute: parentId === null, unavailableReason: parentId ? "Recorded activity only." : null },
    routingProfile: "balanced", workspaceId: null, archived: false, projectRoot: "/work/alpha",
    currentRunId: "run-1", provider: "codex", runStatus: "running", rollupStatus: "active", ...overrides };
}
const root = node("root", "Auth refactor", null, { hasChildren: true, runStatus: "waiting", rollupStatus: "needsAttention" });
const child = node("child", "API reviewer", "root", { hasChildren: true, provider: "claude" });
const grandchild = node("grandchild", "Schema researcher", "child", { runStatus: "completed", rollupStatus: "completed" });
const sibling = node("sibling", "Release notes", null, { projectRoot: null, provider: "claude", runStatus: "queued" });
function page(ids: string[], overrides: Partial<ChildPageSnapshot> = {}): ChildPageSnapshot {
  return { pages: [{ ids, cursor: null, order: 0 }], nextCursor: null, loading: false, error: null, evicted: false, ...overrides };
}
function props(overrides: Partial<ComponentProps<typeof ConversationTree>> = {}): ComponentProps<typeof ConversationTree> {
  return { conversations: [root, sibling], conversationsById: { root, child, grandchild, sibling },
    childPagesById: { root: page(["child"]), child: page(["grandchild"]) }, expandedById: { root: true, child: true },
    selectedId: "root", selectedPath: null, onSelect: vi.fn(), onToggle: vi.fn(), onLoadChildPage: vi.fn(), ...overrides };
}
function ControlledTree({ initial = props() }: { initial?: ComponentProps<typeof ConversationTree> }) {
  const [expandedById, setExpanded] = useState(initial.expandedById);
  const [selectedId, setSelected] = useState(initial.selectedId);
  return <ConversationTree {...initial} expandedById={expandedById} selectedId={selectedId}
    onToggle={id => setExpanded(current => ({ ...current, [id]: !current[id] }))}
    onSelect={id => { setSelected(id); initial.onSelect(id); }} />;
}

describe("ConversationTree", () => {
  it("renders root, child and grandchild with distinct semantic and visual depth", () => {
    render(<ConversationTree {...props()} />);
    const rows = screen.getAllByRole("treeitem");
    expect(rows.map(row => row.getAttribute("aria-label"))).toEqual([
      "Auth refactor, Codex, Needs attention", "API reviewer, Claude, Active",
      "Schema researcher, Codex, Completed", "Release notes, Claude, Queued",
    ]);
    expect(rows.slice(0, 3).map(row => row.getAttribute("aria-level"))).toEqual(["1", "2", "3"]);
    expect(rows.slice(0, 3).map(row => row.style.getPropertyValue("--conversation-depth"))).toEqual(["0", "1", "2"]);
    expect(rows[0]).toHaveAttribute("aria-selected", "true");
  });
  it("groups project and projectless roots and accepts caller-filtered roots", () => {
    const view = render(<ConversationTree {...props()} />);
    expect(screen.getByRole("tree", { name: "alpha" })).toBeVisible();
    expect(screen.getByRole("tree", { name: "No project" })).toBeVisible();
    view.rerender(<ConversationTree {...props({ conversations: [sibling] })} />);
    expect(screen.queryByRole("tree", { name: "alpha" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("treeitem")).toHaveLength(1);
    view.rerender(<ConversationTree {...props({ conversations: [] })} />);
    expect(screen.getByText("No conversations match this status.")).toBeVisible();
  });
  it("disambiguates projects with the same basename using their exact roots", () => {
    render(<ConversationTree {...props({ conversations: [node("a", "First", null, { projectRoot: "/one/compiler" }), node("b", "Second", null, { projectRoot: "/two/compiler" })] })} />);
    expect(screen.getByRole("tree", { name: "compiler — /one/compiler" })).toBeVisible();
    expect(screen.getByRole("tree", { name: "compiler — /two/compiler" })).toBeVisible();
  });
  it("selects each conversation by its own ID without activating disclosure", () => {
    const initial = props();
    render(<ControlledTree initial={initial} />);
    fireEvent.click(screen.getByRole("treeitem", { name: /Schema researcher/ }));
    expect(initial.onSelect).toHaveBeenLastCalledWith("grandchild");
    expect(screen.getByRole("treeitem", { name: /Schema researcher/ })).toHaveAttribute("aria-selected", "true");
    fireEvent.click(screen.getByRole("button", { name: "Collapse Auth refactor" }));
    expect(initial.onSelect).toHaveBeenCalledTimes(1);
  });
  it("collapses and restores descendants using caller-owned disclosure across remounts", () => {
    const initial = props({ expandedById: { root: false, child: true } });
    const view = render(<ConversationTree {...initial} />);
    expect(screen.queryByRole("treeitem", { name: /API reviewer/ })).not.toBeInTheDocument();
    view.rerender(<ConversationTree {...initial} expandedById={{ root: true, child: true }} />);
    expect(screen.getByRole("treeitem", { name: /Schema researcher/ })).toBeVisible();
    view.unmount();
    render(<ConversationTree {...initial} />);
    expect(screen.getByRole("treeitem", { name: /Auth refactor/ })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("treeitem", { name: /API reviewer/ })).not.toBeInTheDocument();
  });
  it("uses all arrow keys, Home, End, Enter and Space for visible depth-first traversal", () => {
    const initial = props({ expandedById: { root: false } });
    render(<ControlledTree initial={initial} />);
    const rootRow = screen.getByRole("treeitem", { name: /Auth refactor/ });
    rootRow.focus();
    fireEvent.keyDown(rootRow, { key: "ArrowRight" });
    expect(rootRow).toHaveAttribute("aria-expanded", "true");
    expect(screen.queryByRole("treeitem", { name: /Schema researcher/ })).not.toBeInTheDocument();
    fireEvent.keyDown(rootRow, { key: "ArrowRight" });
    const childRow = screen.getByRole("treeitem", { name: /API reviewer/ });
    expect(childRow).toHaveFocus();
    fireEvent.keyDown(childRow, { key: "ArrowRight" });
    fireEvent.keyDown(childRow, { key: "ArrowDown" });
    const grandchildRow = screen.getByRole("treeitem", { name: /Schema researcher/ });
    expect(grandchildRow).toHaveFocus();
    fireEvent.keyDown(grandchildRow, { key: "Enter" });
    expect(initial.onSelect).toHaveBeenLastCalledWith("grandchild");
    fireEvent.keyDown(grandchildRow, { key: "ArrowLeft" });
    expect(childRow).toHaveFocus();
    fireEvent.keyDown(childRow, { key: " " });
    expect(initial.onSelect).toHaveBeenLastCalledWith("child");
    fireEvent.keyDown(childRow, { key: "ArrowLeft" });
    expect(childRow).toHaveAttribute("aria-expanded", "false");
    fireEvent.keyDown(childRow, { key: "End" });
    const last = screen.getByRole("treeitem", { name: /Release notes/ });
    expect(last).toHaveFocus();
    fireEvent.keyDown(last, { key: "ArrowUp" });
    expect(childRow).toHaveFocus();
    fireEvent.keyDown(childRow, { key: "Home" });
    expect(rootRow).toHaveFocus();
    expect(screen.getAllByRole("treeitem").filter(row => row.tabIndex === 0)).toEqual([rootRow]);
  });
  it("leaves hidden branches lazy and delegates paging/retry/restart to their exact parent", () => {
    const initial = props({ expandedById: { root: false }, childPagesById: { root: page([], { nextCursor: "next", error: "Page unavailable", evicted: true }) } });
    const view = render(<ConversationTree {...initial} />);
    expect(screen.queryByRole("button", { name: "Retry conversations" })).not.toBeInTheDocument();
    expect(initial.onLoadChildPage).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    expect(initial.onToggle).toHaveBeenCalledExactlyOnceWith("root");
    view.rerender(<ConversationTree {...initial} expandedById={{ root: true }} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Page unavailable");
    fireEvent.click(screen.getByRole("button", { name: "Retry conversations" }));
    expect(initial.onLoadChildPage).toHaveBeenLastCalledWith("root", true);
    fireEvent.click(screen.getByRole("button", { name: "Load more conversations" }));
    expect(initial.onLoadChildPage).toHaveBeenLastCalledWith("root", false);
    fireEvent.click(screen.getByRole("button", { name: "Reload conversations" }));
    expect(initial.onLoadChildPage).toHaveBeenLastCalledWith("root", true);
    expect(initial.onSelect).not.toHaveBeenCalled();
  });
  it("disables paging while loading and keeps eviction recovery visible", () => {
    render(<ConversationTree {...props({ childPagesById: { root: page([], { nextCursor: "next", loading: true, evicted: true }) } })} />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading conversations");
    expect(screen.getByRole("button", { name: "Load more conversations" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reload conversations" })).toBeDisabled();
    expect(screen.getByRole("note")).toHaveTextContent("outside this bounded view");
  });
  it("only mounts retained child pages while keeping the selected path reachable", () => {
    const children = Array.from({ length: 200 }, (_, index) => node(`child-${index}`, `Child ${index}`, "root"));
    const initial = props({ conversations: [root], conversationsById: { root, ...Object.fromEntries(children.map(child => [child.id, child])) },
      childPagesById: { root: page(children.slice(0, 80).map(child => child.id), { nextCursor: "80" }) }, expandedById: { root: true } });
    const view = render(<ConversationTree {...initial} />);
    expect(screen.getAllByRole("treeitem")).toHaveLength(81);
    expect(screen.queryByRole("treeitem", { name: /^Child 199,/ })).not.toBeInTheDocument();
    view.rerender(<ConversationTree {...initial} selectedId="child-199" selectedPath={{ ids: ["root", "child-199"], truncated: false }} />);
    expect(screen.getByRole("treeitem", { name: /^Child 199,/ })).toHaveAttribute("aria-selected", "true");
  });
  it("preserves deep semantic ancestry while capping visual indentation", () => {
    const nodes = Array.from({ length: 64 }, (_, index) => node(`depth-${index}`, `Depth ${index}`, index ? `depth-${index - 1}` : null, { hasChildren: index < 63 }));
    const ids = nodes.map(node => node.id);
    render(<ConversationTree {...props({ conversations: [nodes[0]!], conversationsById: Object.fromEntries(nodes.map(node => [node.id, node])),
      childPagesById: {}, selectedPath: { ids, truncated: false }, expandedById: Object.fromEntries(ids.map(id => [id, true])) })} />);
    const last = screen.getByRole("treeitem", { name: /^Depth 63,/ });
    expect(last).toHaveAttribute("aria-level", "64");
    expect(last.style.getPropertyValue("--conversation-depth")).toBe("8");
    expect(screen.getAllByRole("treeitem")).toHaveLength(64);
  });
  it("ignores duplicate, cross-parent and archived child records", () => {
    const archived = { ...child, id: "archived", archived: true };
    render(<ConversationTree {...props({ conversations: [root], conversationsById: { root, child, grandchild, archived, sibling },
      childPagesById: { root: page(["child", "child", "grandchild", "sibling", "archived", "missing"]) }, expandedById: { root: true } })} />);
    expect(screen.getAllByRole("treeitem")).toHaveLength(2);
    expect(screen.getByRole("treeitem", { name: /API reviewer/ })).toBeVisible();
  });
});
