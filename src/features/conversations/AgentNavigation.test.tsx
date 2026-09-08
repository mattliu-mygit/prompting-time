import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { AppSnapshot, NormalizedConversation } from "../../app/store";
import type { AgentSnapshot } from "../../bridge/types";
import { AgentNavigation, agentPath, knownAgents } from "./AgentNavigation";

const agents: AgentSnapshot[] = [
  { id: "root", parentId: null, label: "Root", status: "running", provider: "codex", summary: null },
  { id: "child", parentId: "root", label: "Reviewer", status: "running", provider: "codex", summary: null },
  { id: "grandchild", parentId: "child", label: "Researcher", status: "completed", provider: "claude", summary: "Checked schema" },
];
const conversation: NormalizedConversation = {
  id: "c", title: "Compiler", currentRunId: "run", agentIds: ["root", "child"], archived: false,
  routingProfile: "bestFit", workspaceId: null, projectRoot: null, provider: "codex", runStatus: "running",
  rollupStatus: "active", agentsTruncated: true, summaryAgentsTruncated: true, summaryAgentIds: ["root", "child"],
};

it("derives nested ancestry, marks missing/cyclic paths and rejects stale selection", () => {
  expect(agentPath(agents, "grandchild")).toEqual({ nodes: agents, incomplete: false });
  expect(agentPath(agents.slice(1), "grandchild").incomplete).toBe(true);
  expect(agentPath([{ ...agents[0], parentId: "child" }, agents[1]], "child").incomplete).toBe(true);
  expect(agentPath(agents, "stale")).toEqual({ nodes: [], incomplete: true });
});

it("merges only the matching current run window and excludes inactive conversations", () => {
  const state = { conversationsById: { c: conversation, old: { ...conversation, id: "old", currentRunId: null } }, agentsById: Object.fromEntries(agents.map(a => [a.id, a])), agentWindow: { conversationId: "c", runId: "run", pages: [["grandchild"]] } } as unknown as AppSnapshot;
  expect(knownAgents(state).map(item => item.agent.id)).toEqual(["root", "child", "grandchild"]);
  expect(knownAgents({ ...state, agentWindow: { ...state.agentWindow!, runId: "old" } }).map(item => item.agent.id)).toEqual(["root", "child"]);
});

it("offers clickable ancestors, root and explicit incomplete-path paging", () => {
  const select = vi.fn(); const load = vi.fn();
  const view = render(<AgentNavigation title="Compiler" agents={agents} selectedId="grandchild" onSelect={select} />);
  expect(screen.getByRole("button", { name: "Researcher" })).toHaveAttribute("aria-current", "location");
  fireEvent.click(screen.getByRole("button", { name: "Reviewer" }));
  expect(select).toHaveBeenLastCalledWith("child");
  fireEvent.click(screen.getByRole("button", { name: "Compiler" }));
  expect(select).toHaveBeenLastCalledWith(undefined);
  view.rerender(<AgentNavigation title="Compiler" agents={agents.slice(1)} selectedId="grandchild" onSelect={select} onLoadMore={load} />);
  expect(screen.getByText("Agent path incomplete")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Load more agents" }));
  expect(load).toHaveBeenCalledOnce();
});
