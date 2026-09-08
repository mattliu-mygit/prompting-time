import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { StrictMode } from "react";
import axe from "axe-core";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { createAppStore, type AppApi } from "./store";
import { BridgeError } from "../bridge/api";
import type { ConversationSummary } from "../bridge/types";

function createdConversation(overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return {
    id: "new-1", title: "synthetic-project", routingProfile: "bestFit", workspaceId: "w-1",
    archived: false, projectRoot: "/tmp/synthetic-project", currentRunId: null, provider: null,
    runStatus: null, rollupStatus: null, agents: [], agentsTruncated: false,
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

async function openChooser(overrides: Partial<AppApi> = {}, strict = false) {
  const api = createApi(overrides);
  const store = createAppStore(api);
  const app = <App store={store} />;
  const rendered = render(strict ? <StrictMode>{app}</StrictMode> : app);
  const trigger = await screen.findByRole("button", { name: "New conversation" });
  fireEvent.click(trigger);
  const dialog = screen.getByRole("dialog", { name: "New conversation" });
  return { ...rendered, api, store, trigger, dialog, choose: within(dialog).getByRole("button", { name: "Choose folder" }) };
}

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
    loadDiagnostics: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
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
    pickProjectDirectory: vi.fn().mockResolvedValue("/tmp/synthetic-project"),
    listRunAudits: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadRunAudit: vi.fn(),
    createConversation: vi.fn().mockResolvedValue(createdConversation()),
    archiveConversation: vi.fn(),
    listenToAppEvents: vi.fn().mockResolvedValue(() => {}),
    ...overrides,
  };
}

describe("App", () => {
  it("chooses a Git folder and creates an empty Best fit conversation from an empty install", async () => {
    const { api, dialog, choose, store } = await openChooser({
      listConversations: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    }, true);
    expect(within(dialog).queryByLabelText("Title")).not.toBeInTheDocument();
    expect(within(dialog).queryByLabelText("Objective")).not.toBeInTheDocument();
    expect(dialog).toHaveTextContent(/uncommitted/i);
    fireEvent.click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith({
      title: "synthetic-project", objective: "", constraints: [],
      workspace: { kind: "isolated", path: "/tmp/synthetic-project" }, routingProfile: "bestFit",
    }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(store.getSnapshot().selectedConversationId).toBe("new-1");
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus();
    expect(api.submitMessage).not.toHaveBeenCalled();
  });

  it("restores chooser focus when the native picker is canceled", async () => {
    const { api, choose, dialog } = await openChooser({ pickProjectDirectory: vi.fn().mockResolvedValue(null) });
    fireEvent.click(choose);
    await waitFor(() => expect(choose).toBeEnabled());
    expect(choose).toHaveFocus();
    expect(api.inspectProject).not.toHaveBeenCalled();
    expect(api.createConversation).not.toHaveBeenCalled();
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });

  it.each(["picker", "preflight", "preparation"] as const)("recovers from %s errors without falling back", async (stage) => {
    const failing = vi.fn().mockRejectedValueOnce(new Error("Directory is unavailable"));
    const overrides: Partial<AppApi> = stage === "picker"
      ? { pickProjectDirectory: failing.mockResolvedValue("/tmp/synthetic-project") }
      : stage === "preflight"
      ? { inspectProject: failing.mockResolvedValue({ isGit: true }) }
      : { createConversation: failing.mockResolvedValue(createdConversation()) };
    const { api, dialog, choose } = await openChooser(overrides);
    fireEvent.click(choose);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Directory is unavailable");
    expect(api.createConversation).toHaveBeenCalledTimes(stage === "preparation" ? 1 : 0);
    fireEvent.click(choose);
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(api.createConversation).toHaveBeenLastCalledWith(expect.objectContaining({
      workspace: { kind: "isolated", path: "/tmp/synthetic-project" },
    }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("uses a non-Git directory directly despite the isolation default", async () => {
    const { api, choose } = await openChooser({ inspectProject: vi.fn().mockResolvedValue({ isGit: false }) });
    fireEvent.click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith(expect.objectContaining({
      workspace: { kind: "direct", path: "/tmp/synthetic-project" },
    })));
  });

  it.each(["balanced", "usageBalance"] as const)("honors Current checkout and the %s routing override", async (routingProfile) => {
    const { api, choose, dialog } = await openChooser();
    fireEvent.click(within(dialog).getByRole("button", { name: "Advanced" }));
    fireEvent.change(within(dialog).getByLabelText("Git execution"), { target: { value: "direct" } });
    fireEvent.change(within(dialog).getByLabelText("Routing profile"), { target: { value: routingProfile } });
    fireEvent.click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith(expect.objectContaining({
      routingProfile, workspace: { kind: "direct", path: "/tmp/synthetic-project" },
    })));
  });

  it.each([["/tmp/synthetic-project///", "synthetic-project"], ["/", "New conversation"]])("derives the title for %s", async (path, title) => {
    const { api, choose } = await openChooser({ pickProjectDirectory: vi.fn().mockResolvedValue(path) });
    fireEvent.click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith(expect.objectContaining({ title })));
  });

  it("creates without a folder and sends only the ordinary first message", async () => {
    const submitMessage = vi.fn().mockResolvedValue({});
    const { api, dialog } = await openChooser({ submitMessage });
    fireEvent.click(within(dialog).getByRole("button", { name: "Without a folder" }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(api.createConversation).toHaveBeenCalledWith({
      title: "New conversation", objective: "", constraints: [], workspace: { kind: "projectless" }, routingProfile: "bestFit",
    });
    expect(api.pickProjectDirectory).not.toHaveBeenCalled();
    expect(api.inspectProject).not.toHaveBeenCalled();
    expect(submitMessage).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Explain the synthetic project" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(submitMessage).toHaveBeenCalledWith({
      conversationId: "new-1", text: "Explain the synthetic project", providerOverride: null, commandId: expect.any(String),
    }));
  });

  it.each(["picker", "preflight"] as const)("ignores a late %s result after unmount", async (stage) => {
    const picker = deferred<string | null>();
    const preflight = deferred<{ isGit: boolean }>();
    const { api, choose, unmount } = await openChooser(stage === "picker"
      ? { pickProjectDirectory: vi.fn().mockReturnValue(picker.promise) }
      : { inspectProject: vi.fn().mockReturnValue(preflight.promise) });
    fireEvent.click(choose);
    if (stage === "preflight") await waitFor(() => expect(api.inspectProject).toHaveBeenCalledTimes(1));
    unmount();
    await act(async () => { picker.resolve("/tmp/synthetic-project"); preflight.resolve({ isGit: true }); });
    expect(api.createConversation).not.toHaveBeenCalled();
    if (stage === "picker") expect(api.inspectProject).not.toHaveBeenCalled();
  });

  it.each(["folder", "projectless"] as const)("admits only one same-tick operation starting with %s", async (intent) => {
    const picker = deferred<string | null>();
    const creation = deferred<ConversationSummary>();
    const { api, choose, dialog } = await openChooser({
      pickProjectDirectory: vi.fn().mockReturnValue(picker.promise),
      createConversation: vi.fn().mockReturnValue(creation.promise),
    });
    const projectless = within(dialog).getByRole("button", { name: "Without a folder" });
    act(() => {
      for (const button of intent === "folder" ? [choose, choose, projectless] : [projectless, projectless, choose]) {
        button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      }
    });
    expect(api.pickProjectDirectory).toHaveBeenCalledTimes(intent === "folder" ? 1 : 0);
    expect(api.createConversation).toHaveBeenCalledTimes(intent === "projectless" ? 1 : 0);
    await act(async () => { picker.resolve("/tmp/synthetic-project"); });
    expect(api.createConversation).toHaveBeenCalledTimes(1);
    await act(async () => { creation.resolve(createdConversation()); });
  });

  it("keeps options and cancellation disabled throughout picker, preflight, and creation", async () => {
    const picker = deferred<string | null>();
    const preflight = deferred<{ isGit: boolean }>();
    const creation = deferred<ConversationSummary>();
    const { choose, dialog } = await openChooser({
      pickProjectDirectory: vi.fn().mockReturnValue(picker.promise),
      inspectProject: vi.fn().mockReturnValue(preflight.promise),
      createConversation: vi.fn().mockReturnValue(creation.promise),
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Advanced" }));
    fireEvent.click(choose);
    const checkBusy = () => {
      expect(dialog).toHaveAttribute("aria-busy", "true");
      for (const control of within(dialog).getAllByRole("button")) expect(control).toBeDisabled();
      for (const control of within(dialog).getAllByRole("combobox")) expect(control).toBeDisabled();
      fireEvent.keyDown(dialog, { key: "Escape" });
      expect(dialog).toBeInTheDocument();
      expect(fireEvent.keyDown(dialog, { key: "Tab" })).toBe(false);
    };
    checkBusy();
    await act(async () => { picker.resolve("/tmp/synthetic-project"); });
    checkBusy();
    await act(async () => { preflight.resolve({ isGit: true }); });
    checkBusy();
    await act(async () => { creation.resolve(createdConversation()); });
  });

  it("traps focus using only visible Advanced controls and restores Cancel focus", async () => {
    const { dialog, choose, trigger } = await openChooser();
    const advanced = within(dialog).getByRole("button", { name: "Advanced" });
    const cancel = within(dialog).getByRole("button", { name: "Cancel" });
    expect(advanced).toHaveAttribute("aria-expanded", "false");
    expect(within(dialog).queryByLabelText("Git execution")).not.toBeInTheDocument();
    expect(within(dialog).queryByLabelText("Routing profile")).not.toBeInTheDocument();
    choose.focus();
    fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true });
    expect(cancel).toHaveFocus();
    fireEvent.keyDown(dialog, { key: "Tab" });
    expect(choose).toHaveFocus();
    fireEvent.click(advanced);
    expect(advanced).toHaveAttribute("aria-expanded", "true");
    expect(within(dialog).getByLabelText("Git execution")).toBeVisible();
    fireEvent.click(cancel);
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("focuses the committed new composer after removing inert and closing a narrow inspector", async () => {
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({
      matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn(),
    }));
    const focus = HTMLElement.prototype.focus;
    const focusSpy = vi.spyOn(HTMLElement.prototype, "focus").mockImplementation(function (this: HTMLElement) {
      if (!this.closest("[inert]")) focus.call(this);
    });
    try {
      const { dialog } = await openChooser();
      fireEvent.click(within(dialog).getByRole("button", { name: "Without a folder" }));
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
      expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
      expect(document.querySelector(".command-center")).not.toHaveAttribute("inert");
    } finally {
      focusSpy.mockRestore();
      vi.unstubAllGlobals();
    }
  });

  it("retains an existing saved Balanced profile after creating a Best fit conversation", async () => {
    const saved = createdConversation({ id: "saved", title: "Saved Balanced", routingProfile: "balanced" });
    const { store, dialog } = await openChooser({
      listConversations: vi.fn().mockResolvedValue({ items: [saved], nextCursor: null }),
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Without a folder" }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(store.getSnapshot().conversationsById.saved.routingProfile).toBe("balanced");
    act(() => store.selectConversation("saved"));
    expect(screen.getByRole("option", { name: "Auto · Balanced" })).toBeInTheDocument();
  });

  it("focuses the composer if the inspector becomes a narrow overlay during creation", async () => {
    const media = { matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() };
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue(media));
    const creation = deferred<ConversationSummary>();
    try {
      const { dialog } = await openChooser({ createConversation: vi.fn().mockReturnValue(creation.promise) });
      fireEvent.click(within(dialog).getByRole("button", { name: "Without a folder" }));
      act(() => {
        media.matches = true;
        media.addEventListener.mock.calls[0][1]();
      });
      await act(async () => { creation.resolve(createdConversation()); });
      expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
      expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus();
    } finally {
      vi.unstubAllGlobals();
    }
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

  it.each([false, true])("restores app interaction and provider focus after canceling interruption (preview: %s)", async (preview) => {
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
      if (preview) {
        fireEvent.change(screen.getByRole("textbox", { name: "Message" }), { target: { value: "**keep draft**" } });
        fireEvent.click(screen.getByRole("button", { name: "Preview Markdown" }));
        fireEvent.click(screen.getByRole("button", { name: "Interrupt Codex" }));
      } else {
        fireEvent.change(provider, { target: { value: "claude" } });
      }
      const dialog = screen.getByRole("dialog", { name: preview ? "Interrupt Codex" : "Interrupt Codex to switch provider" });

      expect(container.querySelector(".app-shell")).toHaveAttribute("inert");
      expect(container).not.toContainElement(dialog);
      fireEvent.click(within(dialog).getByRole("button", { name: "Keep Codex running" }));
      await waitFor(() => expect(container.querySelector(".app-shell")).not.toHaveAttribute("inert"));
      await waitFor(() => expect(provider).toHaveFocus());
      if (preview) expect(screen.getByRole("region", { name: "Message preview" })).toHaveTextContent("keep draft");
    } finally {
      focus.mockRestore();
    }
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

  it("retains the startup diagnostic and recovery action when services are unavailable", async () => {
    const api = createApi({
      getBootstrap: vi.fn().mockResolvedValueOnce({
        providers: [],
        startupDiagnostic: { code: "storage-error", message: "Cannot open database", action: "Check Application Support permissions and restart" },
      }).mockResolvedValue({ providers: [] }),
      listConversations: vi.fn()
        .mockRejectedValueOnce(new BridgeError("startup-unavailable", "Application services are unavailable.", null))
        .mockResolvedValue({ items: [], nextCursor: null }),
    });
    const store = createAppStore(api);
    render(<App store={store} />);
    expect(await screen.findByText("Application services are unavailable.")).toBeVisible();
    expect(screen.getByText("Cannot open database")).toBeVisible();
    expect(screen.getByText("Check Application Support permissions and restart")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No conversation selected")).toBeVisible();
    expect(screen.queryByText("Cannot open database")).not.toBeInTheDocument();
    store.dispose();
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
