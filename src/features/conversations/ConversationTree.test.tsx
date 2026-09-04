import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
    routingProfile: "balanced",
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
    routingProfile: "usageBalance",
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
  it("forgets disclosure state for evicted agents", async () => {
    const root = node("root", "Root", null, "running");
    const child = node("child", "Child", "root", "running");
    const grandchild = node("grandchild", "Grandchild", "child", "running");
    const base = { ...conversationWithThreeLevels(), agents: [root, child, grandchild] };
    const component = (conversations: ConversationSummary[]) => (
      <ConversationTree conversations={conversations} selectedId="c1" selectedAgentId={null} statusFilter="all" onSelect={vi.fn()} />
    );
    const view = render(component([base]));
    fireEvent.click(screen.getByRole("button", { name: `Expand ${base.title}` }));
    fireEvent.click(screen.getByRole("button", { name: "Expand Child" }));
    expect(screen.getByRole("treeitem", { name: /Child/ })).toHaveAttribute("aria-expanded", "true");

    view.rerender(component([{ ...base, agents: [root] }]));
    await waitFor(() => expect(screen.queryByRole("treeitem", { name: /Child/ })).not.toBeInTheDocument());
    view.rerender(component([base]));
    expect(screen.getByRole("treeitem", { name: /Child/ })).toHaveAttribute("aria-expanded", "false");
  });

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

    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    fireEvent.click(screen.getByRole("button", { name: "Expand API reviewer" }));

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

    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
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

    expect(screen.queryByRole("treeitem", { name: /API reviewer/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    fireEvent.click(screen.getByRole("button", { name: "Expand API reviewer" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    const child = screen.getByRole("treeitem", { name: /API reviewer/ });
    fireEvent.click(screen.getByRole("button", { name: "Expand API reviewer" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
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

    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));

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

  it("loads a truncated agent page only when its conversation is expanded", () => {
    const onLoadAgentPage = vi.fn();
    const truncated = {
      ...conversationWithThreeLevels(),
      agents: [node("root", "Root agent", null, "waiting")],
      agentsTruncated: true,
    };
    render(
      <ConversationTree
        conversations={[truncated]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        agentWindow={null}
        onLoadAgentPage={onLoadAgentPage}
        onSelect={vi.fn()}
      />,
    );

    expect(onLoadAgentPage).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    expect(onLoadAgentPage).toHaveBeenCalledWith("c1", true);
  });

  it("offers bounded next-page and restart controls for the active agent window", () => {
    const onLoadAgentPage = vi.fn();
    render(
      <ConversationTree
        conversations={[conversationWithThreeLevels()]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        agentWindow={{
          conversationId: "c1",
          runId: "run-1",
          pages: [["root", "reviewer", "researcher"]],
          nextCursor: "agents-2",
          loading: false,
          error: null,
          evicted: true,
        }}
        onLoadAgentPage={onLoadAgentPage}
        onSelect={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    fireEvent.click(screen.getByRole("button", { name: "Load more agents" }));
    expect(onLoadAgentPage).toHaveBeenCalledWith("c1", false);
    fireEvent.click(screen.getByRole("button", { name: "Reload first agents" }));
    expect(onLoadAgentPage).toHaveBeenCalledWith("c1", true);
    expect(screen.getByRole("note")).toHaveTextContent("outside this bounded view");
  });

  it("offers an explicit retry after the initial agent page fails", () => {
    const onLoadAgentPage = vi.fn();
    const truncated = {
      ...conversationWithThreeLevels(),
      agents: [node("root", "Root agent", null, "waiting")],
      agentsTruncated: true,
    };
    render(
      <ConversationTree
        conversations={[truncated]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        agentWindow={{
          conversationId: "c1",
          runId: "run-1",
          pages: [],
          nextCursor: null,
          loading: false,
          error: "Agent service unavailable.",
          evicted: false,
        }}
        onLoadAgentPage={onLoadAgentPage}
        onSelect={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Agent service unavailable.");
    fireEvent.click(screen.getByRole("button", { name: "Retry agents" }));
    expect(onLoadAgentPage).toHaveBeenCalledWith("c1", true);
  });

  it("keeps a very deep loaded hierarchy collapsed without recursive rendering", () => {
    const agents = [node("root", "Root agent", null, "waiting")];
    let parentId = "root";
    for (let index = 0; index < 20_000; index += 1) {
      const id = `deep-${index}`;
      agents.push(node(id, `Deep ${index}`, parentId, "queued"));
      parentId = id;
    }
    render(
      <ConversationTree
        conversations={[{ ...conversationWithThreeLevels(), agents }]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        onSelect={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    expect(screen.getByRole("treeitem", { name: /Deep 0/ })).toBeVisible();
    expect(screen.queryByRole("treeitem", { name: /Deep 1/ })).not.toBeInTheDocument();
  });

  it("hard-bounds mounted rows for a 200k-wide hierarchy", () => {
    const agents = [
      node("root", "Root agent", null, "waiting"),
      ...Array.from({ length: 200_000 }, (_, index) => node(`wide-${index}`, `Wide ${index}`, "root", "queued")),
    ];
    const view = render(
      <ConversationTree
        conversations={[{ ...conversationWithThreeLevels(), agents }]}
        selectedId="c1"
        selectedAgentId={null}
        statusFilter="all"
        onSelect={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Expand Auth refactor" }));
    expect(view.container.querySelectorAll('[role="treeitem"]')).toHaveLength(81);
  }, 10_000);
});
