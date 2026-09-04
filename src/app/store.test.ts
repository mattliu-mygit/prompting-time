import { waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type {
  AgentSnapshot,
  AppEvent,
  ConversationPage,
  ConversationSummary,
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

function conversation(
  id: string,
  overrides: Partial<ConversationSummary> = {},
): ConversationSummary {
  return {
    id,
    title: `Conversation ${id}`,
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
  it("loads every conversation and every paged descendant into immutable normalized snapshots", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);

    await store.initialize();

    const snapshot = store.getSnapshot();
    expect(snapshot.conversationIds).toEqual(["c1", "c2"]);
    expect(snapshot.conversationsById.c1?.agentIds).toEqual([
      "root-c1",
      "reviewer",
      "researcher",
    ]);
    expect(snapshot.agentsById.researcher?.parentId).toBe("reviewer");
    expect(fake.calls.agentCursors).toEqual([null, "agents-2"]);
    expect(Object.isFrozen(snapshot)).toBe(true);
    expect(Object.isFrozen(snapshot.conversationIds)).toBe(true);
    expect(Object.isFrozen(snapshot.conversationsById.c1?.agentIds)).toBe(true);
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
    expect(loadAgentTree.mock.calls.map(([request]) => request.conversationId)).toEqual(["c1"]);
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
