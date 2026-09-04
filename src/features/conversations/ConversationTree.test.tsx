import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AgentSnapshot, ConversationSummary } from "../../bridge/types";
import { ConversationTree } from "./ConversationTree";

function node(
  id: string,
  label: string,
  parentId: string | null,
  status: AgentSnapshot["status"],
  provider: AgentSnapshot["provider"] = "codex",
): AgentSnapshot {
  return { id, label, parentId, status, provider, summary: null };
}

function conversationWithThreeLevels(): ConversationSummary {
  return {
    id: "c1",
    title: "Auth refactor",
    workspaceId: "workspace-1",
    archived: false,
    projectRoot: "/work/alpha",
    currentRunId: "run-1",
    provider: "codex",
    runStatus: "waiting",
    rollupStatus: "needsAttention",
    agentsTruncated: false,
    agents: [
      node("root", "Root agent", null, "waiting"),
      node("reviewer", "API reviewer", "root", "running", "claude"),
      node("researcher", "Schema researcher", "reviewer", "waiting"),
    ],
  };
}

function queuedProjectless(): ConversationSummary {
  return {
    id: "c2",
    title: "Release notes",
    workspaceId: null,
    archived: false,
    projectRoot: null,
    currentRunId: "run-2",
    provider: "claude",
    runStatus: "queued",
    rollupStatus: "active",
    agentsTruncated: false,
    agents: [node("root-2", "Root agent", null, "queued", "claude")],
  };
}

describe("ConversationTree", () => {
  it("renders an orchestrating grandchild beneath its parent", () => {
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels()]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        onSelect={vi.fn()}
      />,
    );

    expect(screen.getByRole("treeitem", { name: /Auth refactor/ })).toHaveAttribute(
      "aria-level",
      "1",
    );
    expect(screen.getByRole("treeitem", { name: /API reviewer/ })).toHaveAttribute(
      "aria-level",
      "2",
    );
    expect(screen.getByRole("treeitem", { name: /Schema researcher/ })).toHaveAttribute(
      "aria-level",
      "3",
    );
  });

  it("groups project and projectless roots and exposes provider and rollup text", () => {
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels(), queuedProjectless()]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        onSelect={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "alpha" })).toBeVisible();
    expect(screen.getByRole("heading", { name: "No project" })).toBeVisible();
    expect(screen.getByText("Needs attention")).toHaveAttribute(
      "data-status",
      "needsAttention",
    );
    expect(screen.getAllByText("Codex").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Claude").length).toBeGreaterThan(0);
  });

  it("filters conversations by their effective visible status", () => {
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels(), queuedProjectless()]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="queued"
        onSelect={vi.fn()}
      />,
    );

    expect(screen.queryByRole("treeitem", { name: /Auth refactor/ })).not.toBeInTheDocument();
    expect(screen.getByRole("treeitem", { name: /Release notes/ })).toBeVisible();
  });

  it("selects a conversation from either its root or an agent", () => {
    const onSelect = vi.fn();
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels()]}
        selectedId={null}
        selectedAgentId={null}
        statusFilter="all"
        onSelect={onSelect}
      />,
    );

    fireEvent.click(screen.getByRole("treeitem", { name: /API reviewer/ }));
    expect(onSelect).toHaveBeenCalledWith("c1", "reviewer");
  });

  it("collapses and restores all descendants without losing the parent", () => {
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels()]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        onSelect={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Collapse Auth refactor" }));
    expect(screen.getByRole("treeitem", { name: /Auth refactor/ })).toBeVisible();
    expect(screen.queryByRole("treeitem", { name: /API reviewer/ })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    expect(screen.getByRole("treeitem", { name: /Schema researcher/ })).toBeVisible();
  });

  it("uses roving focus for visible depth-first keyboard traversal", () => {
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels()]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        onSelect={vi.fn()}
      />,
    );
    const root = screen.getByRole("treeitem", { name: /Auth refactor/ });
    const child = screen.getByRole("treeitem", { name: /API reviewer/ });
    const grandchild = screen.getByRole("treeitem", { name: /Schema researcher/ });

    root.focus();
    fireEvent.keyDown(root, { key: "ArrowDown" });
    expect(child).toHaveFocus();
    expect(child).toHaveAttribute("tabindex", "0");
    expect(root).toHaveAttribute("tabindex", "-1");
    fireEvent.keyDown(child, { key: "ArrowDown" });
    expect(grandchild).toHaveFocus();
    fireEvent.keyDown(grandchild, { key: "ArrowUp" });
    expect(child).toHaveFocus();
  });

  it("represents agent selection separately and activates it from the keyboard", () => {
    const onSelect = vi.fn();
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels()]}
        selectedId="c1"
        selectedAgentId="reviewer"
        statusFilter="all"
        onSelect={onSelect}
      />,
    );
    const conversation = screen.getByRole("treeitem", { name: /Auth refactor/ });
    const reviewer = screen.getByRole("treeitem", { name: /API reviewer/ });

    expect(conversation).toHaveAttribute("aria-selected", "false");
    expect(reviewer).toHaveAttribute("aria-selected", "true");
    reviewer.focus();
    fireEvent.keyDown(reviewer, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith("c1", "reviewer");
  });

  it("includes providers in accessible node names", () => {
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels()]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        onSelect={vi.fn()}
      />,
    );

    expect(screen.getByRole("treeitem", {
      name: "Auth refactor, Codex, Needs attention",
    })).toBeVisible();
    expect(screen.getByRole("treeitem", {
      name: "API reviewer, Claude, Running",
    })).toBeVisible();
  });

  it("disambiguates projects with the same basename by exact user-owned roots", () => {
    const second = {
      ...conversationWithThreeLevels(),
      id: "c3",
      title: "Other alpha task",
      projectRoot: "/other/alpha",
    };
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels(), second]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        onSelect={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "alpha — /work/alpha" })).toBeVisible();
    expect(screen.getByRole("heading", { name: "alpha — /other/alpha" })).toBeVisible();
  });
});
