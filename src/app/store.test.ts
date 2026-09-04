import { waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type {
  AgentSnapshot,
  AgentTreePage,
  AppEvent,
  ConversationPage,
  ConversationSummary,
  InspectorSnapshot,
} from "../bridge/types";
import { createAppStore, selectVisibleConversations, type AppApi } from "./store";

function agent(
  id: string,
  parentId: string | null,
  status: AgentSnapshot["status"] = "running",
): AgentSnapshot {
  return {
    id,
    parentId,
    provider: "codex",
    label: id,
    summary: null,
    status,
  };
}

function inspectorSnapshot(executionPath: string): InspectorSnapshot {
  return {
    workspace: { mode: "projectless", changes: [], truncated: false },
    executionPath,
    ownedWorktree: false,
    cleanup: { eligible: false, blocker: "notOwned" },
    currentRun: null,
    routing: null,
    handoff: null,
    activeDescendantCount: 0,
    agentsTruncated: false,
  };
}

function conversation(
  id: string,
  overrides: Partial<ConversationSummary> = {},
): ConversationSummary {
  return {
    id,
    title: `Conversation ${id}`,
    routingProfile: "balanced",
    workspaceId: null,
    archived: false,
    projectRoot: null,
    currentRunId: `run-${id}`,
    provider: "codex",
    runStatus: "running",
    rollupStatus: "active",
    agents: [agent(`root-${id}`, null)],
    agentsTruncated: false,
    ...overrides,
  };
}

function conversationActions(): Pick<
  AppApi,
  | "loadTimeline"
  | "loadEventDetail"
  | "loadApprovals"
  | "loadApprovalDetail"
  | "loadApprovalQuestions"
  | "submitMessage"
  | "steerRun"
  | "respondToApproval"
  | "interruptRun"
  | "inspectWorkspace"
  | "listRunAudits"
  | "loadRunAudit"
  | "createConversation"
  | "archiveConversation"
  | "inspectProject"
> {
  return {
    loadTimeline: vi.fn().mockResolvedValue({ items: [], nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null }),
    loadEventDetail: vi.fn(),
    loadApprovals: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadApprovalDetail: vi.fn(),
    loadApprovalQuestions: vi.fn(),
    submitMessage: vi.fn(),
    steerRun: vi.fn(),
    respondToApproval: vi.fn(),
    interruptRun: vi.fn(),
    inspectWorkspace: vi.fn(),
    listRunAudits: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadRunAudit: vi.fn(),
    createConversation: vi.fn(),
    archiveConversation: vi.fn(),
    inspectProject: vi.fn().mockResolvedValue({ isGit: true }),
  };
}

function createFakeApi() {
  let eventHandler: ((event: AppEvent) => void) | null = null;
  let firstTitle = "Project work";
  const calls = {
    listen: 0,
    conversations: 0,
    agentCursors: [] as Array<string | null>,
  };

  const api: AppApi = {
    getBootstrap: vi.fn().mockResolvedValue({
      providers: [
        {
          id: "codex",
          installed: true,
          available: true,
          version: "1.0.0",
          diagnostic: null,
          capabilities: ["streaming"],
        },
      ],
    }),
    listConversations: vi.fn(async ({ cursor }) => {
      calls.conversations += 1;
      if (cursor === null) {
        return {
          items: [
            conversation("c1", {
              title: firstTitle,
              projectRoot: "/work/alpha",
              agentsTruncated: true,
            }),
          ],
          nextCursor: "conversations-2",
        };
      }
      return {
        items: [
          conversation("c2", {
            title: "Queued idea",
            runStatus: "queued",
            rollupStatus: "active",
          }),
        ],
        nextCursor: null,
      };
    }),
    loadConversation: vi.fn(async ({ conversationId }) => {
      if (conversationId === "c1") {
        return conversation("c1", {
          title: firstTitle,
          projectRoot: "/work/alpha",
          agentsTruncated: true,
        });
      }
      return conversation("c2", {
        title: "Queued idea",
        runStatus: "queued",
        rollupStatus: "active",
      });
    }),
    loadAgentTree: vi.fn(async ({ conversationId, cursor }) => {
      calls.agentCursors.push(cursor);
      if (conversationId !== "c1") {
        return { runId: `run-${conversationId}`, items: [], nextCursor: null };
      }
      if (cursor === null) {
        return {
          runId: "run-c1",
          items: [
            { agent: agent("root-c1", null), depth: 0 },
            { agent: agent("reviewer", "root-c1"), depth: 1 },
          ],
          nextCursor: "agents-2",
        };
      }
      return {
        runId: "run-c1",
        items: [{ agent: agent("researcher", "reviewer", "waiting"), depth: 2 }],
        nextCursor: null,
      };
    }),
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
    inspectWorkspace: vi.fn(),
    listRunAudits: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadRunAudit: vi.fn(),
    createConversation: vi.fn(),
    archiveConversation: vi.fn(),
    inspectProject: vi.fn().mockResolvedValue({ isGit: true }),
    listenToAppEvents: vi.fn(async (handler) => {
      calls.listen += 1;
      eventHandler = handler;
      return () => {
        eventHandler = null;
      };
    }),
  };

  return {
    api,
    calls,
    emit(event: AppEvent) {
      eventHandler?.(event);
    },
    renameFirst(title: string) {
      firstTitle = title;
    },
  };
}

describe("app store", () => {
  it("adds and selects a newly created projectless conversation on an empty install", async () => {
    const fake = createFakeApi();
    fake.api.listConversations = vi.fn().mockResolvedValue({ items: [], nextCursor: null });
    fake.api.createConversation = vi.fn().mockResolvedValue(conversation("new", {
      currentRunId: null, provider: null, runStatus: null, rollupStatus: null, agents: [],
    }));
    const store = createAppStore(fake.api);
    await store.initialize();

    await store.createConversation({
      title: "Fresh work", objective: "Explore", constraints: [],
      workspace: { kind: "projectless" }, routingProfile: "balanced",
    });

    expect(fake.api.createConversation).toHaveBeenCalledWith(expect.objectContaining({
      workspace: { kind: "projectless" },
    }));
    expect(store.getSnapshot().selectedConversationId).toBe("new");
    expect(store.getSnapshot().conversationIds).toEqual(["new"]);
  });

  it("archives the selected conversation and selects the next active conversation", async () => {
    const fake = createFakeApi();
    fake.api.archiveConversation = vi.fn().mockResolvedValue(undefined);
    const store = createAppStore(fake.api);
    await store.initialize();

    await store.archiveConversation("c1");

    expect(fake.api.archiveConversation).toHaveBeenCalledWith({ conversationId: "c1" });
    expect(store.getSnapshot().conversationsById.c1).toBeUndefined();
    expect(store.getSnapshot().selectedConversationId).toBe("c2");
  });

  it("removes and reselects when a targeted refresh reports the selection archived", async () => {
    const fake = createFakeApi();
    fake.api.loadConversation = vi.fn().mockResolvedValue(conversation("c1", { archived: true }));
    const store = createAppStore(fake.api);
    await store.initialize();

    fake.emit({ kind: "conversationChanged", sequence: "1", conversationId: "c1" });

    await waitFor(() => expect(store.getSnapshot().conversationsById.c1).toBeUndefined());
    expect(store.getSnapshot().selectedConversationId).toBe("c2");
  });

  it("keeps truncated agent trees lazy during initial synchronization", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);

    await store.initialize();

    const snapshot = store.getSnapshot();
    expect(snapshot.conversationIds).toEqual(["c1", "c2"]);
    expect(snapshot.conversationsById.c1?.agentIds).toEqual(["root-c1"]);
    expect(snapshot.agentsById.researcher).toBeUndefined();
    expect(fake.calls.agentCursors).toEqual([]);
    expect(Object.isFrozen(snapshot)).toBe(true);
    expect(Object.isFrozen(snapshot.conversationIds)).toBe(true);
    expect(Object.isFrozen(snapshot.conversationsById.c1?.agentIds)).toBe(true);
  });

  it("loads one agent page only after explicit disclosure", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();

    await store.loadAgentPage("c1");

    expect(fake.calls.agentCursors).toEqual([null]);
    expect(store.getSnapshot().conversationsById.c1?.agentIds).toEqual([
      "root-c1",
      "reviewer",
    ]);
    expect(store.getSnapshot().agentWindow).toMatchObject({
      conversationId: "c1",
      runId: "run-c1",
      nextCursor: "agents-2",
      evicted: false,
    });

    await store.loadAgentPage("c1");

    expect(fake.calls.agentCursors).toEqual([null, "agents-2"]);
    expect(store.getSnapshot().conversationsById.c1?.agentIds).toEqual([
      "root-c1",
      "reviewer",
      "researcher",
    ]);
    expect(store.getSnapshot().conversationsById.c1?.agentsTruncated).toBe(false);
  });

  it("coalesces agent restarts behind one in-flight page and converges on latest", async () => {
    const fake = createFakeApi();
    const resolvers: Array<(value: AgentTreePage) => void> = [];
    let inFlight = 0;
    let maxInFlight = 0;
    fake.api.loadAgentTree = vi.fn(() => new Promise<AgentTreePage>((resolve) => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      resolvers.push((value) => { inFlight -= 1; resolve(value); });
    }));
    const store = createAppStore(fake.api);
    await store.initialize();
    const first = store.loadAgentPage("c1", true);
    void store.loadAgentPage("c1", true);
    void store.loadAgentPage("c1", true);
    expect(fake.api.loadAgentTree).toHaveBeenCalledTimes(1);
    resolvers.shift()?.({ runId: "run-c1", items: [{ agent: agent("stale", "root-c1"), depth: 1 }], nextCursor: null });
    await waitFor(() => expect(fake.api.loadAgentTree).toHaveBeenCalledTimes(2));
    resolvers.shift()?.({ runId: "run-c1", items: [{ agent: agent("latest", "root-c1"), depth: 1 }], nextCursor: null });
    await first;
    expect(maxInFlight).toBe(1);
    expect(store.getSnapshot().agentsById.latest).toBeDefined();
  });

  it("globally serializes workspace inspection and coalesces to the latest selection", async () => {
    const fake = createFakeApi();
    const requests: string[] = [];
    const resolvers: Array<(value: InspectorSnapshot) => void> = [];
    let inFlight = 0;
    let maxInFlight = 0;
    fake.api.inspectWorkspace = vi.fn(({ conversationId }) => new Promise<InspectorSnapshot>((resolve) => {
      requests.push(conversationId);
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      resolvers.push((value) => { inFlight -= 1; resolve(value); });
    }));
    const store = createAppStore(fake.api);
    const first = store.actions.inspectWorkspace({ conversationId: "c1" });
    const second = store.actions.inspectWorkspace({ conversationId: "c2" });
    const third = store.actions.inspectWorkspace({ conversationId: "c3" });
    expect(requests).toEqual(["c1"]);
    resolvers.shift()?.(inspectorSnapshot("A"));
    await waitFor(() => expect(requests).toEqual(["c1", "c3"]));
    resolvers.shift()?.(inspectorSnapshot("C"));
    await expect(first).resolves.toMatchObject({ executionPath: "A" });
    await expect(second).resolves.toMatchObject({ executionPath: "C" });
    await expect(third).resolves.toMatchObject({ executionPath: "C" });
    expect(maxInFlight).toBe(1);
  });

  it("retains a loaded selected path when the bounded agent window evicts older pages", async () => {
    const fake = createFakeApi();
    let page = 0;
    fake.api.loadAgentTree = vi.fn(async ({ cursor }) => {
      page += 1;
      if (cursor === null) {
        return {
          runId: "run-c1",
          items: [
            { agent: agent("root-c1", null), depth: 0 },
            { agent: agent("reviewer", "root-c1"), depth: 1 },
            { agent: agent("researcher", "reviewer"), depth: 2 },
          ],
          nextCursor: "agents-2",
        };
      }
      return {
        runId: "run-c1",
        items: [{ agent: agent(`later-${page}`, "root-c1"), depth: 1 }],
        nextCursor: `agents-${page + 1}`,
      };
    });
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadAgentPage("c1");
    store.selectConversation("c1", "researcher");

    for (let index = 0; index < 4; index += 1) await store.loadAgentPage("c1");

    const snapshot = store.getSnapshot();
    expect(snapshot.agentWindow?.evicted).toBe(true);
    expect(snapshot.selectedAgentId).toBe("researcher");
    expect(snapshot.conversationsById.c1?.agentIds).toEqual(expect.arrayContaining([
      "root-c1",
      "reviewer",
      "researcher",
    ]));
    expect(snapshot.conversationsById.c1?.agentIds.length).toBeLessThanOrEqual(8);
  });

  it("releases a pinned path after selection moves away from its evicted window", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadAgentPage("c1");
    store.selectConversation("c1", "reviewer");
    await store.loadAgentPage("c2");

    store.selectConversation("c2");

    expect(store.getSnapshot().conversationsById.c1?.agentIds).toEqual(["root-c1"]);
    expect(store.getSnapshot().agentsById.reviewer).toBeUndefined();
  });

  it("refreshes the first disclosed agent page across same-run targeted refreshes", async () => {
    const fake = createFakeApi();
    let reviewerStatus: AgentSnapshot["status"] = "running";
    vi.mocked(fake.api.loadAgentTree).mockImplementation(async ({ cursor }) => {
      fake.calls.agentCursors.push(cursor);
      return {
        runId: "run-c1",
        items: [
          { agent: agent("root-c1", null), depth: 0 },
          { agent: agent("reviewer", "root-c1", reviewerStatus), depth: 1 },
        ],
        nextCursor: "agents-2",
      };
    });
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadAgentPage("c1");
    store.selectConversation("c1", "reviewer");
    reviewerStatus = "completed";

    fake.emit({ kind: "conversationChanged", sequence: "1", conversationId: "c1" });

    await waitFor(() => expect(store.getSnapshot().agentsById.reviewer?.status).toBe("completed"));
    expect(store.getSnapshot().selectedAgentId).toBe("reviewer");
    expect(store.getSnapshot().conversationsById.c1?.agentIds).toContain("reviewer");
    expect(fake.calls.agentCursors).toEqual([null, null]);
  });

  it("refreshes the first disclosed agent page across a same-run full refresh", async () => {
    const fake = createFakeApi();
    let reviewerStatus: AgentSnapshot["status"] = "running";
    vi.mocked(fake.api.loadAgentTree).mockImplementation(async ({ cursor }) => {
      fake.calls.agentCursors.push(cursor);
      return {
        runId: "run-c1",
        items: [
          { agent: agent("root-c1", null), depth: 0 },
          { agent: agent("reviewer", "root-c1", reviewerStatus), depth: 1 },
        ],
        nextCursor: "agents-2",
      };
    });
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadAgentPage("c1");
    store.selectConversation("c1", "reviewer");
    const initialListCalls = fake.calls.conversations;
    reviewerStatus = "completed";

    fake.emit({ kind: "reloadRequired", sequence: "1" });

    await waitFor(() => expect(store.getSnapshot().agentsById.reviewer?.status).toBe("completed"));
    expect(fake.calls.conversations).toBeGreaterThan(initialListCalls);
    expect(store.getSnapshot().selectedAgentId).toBe("reviewer");
    expect(store.getSnapshot().conversationsById.c1?.agentIds).toContain("reviewer");
    expect(fake.calls.agentCursors).toEqual([null, null]);
  });

  it("retries an initially failed agent page without losing explicit disclosure", async () => {
    const fake = createFakeApi();
    vi.mocked(fake.api.loadAgentTree)
      .mockRejectedValueOnce(new Error("Agent service unavailable."))
      .mockResolvedValueOnce({
        runId: "run-c1",
        items: [
          { agent: agent("root-c1", null), depth: 0 },
          { agent: agent("reviewer", "root-c1"), depth: 1 },
        ],
        nextCursor: "agents-2",
      });
    const store = createAppStore(fake.api);
    await store.initialize();

    await store.loadAgentPage("c1");
    expect(store.getSnapshot().agentWindow).toMatchObject({
      pages: [],
      error: "Agent service unavailable.",
    });

    await store.loadAgentPage("c1", true);

    expect(fake.api.loadAgentTree).toHaveBeenCalledTimes(2);
    expect(store.getSnapshot().agentWindow).toMatchObject({
      pages: [["root-c1", "reviewer"]],
      error: null,
    });
    expect(store.getSnapshot().conversationsById.c1?.agentIds).toContain("reviewer");
  });

  it("drops a loaded agent window and selection when the current run changes", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadAgentPage("c1");
    store.selectConversation("c1", "reviewer");
    vi.mocked(fake.api.loadConversation).mockResolvedValue(conversation("c1", {
      currentRunId: "run-new",
      agents: [agent("root-new", null)],
      agentsTruncated: false,
    }));

    fake.emit({ kind: "conversationChanged", sequence: "1", conversationId: "c1" });

    await waitFor(() => expect(store.getSnapshot().conversationsById.c1?.currentRunId).toBe("run-new"));
    expect(store.getSnapshot().selectedAgentId).toBeNull();
    expect(store.getSnapshot().agentWindow).toBeNull();
    expect(store.getSnapshot().agentsById.reviewer).toBeUndefined();
  });

  it("subscribes once and replaces, rather than mutating, state when events arrive", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await Promise.all([store.initialize(), store.initialize()]);
    const before = store.getSnapshot();

    fake.emit({ kind: "conversationChanged", sequence: "1", conversationId: "c1" });

    const after = store.getSnapshot();
    expect(fake.calls.listen).toBe(1);
    expect(after).not.toBe(before);
    expect(after.lastSequence).toBe("1");
    expect(before.lastSequence).toBeNull();
  });

  it("reloads authoritative state after an event sequence gap", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();

    fake.emit({ kind: "conversationChanged", sequence: "1", conversationId: "c1" });
    await waitFor(() => expect(fake.api.loadConversation).toHaveBeenCalledTimes(1));
    fake.renameFirst("Reloaded after gap");
    fake.emit({ kind: "runChanged", sequence: "3", conversationId: "c1", runId: "run-c1" });

    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("Reloaded after gap");
    });
    expect(store.getSnapshot().lastSequence).toBe("3");
    expect(store.getSnapshot().conversationVersions.c2).toBe(1);
    expect(fake.calls.conversations).toBeGreaterThanOrEqual(4);
  });

  it("derives queued count, selection, and status-filtered conversations", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    const before = store.getSnapshot();

    store.selectConversation("c2");
    store.setStatusFilter("queued");

    const after = store.getSnapshot();
    expect(after.selectedConversationId).toBe("c2");
    expect(after.queuedCount).toBe(1);
    expect(selectVisibleConversations(after).map(({ id }) => id)).toEqual(["c2"]);
    expect(before.selectedConversationId).toBe("c1");
    expect(before.statusFilter).toBe("all");
  });

  it("releases a subscription that resolves after disposal", async () => {
    let resolveSubscription!: (unlisten: () => void) => void;
    const unlisten = vi.fn();
    const fake = createFakeApi();
    fake.api.listenToAppEvents = () => new Promise((resolve) => {
      resolveSubscription = resolve;
    });
    const store = createAppStore(fake.api);

    const initializing = store.initialize();
    store.dispose();
    resolveSubscription(unlisten);
    await initializing;

    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("does not claim a stale-run agent summary is complete", async () => {
    const fake = createFakeApi();
    fake.api.loadAgentTree = vi.fn().mockResolvedValue({
      runId: "newer-run",
      items: [],
      nextCursor: null,
    });
    const store = createAppStore(fake.api);

    await store.initialize();

    expect(store.getSnapshot().conversationsById.c1?.agentsTruncated).toBe(true);
    expect(store.getSnapshot().conversationsById.c1?.agentIds).toEqual(["root-c1"]);
  });

  it("accepts the explicit maximum-sequence rollover handshake and continues at one", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    const initialListCalls = fake.calls.conversations;

    fake.renameFirst("After maximum");
    fake.emit({ kind: "reloadRequired", sequence: "18446744073709551615" });
    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("After maximum");
    });
    fake.renameFirst("After rollover one");
    fake.emit({ kind: "conversationChanged", sequence: "1", conversationId: "c1" });
    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("After rollover one");
    });
    fake.renameFirst("After rollover two");
    fake.emit({ kind: "runChanged", sequence: "2", conversationId: "c1", runId: "run-c1" });

    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("After rollover two");
    });
    expect(store.getSnapshot().lastSequence).toBe("2");
    expect(fake.calls.conversations).toBe(initialListCalls + 2);
  });

  it("recovers once when the first post-rollover event is lost, then resumes targeted refreshes", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    fake.emit({ kind: "reloadRequired", sequence: "18446744073709551615" });
    await waitFor(() => expect(fake.calls.conversations).toBeGreaterThanOrEqual(4));
    const listCallsAfterMaximum = fake.calls.conversations;
    vi.mocked(fake.api.loadConversation).mockClear();

    fake.renameFirst("Recovered after lost one");
    fake.emit({ kind: "conversationChanged", sequence: "2", conversationId: "c1" });
    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("Recovered after lost one");
    });
    expect(fake.calls.conversations).toBe(listCallsAfterMaximum + 2);
    expect(fake.api.loadConversation).not.toHaveBeenCalled();

    fake.renameFirst("Targeted after recovery");
    fake.emit({ kind: "runChanged", sequence: "3", conversationId: "c1", runId: "run-c1" });
    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("Targeted after recovery");
    });
    expect(fake.calls.conversations).toBe(listCallsAfterMaximum + 2);
    expect(fake.api.loadConversation).toHaveBeenCalledTimes(1);
    expect(store.getSnapshot().lastSequence).toBe("3");
  });

  it("rejects an unrelated sequence regression without opening the rollover gate", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    fake.emit({ kind: "conversationChanged", sequence: "10", conversationId: "c1" });
    await waitFor(() => expect(fake.api.loadConversation).toHaveBeenCalledTimes(1));
    const titleBeforeRegression = store.getSnapshot().conversationsById.c1?.title;
    const listCalls = fake.calls.conversations;
    vi.mocked(fake.api.loadConversation).mockClear();

    fake.renameFirst("Must stay hidden");
    fake.emit({ kind: "conversationChanged", sequence: "9", conversationId: "c1" });
    await Promise.resolve();

    expect(store.getSnapshot().lastSequence).toBe("10");
    expect(store.getSnapshot().conversationsById.c1?.title).toBe(titleBeforeRegression);
    expect(fake.calls.conversations).toBe(listCalls);
    expect(fake.api.loadConversation).not.toHaveBeenCalled();
  });

  it("discards a live paginated scan changed between pages without losing selection", async () => {
    let handler!: (event: AppEvent) => void;
    let firstPageCalls = 0;
    let moved = false;
    const all = Array.from({ length: 201 }, (_, index) => conversation(`c${index}`));
    const api: AppApi = {
      ...conversationActions(),
      getBootstrap: vi.fn().mockResolvedValue({ providers: [] }),
      listConversations: vi.fn(async ({ cursor }) => {
        if (cursor === null) {
          firstPageCalls += 1;
          if (firstPageCalls === 2) {
            const page = { items: all.slice(0, 200), nextCursor: "older" };
            moved = true;
            handler({ kind: "conversationChanged", sequence: "2", conversationId: "c200" });
            return page;
          }
          const current = moved ? [all[200]!, ...all.slice(0, 200)] : all;
          return { items: current.slice(0, 200), nextCursor: "older" };
        }
        return {
          items: moved ? [all[199]!] : [all[200]!],
          nextCursor: null,
        };
      }),
      loadConversation: vi.fn(async () => all[200]!),
      loadAgentTree: vi.fn().mockResolvedValue({ runId: null, items: [], nextCursor: null }),
      listenToAppEvents: vi.fn(async (nextHandler) => {
        handler = nextHandler;
        return () => {};
      }),
    };
    const store = createAppStore(api);
    await store.initialize();
    store.selectConversation("c200");
    let publishedIncomplete = false;
    store.subscribe(() => {
      const snapshot = store.getSnapshot();
      if (snapshot.phase === "ready" && !snapshot.conversationIds.includes("c200")) {
        publishedIncomplete = true;
      }
    });

    handler({ kind: "reloadRequired", sequence: "1" });

    await waitFor(() => expect(firstPageCalls).toBeGreaterThanOrEqual(3));
    await waitFor(() => expect(store.getSnapshot().conversationIds).toHaveLength(201));
    expect(store.getSnapshot().selectedConversationId).toBe("c200");
    expect(publishedIncomplete).toBe(false);
  });

  it("coalesces streaming invalidations into a targeted refresh", async () => {
    let handler!: (event: AppEvent) => void;
    const relevant = conversation("c1", { agentsTruncated: true });
    const unrelated = conversation("c2", { agentsTruncated: true });
    const archived = conversation("c3", { archived: true, agentsTruncated: true });
    const loadAgentTree = vi.fn(async ({ conversationId }: { conversationId: string }) => ({
      runId: `run-${conversationId}`,
      items: [{ agent: agent(`root-${conversationId}`, null), depth: 0 }],
      nextCursor: null,
    }));
    const listConversations = vi.fn().mockResolvedValue({
      items: [relevant, unrelated, archived],
      nextCursor: null,
    });
    const loadConversation = vi.fn().mockResolvedValue({ ...relevant, title: "Streaming" });
    const api: AppApi = {
      ...conversationActions(),
      getBootstrap: vi.fn().mockResolvedValue({ providers: [] }),
      listConversations,
      loadConversation,
      loadAgentTree,
      listenToAppEvents: vi.fn(async (nextHandler) => {
        handler = nextHandler;
        return () => {};
      }),
    };
    const store = createAppStore(api);
    await store.initialize();
    expect(loadAgentTree.mock.calls.map(([request]) => request.conversationId)).not.toContain("c3");
    listConversations.mockClear();
    loadConversation.mockClear();
    loadAgentTree.mockClear();

    for (let sequence = 1; sequence <= 50; sequence += 1) {
      handler({
        kind: "runChanged",
        sequence: sequence.toString(),
        conversationId: "c1",
        runId: "run-c1",
      });
    }

    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("Streaming");
    });
    expect(listConversations).not.toHaveBeenCalled();
    expect(loadConversation).toHaveBeenCalledTimes(1);
    expect(loadAgentTree).not.toHaveBeenCalled();
    expect(store.getSnapshot().conversationVersions.c1).toBe(1);
  });

  it("does not let targeted work from an aborted scan overwrite its authoritative retry", async () => {
    let handler!: (event: AppEvent) => void;
    let resolveAbortedScan!: (page: ConversationPage) => void;
    let resolveStaleTarget!: (value: ConversationSummary) => void;
    let listCalls = 0;
    let targetCalls = 0;
    const initial = conversation("c1", { title: "Initial" });
    const authoritative = conversation("c1", { title: "Authoritative retry" });
    const stale = conversation("c1", { title: "Stale targeted result" });
    const future = conversation("c1", { title: "Future targeted result" });
    const api: AppApi = {
      ...conversationActions(),
      getBootstrap: vi.fn().mockResolvedValue({ providers: [] }),
      listConversations: vi.fn(async (): Promise<ConversationPage> => {
        listCalls += 1;
        if (listCalls === 1) return { items: [initial], nextCursor: null };
        if (listCalls === 2) {
          return new Promise((resolve) => { resolveAbortedScan = resolve; });
        }
        return { items: [authoritative], nextCursor: null };
      }),
      loadConversation: vi.fn(async (): Promise<ConversationSummary> => {
        targetCalls += 1;
        if (targetCalls === 1) {
          return new Promise((resolve) => { resolveStaleTarget = resolve; });
        }
        return future;
      }),
      loadAgentTree: vi.fn().mockResolvedValue({ runId: null, items: [], nextCursor: null }),
      listenToAppEvents: vi.fn(async (nextHandler) => {
        handler = nextHandler;
        return () => {};
      }),
    };
    const store = createAppStore(api);
    await store.initialize();

    handler({ kind: "reloadRequired", sequence: "1" });
    await waitFor(() => expect(listCalls).toBe(2));
    handler({ kind: "conversationChanged", sequence: "2", conversationId: "c1" });
    await waitFor(() => expect(targetCalls).toBe(1));
    resolveAbortedScan({
      items: [conversation("c1", { title: "Aborted scan" })],
      nextCursor: null,
    });
    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("Authoritative retry");
    });

    resolveStaleTarget(stale);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(store.getSnapshot().conversationsById.c1?.title).toBe("Authoritative retry");

    handler({ kind: "runChanged", sequence: "3", conversationId: "c1", runId: "run-c1" });
    await waitFor(() => {
      expect(store.getSnapshot().conversationsById.c1?.title).toBe("Future targeted result");
    });
    expect(store.getSnapshot().lastSequence).toBe("3");
  });
});
