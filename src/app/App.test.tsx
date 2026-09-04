import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { createAppStore, type AppApi } from "./store";

function createApi(overrides: Partial<AppApi> = {}): AppApi {
  return {
    getBootstrap: vi.fn().mockResolvedValue({
      providers: [
        {
          id: "codex",
          installed: true,
          available: true,
          version: "0.144.1",
          diagnostic: null,
          capabilities: [],
        },
        {
          id: "claude",
          installed: true,
          available: false,
          version: "2.1.205",
          diagnostic: "Claude protocol gate has not passed.",
          capabilities: [],
        },
      ],
    }),
    listConversations: vi.fn().mockResolvedValue({
      items: [
        {
          id: "c1",
          title: "Auth refactor",
          workspaceId: null,
          archived: false,
          projectRoot: null,
          currentRunId: "run-1",
          provider: "codex",
          runStatus: "queued",
          rollupStatus: "active",
          agents: [
            {
              id: "root-1",
              parentId: null,
              provider: "codex",
              label: "Root agent",
              summary: null,
              status: "queued",
            },
          ],
          agentsTruncated: false,
        },
      ],
      nextCursor: null,
    }),
    loadConversation: vi.fn().mockResolvedValue({
      id: "c1",
      title: "Auth refactor",
      workspaceId: null,
      archived: false,
      projectRoot: null,
      currentRunId: "run-1",
      provider: "codex",
      runStatus: "queued",
      rollupStatus: "active",
      agents: [],
      agentsTruncated: false,
    }),
    loadAgentTree: vi.fn().mockResolvedValue({ runId: null, items: [], nextCursor: null }),
    listenToAppEvents: vi.fn().mockResolvedValue(() => {}),
    ...overrides,
  };
}

describe("App", () => {
  it("renders the three-pane command center with queue and provider diagnostics", async () => {
    const store = createAppStore(createApi());
    render(<App store={store} />);

    expect(await screen.findByRole("treeitem", { name: /Auth refactor/ })).toBeVisible();
    expect(screen.getByRole("complementary", { name: "Conversations" })).toBeVisible();
    expect(screen.getByRole("main", { name: "Conversation workspace" })).toBeVisible();
    expect(screen.getByRole("complementary", { name: "Inspector" })).toBeVisible();
    expect(screen.getByText("1 queued")).toBeVisible();
    expect(screen.getByText("Codex 0.144.1")).toBeVisible();
    expect(screen.getByText(/Claude unavailable/)).toBeVisible();
  });

  it("filters by status and preserves a toolbar route back to the conversation tree", async () => {
    const store = createAppStore(createApi());
    render(<App store={store} />);
    await screen.findByRole("treeitem", { name: /Auth refactor/ });

    fireEvent.change(screen.getByRole("combobox", { name: "Filter conversations" }), {
      target: { value: "completed" },
    });
    expect(screen.queryByRole("treeitem", { name: /Auth refactor/ })).not.toBeInTheDocument();

    const sidebarButton = screen.getByRole("button", { name: "Hide conversations" });
    fireEvent.click(sidebarButton);
    expect(screen.getByRole("button", { name: "Show conversations" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  it("collapses and restores the inspector", async () => {
    const store = createAppStore(createApi());
    render(<App store={store} />);
    await screen.findByRole("treeitem", { name: /Auth refactor/ });

    fireEvent.click(screen.getByRole("button", { name: "Hide inspector" }));
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show inspector" }));
    expect(screen.getByRole("complementary", { name: "Inspector" })).toBeVisible();
  });

  it("shows a recoverable synchronization error", async () => {
    const api = createApi({
      listConversations: vi
        .fn()
        .mockRejectedValueOnce(new Error("Bridge disconnected"))
        .mockResolvedValue({ items: [], nextCursor: null }),
    });
    const store = createAppStore(api);
    render(<App store={store} />);

    expect(await screen.findByText("Bridge disconnected")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No conversation selected")).toBeVisible();
  });
});
