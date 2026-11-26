import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BridgeError } from "../bridge/api";
import type { ConversationSummary, ListChildConversationsRequest, LoadConversationRequest, TimelineItem } from "../bridge/types";
import { axeViolations, render, settle } from "../test/render";
import { App } from "./App";
import { createAppStore, type AppApi, type AppStore } from "./store";

const click = (element: Element) => {
  fireEvent.click(element);
  flush();
};
const press = (key: string, init: KeyboardEventInit = {}) => {
  fireEvent.keyDown(window, { key, metaKey: true, ...init });
  flush();
};
const narrowQuery = (query: string) => query === "(max-width: 72rem)";
let stores: AppStore[] = [];

function mediaQueries(matches: (query: string) => boolean = () => false) {
  vi.stubGlobal("matchMedia", vi.fn((query: string) => ({
    matches: matches(query), media: query, onchange: null,
    addListener: vi.fn(), removeListener: vi.fn(), addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: vi.fn(),
  })));
}

/** Renders the app over a store built from `api`; the store is disposed after the test. */
function renderApp(api: AppApi) {
  const store = createAppStore(api);
  stores.push(store);
  const view = render(() => <App store={store} />);
  return { store, ...view };
}

beforeEach(() => {
  mediaQueries();
  Object.defineProperty(Element.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
});
afterEach(() => {
  stores.forEach((store) => store.dispose());
  stores = [];
  vi.unstubAllGlobals();
  Reflect.deleteProperty(Element.prototype, "scrollIntoView");
});

describe("conversation hierarchy", () => {
  function hierarchyApi() {
    const root = createdConversation({ id: "compiler", title: "Compiler", hasChildren: true,
      currentRunId: "run-compiler", provider: "codex", runStatus: "completed", rollupStatus: "completed" });
    const unavailableReason = "This provider exposes recorded child activity, not an independently controllable chat.";
    const child = createdConversation({ ...root, id: "reviewer", parentId: root.id, title: "Parser reviewer",
      summary: "Checked parser ownership", capabilities: { canSend: false, canInterrupt: false,
        canArchive: false, canRoute: false, unavailableReason } });
    const grandchild = createdConversation({ ...child, id: "researcher", parentId: child.id,
      title: "Schema researcher", hasChildren: false });
    const nodes = [root, child, grandchild];
    const captured = new Map(nodes.map(node => [node.id, `Captured ${node.id} activity`]));
    const api = {
      ...createApi({
        getBootstrap: vi.fn().mockResolvedValue({ providers: [
          { id: "codex", installed: true, available: true, version: "1", diagnostic: null, capabilities: ["steering", "interruption"] },
          { id: "claude", installed: true, available: true, version: "2", diagnostic: null, capabilities: ["interruption"] },
        ] }),
        listConversations: vi.fn().mockResolvedValue({ items: [root], nextCursor: null }),
        loadConversation: vi.fn(async ({ conversationId }: LoadConversationRequest) => {
          const node = nodes.find(item => item.id === conversationId);
          if (!node) throw new Error("Conversation unavailable");
          return node;
        }),
        loadTimeline: vi.fn(async ({ conversationId }) => {
          const content = captured.get(conversationId)!;
          const item: TimelineItem = { id: `event-${conversationId}`, conversationId,
            runId: root.currentRunId!, agentId: `execution-${conversationId}`, sequence: "1",
            kind: "message", presentation: "normal", role: "assistant", content,
            contentBytes: String(content.length), truncated: false, provider: "codex", operation: null };
          return { items: [item], nextCursor: null, approvals: [], approvalsTruncated: false, activeCompaction: null, approvalsNextCursor: null };
        }),
      }),
      listChildConversations: vi.fn(async ({ parentId }: ListChildConversationsRequest) => ({
        items: nodes.filter(node => node.parentId === parentId), nextCursor: null,
      })),
      loadConversationPath: vi.fn(async ({ conversationId }: LoadConversationRequest) => ({
        items: nodes.slice(0, nodes.findIndex(node => node.id === conversationId) + 1),
        truncated: false, ownerConversationId: root.id,
      })),
    };
    return { api, root, child, grandchild, unavailableReason };
  }

  it("opens root, child, and grandchild through the same conversation timeline", async () => {
    const { api } = hierarchyApi();
    const { store } = renderApp(api);
    await screen.findByText("Captured compiler activity");
    click(await screen.findByRole("treeitem", { name: /Parser reviewer/ }));
    await screen.findByText("Captured reviewer activity");
    expect(store.getSnapshot().selectedConversationId).toBe("reviewer");
    expect(screen.queryByText("Captured compiler activity")).not.toBeInTheDocument();
    expect(screen.queryByRole("treeitem", { name: /Schema researcher/ })).not.toBeInTheDocument();
    click(screen.getByRole("button", { name: "Expand Parser reviewer" }));
    click(await screen.findByRole("treeitem", { name: /Schema researcher/ }));
    await screen.findByText("Captured researcher activity");
    expect(screen.queryByText("Captured reviewer activity")).not.toBeInTheDocument();
    expect(vi.mocked(api.loadTimeline).mock.calls.map(([request]) => request.conversationId)).toEqual(["compiler", "reviewer", "researcher"]);
  });

  it("goes back up through the ancestry and focuses the new timeline", async () => {
    const { api } = hierarchyApi();
    const { store } = renderApp(api);
    await screen.findByText("Captured compiler activity");
    await store.selectConversation("researcher");
    await settle();
    const ancestry = await screen.findByRole("navigation", { name: "Conversation ancestry" });
    click(within(ancestry).getByRole("button", { name: "Parser reviewer" }));
    await screen.findByText("Captured reviewer activity");
    await waitFor(() => expect(screen.getByRole("heading", { name: "Timeline" })).toHaveFocus());
  });

  it("keeps the current conversation and explains a failed path lookup", async () => {
    const { api } = hierarchyApi();
    api.loadConversationPath.mockRejectedValue(new Error("Conversation no longer available"));
    const { store } = renderApp(api);
    await screen.findByText("Captured compiler activity");
    await store.selectConversation("missing-child");
    await settle();
    expect(screen.getByRole("alert")).toHaveTextContent("Conversation no longer available");
    expect(screen.getByText("Captured compiler activity")).toBeVisible();
  });
});

describe("All work and ⌘J", () => {
  const waitingApi = () => {
    const first = createdConversation({ id: "first", title: "Evaluation rows", provider: "codex", runStatus: "waiting", rollupStatus: "needsAttention", currentRunId: "run-first" });
    const working = createdConversation({ id: "working", title: "Parser cleanup", provider: "claude", runStatus: "running", rollupStatus: "active", currentRunId: "run-working" });
    const second = createdConversation({ id: "second", title: "Tag names", projectRoot: "/tmp/other", provider: "codex", runStatus: "running", rollupStatus: "needsAttention", currentRunId: "run-second" });
    return createApi({ listConversations: vi.fn().mockResolvedValue({ items: [working, first, second], nextCursor: null }) });
  };

  it("lists roots waiting on you first under Needs you, then by project", async () => {
    renderApp(waitingApi());
    const needsYou = await screen.findByRole("tree", { name: "Needs you" });
    expect(within(needsYou).getAllByRole("treeitem").map((row) => row.getAttribute("aria-label"))).toEqual([
      "Evaluation rows, Codex, waiting for you", "Tag names, Codex, waiting for you",
    ]);
    expect(screen.getAllByRole("tree").map((tree) => tree.getAttribute("aria-labelledby") && document.getElementById(tree.getAttribute("aria-labelledby")!)?.textContent))
      .toEqual(["Needs you", "synthetic-project"]);
  });

  it("jumps to the next root waiting on you with ⌘J or Ctrl+J, wrapping around", async () => {
    const { store } = renderApp(waitingApi());
    await screen.findByRole("tree", { name: "Needs you" });
    expect(store.getSnapshot().selectedConversationId).toBe("working");
    press("j");
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("first"));
    press("j", { metaKey: false, ctrlKey: true });
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("second"));
    press("j");
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("first"));
    press("j", { repeat: true });
    press("j", { isComposing: true });
    await settle();
    expect(store.getSnapshot().selectedConversationId).toBe("first");
  });

  it("does nothing on ⌘J when nothing waits on you", async () => {
    const { store } = renderApp(createApi());
    await screen.findByRole("treeitem", { name: /Auth refactor/ });
    press("j");
    await settle();
    expect(store.getSnapshot().selectedConversationId).toBe("c1");
  });
});

describe("command palette", () => {
  async function openWorkspace(items = [
    createdConversation({ id: "a", title: "Alpha", projectRoot: "/projects/compiler" }),
    createdConversation({ id: "b", title: "Beta", projectRoot: "/projects/renderer" }),
  ]) {
    const api = createApi({ listConversations: vi.fn().mockResolvedValue({ items, nextCursor: null }) });
    const view = renderApp(api);
    await screen.findByRole("treeitem", { name: /Alpha/ });
    return view;
  }
  const combobox = () => screen.findByRole("combobox", { name: "Search conversations and commands" });

  it("opens with ⌘K, searches conversations by project path, and opens the chosen one", async () => {
    const { store } = await openWorkspace();
    press("k");
    const input = await combobox();
    expect(screen.getByRole("option", { name: /Alpha.*Current/ })).toBeVisible();
    fireEvent.input(input, { target: { value: "renderer" } });
    flush();
    expect(screen.queryByRole("option", { name: /Alpha/ })).not.toBeInTheDocument();
    fireEvent.keyDown(input, { key: "Enter" });
    flush();
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("b"));
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("toggles closed with Ctrl+K and ignores composition and held keys", async () => {
    await openWorkspace();
    press("k", { isComposing: true });
    press("k", { repeat: true });
    await settle();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    press("k", { metaKey: false, ctrlKey: true });
    await combobox();
    press("k", { metaKey: false, ctrlKey: true });
    await waitFor(() => expect(screen.queryByRole("combobox")).not.toBeInTheDocument());
  });

  it("runs layout commands after closing and names disabled ones", async () => {
    await openWorkspace();
    click(screen.getByRole("button", { name: "Search (⌘K)" }));
    const input = await combobox();
    expect(screen.getByRole("option", { name: /Go to parent conversation/ })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("option", { name: /Next item waiting on you/ })).toHaveAttribute("aria-disabled", "true");
    fireEvent.input(input, { target: { value: "show inspector" } });
    flush();
    fireEvent.keyDown(input, { key: "Enter" });
    flush();
    expect(await screen.findByRole("complementary", { name: "Inspector" })).toBeVisible();
  });

  it("does not open while the workspace is loading", async () => {
    let finish!: (value: { items: ConversationSummary[]; nextCursor: null }) => void;
    renderApp(createApi({ listConversations: vi.fn(() => new Promise<{ items: ConversationSummary[]; nextCursor: null }>((resolve) => { finish = resolve; })) }));
    press("k");
    await settle();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    finish({ items: [createdConversation({ id: "a", title: "Alpha" })], nextCursor: null });
    await screen.findByRole("treeitem", { name: /Alpha/ });
    press("k");
    expect(await combobox()).toBeVisible();
  });
});

describe("App", () => {
  it("renders the shell with All work, the queue, and provider details in the inspector", async () => {
    renderApp(createApi());
    expect(await screen.findByRole("treeitem", { name: /Auth refactor/ })).toBeVisible();
    expect(screen.getByRole("heading", { level: 1, name: "Auth refactor" })).toBeVisible();
    expect(within(screen.getByRole("banner")).getByText(/Codex · queued/)).toBeVisible();
    click(screen.getByRole("button", { name: "Show inspector" }));
    expect(screen.getByRole("complementary", { name: "All work" })).toBeVisible();
    expect(screen.getByRole("main", { name: "Conversation workspace" })).toBeVisible();
    expect(screen.getByRole("complementary", { name: "Inspector" })).toBeVisible();
    expect(screen.getByText("1 queued")).toBeVisible();
    expect(await screen.findByText("Codex 0.144.1")).toBeVisible();
    expect(screen.getByText(/Claude unavailable/)).toBeVisible();
  });

  it("reads its store once, even when the prop is written as a call", async () => {
    const api = createApi();
    render(() => <App store={createAppStore(api)} />);
    expect(await screen.findByRole("treeitem", { name: /Auth refactor/ })).toBeVisible();
    expect(api.listConversations).toHaveBeenCalledOnce();
  });

  it("only reads: no composer, approvals buttons, or conversation lifecycle controls", async () => {
    renderApp(createApi());
    await screen.findByRole("heading", { name: "Timeline" });
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    for (const name of [/New conversation/, /Archive/, /Send/, /Interrupt/, /Review/]) expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
  });

  it("filters by status and hides and shows All work", async () => {
    renderApp(createApi());
    await screen.findByRole("treeitem", { name: /Auth refactor/ });
    const filter = screen.getByRole("button", { name: "Filter conversations: All statuses" });
    filter.focus();
    click(filter);
    const menu = await screen.findByRole("menu", { name: "Filter conversations: All statuses" });
    expect(screen.getByRole("menuitem", { name: "All statuses (shown)" })).toBeVisible();
    for (let step = 0; step < 6; step += 1) fireEvent.keyDown(menu, { key: "ArrowDown" });
    await waitFor(() => expect(screen.getByRole("menuitem", { name: "Completed" })).toHaveAttribute("data-highlighted"));
    fireEvent.keyDown(menu, { key: "Enter" });
    await waitFor(() => expect(screen.queryByRole("treeitem", { name: /Auth refactor/ })).not.toBeInTheDocument());
    expect(screen.getByText("No conversations match this status.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Filter conversations: Completed" })).toBeVisible();
    click(screen.getByRole("button", { name: "Hide All work" }));
    expect(screen.getByRole("button", { name: "Show All work" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("complementary", { name: "All work" })).not.toBeInTheDocument();
    click(screen.getByRole("button", { name: "Show All work" }));
    expect(screen.getByRole("button", { name: "Filter conversations: Completed" })).toBeVisible();
  });

  it("starts with the inspector closed and keeps explicit toggles across conversation changes", async () => {
    const api = createApi();
    const other = createdConversation({ id: "c2", title: "Other conversation" });
    const initial = await api.listConversations({ cursor: null, limit: 40 });
    api.listConversations = vi.fn().mockResolvedValue({ items: [...initial.items, other], nextCursor: null });
    const { store } = renderApp(api);
    await screen.findByRole("treeitem", { name: /Auth refactor/ });
    expect(screen.getByRole("button", { name: "Show inspector" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
    expect(api.loadDiagnostics).not.toHaveBeenCalled();
    await store.selectConversation("c2");
    await settle();
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
    click(screen.getByRole("button", { name: "Show inspector" }));
    expect(screen.getByRole("complementary", { name: "Inspector" })).toBeVisible();
    await store.selectConversation("c1");
    await settle();
    expect(screen.getByRole("button", { name: "Hide inspector" })).toHaveAttribute("aria-expanded", "true");
    click(screen.getByRole("button", { name: "Hide inspector" }));
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
  });

  it("shows a recoverable synchronization error", async () => {
    renderApp(createApi({ listConversations: vi.fn().mockRejectedValueOnce(new Error("Bridge disconnected")).mockResolvedValue({ items: [], nextCursor: null }) }));
    expect(await screen.findByText("Bridge disconnected")).toBeVisible();
    click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No conversation selected")).toBeVisible();
  });

  it("keeps the startup diagnostic and recovery action when services are unavailable", async () => {
    renderApp(createApi({
      getBootstrap: vi.fn().mockResolvedValueOnce({
        providers: [],
        startupDiagnostic: { code: "storage-error", message: "Cannot open database", action: "Check Application Support permissions and restart" },
      }).mockResolvedValue({ providers: [] }),
      listConversations: vi.fn()
        .mockRejectedValueOnce(new BridgeError("startup-unavailable", "Application services are unavailable.", null))
        .mockResolvedValue({ items: [], nextCursor: null }),
    }));
    expect(await screen.findByText("Application services are unavailable.")).toBeVisible();
    expect(screen.getByText("Cannot open database")).toBeVisible();
    expect(screen.getByText("Check Application Support permissions and restart")).toBeVisible();
    click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No conversation selected")).toBeVisible();
    expect(screen.queryByText("Cannot open database")).not.toBeInTheDocument();
  });

  it("has no detectable axe violations with every pane open", async () => {
    renderApp(createApi());
    await screen.findByRole("heading", { name: "Timeline" });
    click(screen.getByRole("button", { name: "Show inspector" }));
    await screen.findByText("Codex 0.144.1");
    expect(await axeViolations({ page: true })).toEqual([]);
  });

  it("gives focus back to the inspector trigger when the inspector closes", async () => {
    renderApp(createApi());
    await screen.findByRole("heading", { name: "Timeline" });
    click(screen.getByRole("button", { name: "Show inspector" }));
    click(screen.getByRole("button", { name: "Close inspector" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Show inspector" })).toHaveFocus());
  });

  it("moves focus into the narrow inspector overlay, keeps Tab inside, and restores the trigger", async () => {
    mediaQueries(narrowQuery);
    renderApp(createApi());
    await screen.findByRole("heading", { name: "Timeline" });
    const trigger = screen.getByRole("button", { name: "Show inspector" });
    click(trigger);
    const close = await screen.findByRole("button", { name: "Close inspector" });
    await waitFor(() => expect(close).toHaveFocus());
    expect(screen.getByRole("main", { name: "Conversation workspace" })).toHaveAttribute("inert");
    press("k");
    await settle();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("complementary", { name: "Inspector" }), { key: "Tab", shiftKey: true });
    expect(document.activeElement).not.toBe(trigger);
    fireEvent.keyDown(screen.getByRole("complementary", { name: "Inspector" }), { key: "Escape" });
    flush();
    await waitFor(() => expect(screen.getByRole("button", { name: "Show inspector" })).toHaveFocus());
    expect(screen.getByRole("main", { name: "Conversation workspace" })).not.toHaveAttribute("inert");
  });
});

function createdConversation(overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return {
    contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null, thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
    id: "new-1", title: "synthetic-project", routingProfile: "bestFit", workspaceId: "w-1",
    parentId: null, hasChildren: false, summary: null,
    capabilities: { canSend: true, canInterrupt: true, canArchive: true, canRoute: true, unavailableReason: null },
    archived: false, projectRoot: "/tmp/synthetic-project", currentRunId: null, provider: null,
    runStatus: null, rollupStatus: null,
    ...overrides,
  };
}


function createApi(overrides: Partial<AppApi> = {}): AppApi {
  const root = createdConversation({ id: "c1", title: "Auth refactor", projectRoot: null, workspaceId: null,
    hasChildren: true, currentRunId: "run-1", provider: "codex", runStatus: "queued", rollupStatus: "active" });
  const child = createdConversation({ ...root, id: "child-1", title: "Reviewer", parentId: "c1",
    hasChildren: false, summary: "Reviewing", provider: "claude", runStatus: "running",
    capabilities: { canSend: false, canInterrupt: false, canArchive: false, canRoute: false,
      unavailableReason: "Recorded child activity is read-only." } });
  return {
    setContextBudget: vi.fn(), setThinkingPreference: vi.fn(),
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
    listConversations: vi.fn().mockResolvedValue({ items: [root], nextCursor: null }),
    loadConversation: vi.fn(async ({ conversationId }) => conversationId === child.id ? child : root),
    listChildConversations: vi.fn(async ({ parentId }) => ({ items: parentId === root.id ? [child] : [], nextCursor: null })),
    loadConversationPath: vi.fn(async ({ conversationId }) => ({
      items: conversationId === child.id ? [root, child] : [root], truncated: false, ownerConversationId: root.id,
    })),
    loadTimeline: vi.fn().mockResolvedValue({
      items: [], nextCursor: null, approvals: [], approvalsTruncated: false, activeCompaction: null, approvalsNextCursor: null,
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
