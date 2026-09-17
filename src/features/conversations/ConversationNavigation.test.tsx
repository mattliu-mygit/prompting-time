import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { ConversationSummary } from "../../bridge/types";
import { ConversationNavigation } from "./ConversationNavigation";

function conversation(id: string, title: string, parentId: string | null): ConversationSummary {
  return { id, title, parentId, hasChildren: id !== "grandchild", summary: null,
    contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null, thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
    capabilities: { canSend: parentId === null, canInterrupt: parentId === null, canArchive: parentId === null, canRoute: parentId === null, unavailableReason: parentId ? "Recorded activity only." : null },
    routingProfile: "balanced", workspaceId: null, archived: false, projectRoot: null,
    currentRunId: "run", provider: "codex", runStatus: "running", rollupStatus: "active" };
}
const root = conversation("root", "Compiler", null);
const child = conversation("child", "Reviewer", "root");
const grandchild = conversation("grandchild", "Researcher", "child");
const conversationsById = { root, child, grandchild };

it("selects ordinary parent and root conversation IDs and marks the current descendant", () => {
  const onSelect = vi.fn();
  render(<ConversationNavigation conversation={grandchild} conversationsById={conversationsById} path={{ ids: ["root", "child", "grandchild"], truncated: false }} onSelect={onSelect} />);
  const ancestry = screen.getByRole("navigation", { name: "Conversation ancestry" });
  expect(within(ancestry).getAllByRole("button").map(button => button.textContent)).toEqual(["Compiler", "Reviewer", "Researcher"]);
  expect(screen.getByRole("button", { name: "Researcher" })).toHaveAttribute("aria-current", "location");
  fireEvent.click(screen.getByRole("button", { name: "Reviewer" }));
  expect(onSelect).toHaveBeenLastCalledWith("child");
  fireEvent.click(screen.getByRole("button", { name: "Compiler" }));
  expect(onSelect).toHaveBeenLastCalledWith("root");
});

it("keeps known parent navigation when earlier ancestry is outside the bounded path", () => {
  const onSelect = vi.fn();
  render(<ConversationNavigation conversation={grandchild} conversationsById={{ child, grandchild }} path={{ ids: ["child", "grandchild"], truncated: true }} onSelect={onSelect} />);
  expect(screen.getByText("Earlier ancestry is outside this view")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Compiler" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Reviewer" }));
  expect(onSelect).toHaveBeenCalledExactlyOnceWith("child");
});

it("does not fabricate missing ancestors or display root ancestry", () => {
  const onSelect = vi.fn();
  const view = render(<ConversationNavigation conversation={grandchild} conversationsById={{ grandchild }} path={null} onSelect={onSelect} />);
  expect(screen.getAllByRole("button")).toHaveLength(1);
  expect(screen.getByRole("button", { name: "Researcher" })).toHaveAttribute("aria-current", "location");
  view.rerender(<ConversationNavigation conversation={root} conversationsById={conversationsById} path={{ ids: ["root"], truncated: false }} onSelect={onSelect} />);
  expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
});
