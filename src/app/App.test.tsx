import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import axe from "axe-core";
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
          routingProfile: "bestFit",
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
            {
              id: "child-1",
              parentId: "root-1",
              provider: "claude",
              label: "Reviewer",
              summary: "Reviewing",
              status: "running",
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
      routingProfile: "bestFit",
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
    loadTimeline: vi.fn().mockResolvedValue({
      items: [], nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
    }),
    loadEventDetail: vi.fn(),
    loadApprovals: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadApprovalDetail: vi.fn(),
    loadApprovalQuestions: vi.fn(),
    submitMessage: vi.fn(),
    steerRun: vi.fn(),
    respondToApproval: vi.fn(),
    interruptRun: vi.fn(),
    inspectWorkspace: vi.fn().mockResolvedValue({
      workspace: { mode: "projectless", changes: [], truncated: false },
      executionPath: "/tmp/prompting-time/conversation-1",
      ownedWorktree: false,
      cleanup: { eligible: false, blocker: "notOwned" },
      currentRun: { id: "run-1", provider: "codex", status: "queued" },
      routing: null,
      handoff: null,
      activeDescendantCount: 1,
      agentsTruncated: false,
    }),
    inspectProject: vi.fn().mockResolvedValue({ isGit: true }),
    listRunAudits: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadRunAudit: vi.fn(),
    createConversation: vi.fn(),
    archiveConversation: vi.fn(),
    listenToAppEvents: vi.fn().mockResolvedValue(() => {}),
    ...overrides,
  };
}

describe("App", () => {
  it("creates projectless and project-backed conversations from an empty install", async () => {
    const createConversation = vi.fn()
      .mockResolvedValueOnce({
        id: "new-1", title: "Scratch", routingProfile: "balanced", workspaceId: null,
        archived: false, projectRoot: null, currentRunId: null, provider: null,
        runStatus: null, rollupStatus: null, agents: [], agentsTruncated: false,
      })
      .mockResolvedValueOnce({
        id: "new-2", title: "Repo work", routingProfile: "bestFit", workspaceId: "workspace-2",
        archived: false, projectRoot: "/repo", currentRunId: null, provider: null,
        runStatus: null, rollupStatus: null, agents: [], agentsTruncated: false,
      });
    const store = createAppStore(createApi({
      listConversations: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
      createConversation,
    }));
    render(<App store={store} />);

    fireEvent.click(await screen.findByRole("button", { name: "New conversation" }));
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Scratch" } });
    fireEvent.change(screen.getByLabelText("Objective"), { target: { value: "Explore" } });
    fireEvent.click(screen.getByRole("button", { name: "Create conversation" }));
    await waitFor(() => expect(createConversation).toHaveBeenNthCalledWith(1, expect.objectContaining({
      title: "Scratch", workspace: { kind: "projectless" },
    })));

    fireEvent.click(screen.getByRole("button", { name: "New conversation" }));
    const dialog = screen.getByRole("dialog", { name: "New conversation" });
    fireEvent.change(within(dialog).getByLabelText("Title"), { target: { value: "Repo work" } });
    fireEvent.change(within(dialog).getByLabelText("Objective"), { target: { value: "Implement" } });
    fireEvent.change(within(dialog).getByLabelText("Workspace"), { target: { value: "project" } });
    fireEvent.change(within(dialog).getByLabelText("Project root"), { target: { value: "/repo" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Check directory" }));
    await within(dialog).findByRole("combobox", { name: "Execution" });
    fireEvent.change(within(dialog).getByLabelText("Routing profile"), { target: { value: "bestFit" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create conversation" }));
    await waitFor(() => expect(createConversation).toHaveBeenNthCalledWith(2, expect.objectContaining({
      title: "Repo work", workspace: { kind: "isolated", path: "/repo" }, routingProfile: "bestFit",
    })));
  });

  it("requires deliberate confirmation before archiving the current conversation", async () => {
    const archiveConversation = vi.fn().mockResolvedValue(undefined);
    const store = createAppStore(createApi({ archiveConversation }));
    render(<App store={store} />);
    await screen.findByRole("heading", { name: "Timeline" });

    fireEvent.click(screen.getByRole("button", { name: "Archive conversation" }));
    expect(screen.getByRole("dialog", { name: "Archive Auth refactor" })).toBeVisible();
    expect(archiveConversation).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Confirm archive" }));

    await waitFor(() => expect(archiveConversation).toHaveBeenCalledWith({ conversationId: "c1" }));
    expect(await screen.findByText("No conversation selected")).toBeVisible();
  });

  it("keeps lifecycle dialogs keyboard-modal and restores their trigger", async () => {
    const store = createAppStore(createApi());
    render(<App store={store} />);
    const trigger = await screen.findByRole("button", { name: "Archive conversation" });
    fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Archive Auth refactor" });
    expect(document.querySelector(".command-center")).toHaveAttribute("inert");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(trigger).toHaveFocus());

    const create = screen.getByRole("button", { name: "New conversation" });
    fireEvent.click(create);
    const createDialog = screen.getByRole("dialog", { name: "New conversation" });
    expect(screen.queryByRole("combobox", { name: "Execution" })).not.toBeInTheDocument();
    fireEvent.keyDown(createDialog, { key: "Escape" });
    await waitFor(() => expect(create).toHaveFocus());
  });

  it("makes the complete app background inert while the portaled provider-switch modal is open", async () => {
    const browserFocus = HTMLElement.prototype.focus;
    const focus = vi.spyOn(HTMLElement.prototype, "focus").mockImplementation(function (this: HTMLElement) {
      if (this.closest("[inert]")) return;
      browserFocus.call(this);
    });
    const store = createAppStore(createApi({
      getBootstrap: vi.fn().mockResolvedValue({
        providers: [
          { id: "codex", installed: true, available: true, version: "1", diagnostic: null, capabilities: ["interruption"] },
          { id: "claude", installed: true, available: true, version: "2", diagnostic: null, capabilities: ["interruption"] },
        ],
      }),
    }));
    try {
      const { container } = render(<App store={store} />);
      const provider = await screen.findByRole("combobox", { name: "Provider" });
      fireEvent.change(provider, { target: { value: "claude" } });
      const dialog = screen.getByRole("dialog", { name: "Interrupt Codex to switch provider" });

      expect(container.querySelector(".app-shell")).toHaveAttribute("inert");
      expect(container).not.toContainElement(dialog);
      fireEvent.click(within(dialog).getByRole("button", { name: "Keep Codex running" }));
      await waitFor(() => expect(container.querySelector(".app-shell")).not.toHaveAttribute("inert"));
      await waitFor(() => expect(provider).toHaveFocus());
    } finally {
      focus.mockRestore();
    }
  });

  it("surfaces a project preflight failure and permits retry", async () => {
    const inspectProject = vi.fn()
      .mockRejectedValueOnce(new Error("Directory is unavailable"))
      .mockResolvedValueOnce({ isGit: true });
    const store = createAppStore(createApi({ inspectProject }));
    render(<App store={store} />);
    fireEvent.click(await screen.findByRole("button", { name: "New conversation" }));
    const dialog = screen.getByRole("dialog", { name: "New conversation" });
    fireEvent.change(within(dialog).getByLabelText("Workspace"), { target: { value: "project" } });
    fireEvent.change(within(dialog).getByLabelText("Project root"), { target: { value: "/repo" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Check directory" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Directory is unavailable");
    fireEvent.click(within(dialog).getByRole("button", { name: "Check directory" }));
    expect(await within(dialog).findByRole("combobox", { name: "Execution" })).toBeVisible();
    expect(inspectProject).toHaveBeenCalledTimes(2);
  });

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
    expect(screen.getByRole("option", { name: "Auto · Best fit" })).toBeVisible();
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

  it("has no detectable axe violations in the three-pane workspace", async () => {
    const store = createAppStore(createApi());
    const { container } = render(<App store={store} />);
    await screen.findByRole("heading", { name: "Timeline" });

    const result = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(result.violations.map(({ id }) => id)).toEqual([]);
  });

  it("restores focus to the inspector trigger when the overlay closes", async () => {
    const store = createAppStore(createApi());
    render(<App store={store} />);
    await screen.findByRole("heading", { name: "Timeline" });

    fireEvent.click(screen.getByRole("button", { name: "Close inspector" }));
    const trigger = screen.getByRole("button", { name: "Show inspector" });
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("moves focus into the narrow inspector overlay, contains Tab, and restores the trigger", async () => {
    vi.stubGlobal("matchMedia", vi.fn((query: string) => ({
      matches: query === "(max-width: 56rem)", media: query, onchange: null,
      addListener: vi.fn(), removeListener: vi.fn(), addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: vi.fn(),
    })));
    const store = createAppStore(createApi());
    render(<App store={store} />);
    await screen.findByRole("heading", { name: "Timeline" });
    await waitFor(() => expect(screen.getByRole("button", { name: "Close inspector" })).toHaveFocus());
    fireEvent.click(screen.getByRole("button", { name: "Close inspector" }));
    const trigger = screen.getByRole("button", { name: "Show inspector" });
    fireEvent.click(trigger);

    const close = await screen.findByRole("button", { name: "Close inspector" });
    await waitFor(() => expect(close).toHaveFocus());
    expect(screen.getByRole("main", { name: "Conversation workspace" })).toHaveAttribute("inert");
    fireEvent.keyDown(screen.getByRole("complementary", { name: "Inspector" }), { key: "Tab", shiftKey: true });
    expect(document.activeElement).not.toBe(trigger);
    fireEvent.keyDown(screen.getByRole("complementary", { name: "Inspector" }), { key: "Escape" });
    await waitFor(() => expect(trigger).toHaveFocus());
    vi.unstubAllGlobals();
  });
});
