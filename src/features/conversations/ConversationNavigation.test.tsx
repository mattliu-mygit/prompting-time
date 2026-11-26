import { fireEvent, screen } from "@testing-library/dom";
import { describe, expect, it, vi } from "vitest";
import type { ConversationSummary } from "../../bridge/types";
import { render } from "../../test/render";
import { ConversationNavigation } from "./ConversationNavigation";

function node(id: string, title: string, parentId: string | null): ConversationSummary {
  return { id, title, parentId, hasChildren: parentId === null, summary: null,
    contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null, thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
    capabilities: { canSend: parentId === null, canInterrupt: false, canArchive: parentId === null, canRoute: parentId === null, unavailableReason: null },
    routingProfile: "balanced", workspaceId: null, archived: false, projectRoot: null, currentRunId: "run", provider: "codex", runStatus: "completed", rollupStatus: "completed" };
}
const root = node("root", "Auth refactor", null);
const child = node("child", "API reviewer", "root");
const grandchild = node("grandchild", "Schema researcher", "child");
const byId = { root, child, grandchild };

describe("ConversationNavigation", () => {
  it("selects each ancestor by its own ID and marks the current descendant", () => {
    const onSelect = vi.fn();
    render(() => <ConversationNavigation conversation={grandchild} conversationsById={byId} path={{ ids: ["root", "child", "grandchild"], truncated: false }} onSelect={onSelect} />);
    const buttons = screen.getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual(["Auth refactor", "API reviewer", "Schema researcher"]);
    expect(buttons[2]).toHaveAttribute("aria-current", "location");
    fireEvent.click(buttons[1]!);
    expect(onSelect).toHaveBeenLastCalledWith("child");
    fireEvent.click(buttons[0]!);
    expect(onSelect).toHaveBeenLastCalledWith("root");
  });

  it("keeps the known parent when earlier ancestry is outside the bounded path", () => {
    render(() => <ConversationNavigation conversation={grandchild} conversationsById={byId} path={{ ids: ["child", "grandchild"], truncated: true }} onSelect={vi.fn()} />);
    expect(screen.getByText("Earlier ancestry is outside this view")).toBeVisible();
    expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual(["API reviewer", "Schema researcher"]);
  });

  it("does not invent missing ancestors and shows nothing for a root", () => {
    const view = render(() => <ConversationNavigation conversation={grandchild} conversationsById={{ grandchild }} path={{ ids: ["root", "child", "grandchild"], truncated: false }} onSelect={vi.fn()} />);
    expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual(["Schema researcher"]);
    view.unmount();
    render(() => <ConversationNavigation conversation={root} conversationsById={byId} path={{ ids: ["root"], truncated: false }} onSelect={vi.fn()} />);
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
  });
});
