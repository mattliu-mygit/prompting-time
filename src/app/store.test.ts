import { waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type {
  BootstrapSnapshot,
  ConversationPath,
  AppEvent,
  ConversationPage,
  ConversationSummary,
  InspectorSnapshot,
} from "../bridge/types";
import { createAppStore, selectVisibleConversations, type AppApi } from "./store";

describe("in-session drafts", () => {
  it("retains drafts through refresh and archive, removes empty entries, and drops them on disposal", async () => {
    const { api } = createFakeApi();
    const store = createAppStore(api);
    await store.initialize();
    store.setDraft("c1", "    **raw**\n");
    store.setDraft("c2", "second");
    await store.retry();
    expect(store.getSnapshot().draftsById.c1?.text).toBe("    **raw**\n");
    await store.archiveConversation("c1");
    expect(store.getSnapshot().draftsById.c1?.text).toBe("    **raw**\n");
    store.setDraft("c2", "");
    expect(store.getSnapshot().draftsById).not.toHaveProperty("c2");
    store.dispose();
    expect(store.getSnapshot().draftsById).toEqual({});
  });

  it.each([null, "run-c1"])("preserves a newer draft when an older request completes (run %s)", async (runId) => {
    const { api } = createFakeApi();
    let resolve!: () => void;
    const pending = new Promise<void>((finish) => { resolve = finish; });
    api.submitMessage = vi.fn<AppApi["submitMessage"]>(async () => {
      await pending;
      return { runId: "run", status: "queued", provider: "codex", duplicate: false, routingExplanation: "test" };
    });
    api.steerRun = vi.fn(() => pending);
    const store = createAppStore(api);
    await store.initialize();
    store.setDraft("c1", "original");
    const sending = store.submitDraft("c1", null, runId);
    store.setDraft("c1", "newer");
    store.setDraft("c1", "original");
    resolve();
    expect(await sending).toBe(true);
    expect(store.getSnapshot().draftsById.c1?.text).toBe("original");
    expect(store.getSnapshot().submissionsById).toEqual({});
  });

  it("does not restore transient state when a request fails after disposal", async () => {
    const { api } = createFakeApi();
    let reject!: (reason: Error) => void;
    api.steerRun = vi.fn(() => new Promise<void>((_resolve, fail) => { reject = fail; }));
    const store = createAppStore(api);
    await store.initialize();
    store.setDraft("c1", "direction");
    const sending = store.submitDraft("c1", null, "run-c1");
    store.dispose();
    reject(new Error("Disconnected"));
    expect(await sending).toBe(false);
    expect(store.getSnapshot().draftsById).toEqual({});
    expect(store.getSnapshot().submissionsById).toEqual({});
  });
});

function child(id: string, parentId: string, status: ConversationSummary["runStatus"] = "running"): ConversationSummary {
  return conversation(id, { parentId, currentRunId: "run-c1", runStatus: status,
    rollupStatus: status === "waiting" ? "needsAttention" : status === "completed" ? "completed" : "active",
    capabilities: { canSend: false, canInterrupt: false, canArchive: false, canRoute: false,
      unavailableReason: "Recorded child activity is read-only." } });
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
  };
}

function conversation(
  id: string,
  overrides: Partial<ConversationSummary> = {},
): ConversationSummary {
  return {
    id,
    title: `Conversation ${id}`,
    parentId: null, hasChildren: false, summary: null,
    capabilities: { canSend: true, canInterrupt: true, canArchive: true, canRoute: true, unavailableReason: null },
    routingProfile: "balanced",
    workspaceId: null,
    archived: false,
    projectRoot: null,
    currentRunId: `run-${id}`,
    provider: "codex",
    runStatus: "running",
    rollupStatus: "active",
    ...overrides,
  };
}

function conversationActions(): Pick<
  AppApi,
  | "loadTimeline"
  | "loadDiagnostics"
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
  | "pickProjectDirectory"
> {
  return {
    loadDiagnostics: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
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
    pickProjectDirectory: vi.fn().mockResolvedValue(null),
  };
}

function createFakeApi() {
  let eventHandler: ((event: AppEvent) => void) | null = null;
  let firstTitle = "Project work";
  const calls = {
    listen: 0,
    conversations: 0,
    childCursors: [] as Array<string | null>,
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
        });
      }
      if (conversationId === "researcher") return child("researcher", "reviewer", "waiting");
      if (conversationId === "reviewer" || conversationId === "sibling") return child(conversationId, "c1");
      return conversation("c2", {
        title: "Queued idea",
        runStatus: "queued",
        rollupStatus: "active",
      });
    }),
    listChildConversations: vi.fn(async ({ parentId, cursor }) => {
      calls.childCursors.push(cursor);
      if (parentId === "reviewer") return { items: [child("researcher", "reviewer", "waiting")], nextCursor: null };
      if (parentId !== "c1") return { items: [], nextCursor: null };
      return cursor === null
        ? { items: [{ ...child("reviewer", "c1"), hasChildren: true }], nextCursor: "children-2" }
        : { items: [child("sibling", "c1")], nextCursor: null };
    }),
    loadConversationPath: vi.fn(async ({ conversationId }) => ({
      items: [conversation("c1"), child(conversationId, "c1")],
      truncated: false, ownerConversationId: "c1",
    })),
    loadDiagnostics: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
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
    pickProjectDirectory: vi.fn().mockResolvedValue(null),
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
  it("delegates folder selection including cancellation and failure", async () => {
    const fake = createFakeApi();
    const failure = new Error("Picker failed");
    fake.api.pickProjectDirectory = vi.fn()
      .mockResolvedValueOnce("/tmp/synthetic-project")
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(failure);
    const store = createAppStore(fake.api);
    await expect(store.pickProjectDirectory()).resolves.toBe("/tmp/synthetic-project");
    await expect(store.pickProjectDirectory()).resolves.toBeNull();
    await expect(store.pickProjectDirectory()).rejects.toBe(failure);
    expect(fake.api.pickProjectDirectory).toHaveBeenCalledTimes(3);
    expect(fake.api.inspectProject).not.toHaveBeenCalled();
    expect(fake.api.createConversation).not.toHaveBeenCalled();
  });

  it("retains a late bootstrap diagnostic after a concurrent conversation failure", async () => {
    const fake = createFakeApi();
    let finishBootstrap!: (value: BootstrapSnapshot) => void;
    fake.api.getBootstrap = vi.fn(() => new Promise<BootstrapSnapshot>((resolve) => { finishBootstrap = resolve; }));
    fake.api.listConversations = vi.fn().mockRejectedValue(new Error("Services unavailable"));
    const store = createAppStore(fake.api);
    await store.initialize();
    expect(fake.api.getBootstrap).toHaveBeenCalledOnce();
    expect(fake.api.listConversations).toHaveBeenCalledOnce();
    expect(store.getSnapshot().phase).toBe("error");
    const bootstrap = { providers: [], startupDiagnostic: { code: "storage-error", message: "Cannot open database", action: "Check permissions" } };
    finishBootstrap(bootstrap);
    await waitFor(() => expect(store.getSnapshot().bootstrap).toEqual(bootstrap));
    expect(store.getSnapshot().error).toBe("Services unavailable");
    store.dispose();
  });

  it.each(["retry", "dispose", "event"] as const)("ignores a late bootstrap from before %s", async (transition) => {
    const fake = createFakeApi();
    let finishBootstrap!: (value: BootstrapSnapshot) => void;
    fake.api.getBootstrap = vi.fn()
      .mockImplementationOnce(() => new Promise<BootstrapSnapshot>((resolve) => { finishBootstrap = resolve; }))
      .mockResolvedValue({ providers: [] });
    fake.api.listConversations = vi.fn()
      .mockRejectedValueOnce(new Error("Services unavailable"))
      .mockResolvedValue({ items: [], nextCursor: null });
    const store = createAppStore(fake.api);
    await store.initialize();
    if (transition === "retry") await store.retry();
    else if (transition === "dispose") store.dispose();
    else {
      fake.emit({ kind: "reloadRequired", sequence: "1" });
      await waitFor(() => expect(store.getSnapshot().phase).toBe("ready"));
    }
    const before = store.getSnapshot();
    finishBootstrap({ providers: [], startupDiagnostic: { code: "storage-error", message: "Obsolete failure", action: null } });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(store.getSnapshot()).toBe(before);
    store.dispose();
  });

  it("reports a failed bootstrap without waiting for the conversation request", async () => {
    const fake = createFakeApi();
    let finishConversations!: (value: ConversationPage) => void;
    fake.api.getBootstrap = vi.fn().mockRejectedValue(new Error("Bootstrap disconnected"));
    fake.api.listConversations = vi.fn(() => new Promise<ConversationPage>((resolve) => { finishConversations = resolve; }));
    const store = createAppStore(fake.api);
    await store.initialize();
    expect(store.getSnapshot()).toMatchObject({ phase: "error", error: "Bootstrap disconnected", bootstrap: null });
    finishConversations({ items: [], nextCursor: null });
    store.dispose();
  });

  it("adds and selects a newly created projectless conversation on an empty install", async () => {
    const fake = createFakeApi();
    fake.api.listConversations = vi.fn().mockResolvedValue({ items: [], nextCursor: null });
    fake.api.createConversation = vi.fn().mockResolvedValue(conversation("new", {
      currentRunId: null, provider: null, runStatus: null, rollupStatus: null,
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
      listChildConversations: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
      loadConversationPath: vi.fn(),
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
    const relevant = conversation("c1", { hasChildren: false });
    const unrelated = conversation("c2", { hasChildren: false });
    const archived = conversation("c3", { archived: true });
    const listChildConversations = vi.fn(async ({ parentId }: { parentId: string }) => ({
      items: parentId === "c1" ? [child("reviewer", "c1")] : [], nextCursor: null,
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
      listChildConversations,
      loadConversationPath: vi.fn(),
      listenToAppEvents: vi.fn(async (nextHandler) => {
        handler = nextHandler;
        return () => {};
      }),
    };
    const store = createAppStore(api);
    await store.initialize();
    expect(listChildConversations.mock.calls.map(([request]) => request.parentId)).not.toContain("c3");
    listConversations.mockClear();
    loadConversation.mockClear();
    listChildConversations.mockClear();

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
    expect(listChildConversations).not.toHaveBeenCalled();
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
      listChildConversations: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
      loadConversationPath: vi.fn(),
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

describe("durable child conversation navigation", () => {
  it("loads direct children explicitly, with independent pages for deeper branches", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    expect(fake.api.listChildConversations).not.toHaveBeenCalled();
    store.toggleConversation("c1");
    await waitFor(() => expect(store.getSnapshot().conversationsById.reviewer).toBeDefined());
    expect(fake.api.listChildConversations).toHaveBeenCalledWith({ parentId: "c1", cursor: null, limit: 20 });
    expect(store.getSnapshot().conversationsById.researcher).toBeUndefined();
    await store.loadChildPage("c1");
    expect(store.getSnapshot().conversationsById.sibling?.parentId).toBe("c1");
    store.toggleConversation("reviewer");
    await waitFor(() => expect(store.getSnapshot().conversationsById.researcher?.parentId).toBe("reviewer"));
    expect(Object.isFrozen(store.getSnapshot().childPagesById.c1?.pages)).toBe(true);
  });

  it("auto-reveals the selected root's first child once and preserves explicit collapse", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    fake.api.loadConversation = vi.fn(async () => conversation("c1", { hasChildren: true }));
    fake.emit({ kind: "runChanged", sequence: "1", conversationId: "c1", runId: "run-c1" });
    await waitFor(() => expect(store.getSnapshot().conversationsById.reviewer).toBeDefined());
    expect(store.getSnapshot().expandedById.c1).toBe(true);
    store.toggleConversation("c1");
    await store.selectConversation("c2");
    await store.selectConversation("c1");
    vi.mocked(fake.api.listChildConversations).mockClear();
    fake.emit({ kind: "runChanged", sequence: "2", conversationId: "c1", runId: "run-c1" });
    await waitFor(() => expect(store.getSnapshot().conversationVersions.c1).toBe(2));
    expect(store.getSnapshot().expandedById.c1).toBe(false);
    expect(fake.api.listChildConversations).not.toHaveBeenCalled();
  });

  it("refreshes an open empty branch when children arrive and keeps closed branches lazy", async () => {
    const fake = createFakeApi();
    fake.api.listChildConversations = vi.fn().mockResolvedValue({ items: [], nextCursor: null });
    const store = createAppStore(fake.api);
    await store.initialize();
    store.toggleConversation("c1");
    await waitFor(() => expect(store.getSnapshot().childPagesById.c1?.loading).toBe(false));
    vi.mocked(fake.api.listChildConversations).mockResolvedValue({ items: [child("late", "c1")], nextCursor: null });
    fake.emit({ kind: "runChanged", sequence: "1", conversationId: "c1", runId: "run-c1" });
    await waitFor(() => expect(store.getSnapshot().conversationsById.late).toBeDefined());
    expect(vi.mocked(fake.api.listChildConversations).mock.calls.every(([request]) => request.parentId === "c1")).toBe(true);
  });

  it("coalesces child restarts behind one in-flight read", async () => {
    const fake = createFakeApi();
    const resolvers: Array<(value: ConversationPage) => void> = [];
    fake.api.listChildConversations = vi.fn(() => new Promise<ConversationPage>(resolve => resolvers.push(resolve)));
    const store = createAppStore(fake.api);
    await store.initialize();
    const first = store.loadChildPage("c1", true);
    void store.loadChildPage("c1", true);
    void store.loadChildPage("c1", true);
    expect(fake.api.listChildConversations).toHaveBeenCalledTimes(1);
    resolvers.shift()!({ items: [child("stale", "c1")], nextCursor: null });
    await waitFor(() => expect(fake.api.listChildConversations).toHaveBeenCalledTimes(2));
    resolvers.shift()!({ items: [child("latest", "c1")], nextCursor: null });
    await first;
    expect(store.getSnapshot().conversationsById.latest).toBeDefined();
    expect(store.getSnapshot().conversationsById.stale).toBeUndefined();
  });

  it("preserves a manually paged open branch across run updates with coherent newest-first cursors", async () => {
    const fake = createFakeApi();
    let children = Array.from({ length: 60 }, (_, index) => child(`sibling-${index}`, "c1"));
    fake.api.listChildConversations = vi.fn(async ({ parentId, cursor, limit }) => {
      if (parentId !== "c1") return { items: [], nextCursor: null };
      const offset = cursor ? children.findIndex(item => item.id === cursor) + 1 : 0;
      const items = children.slice(offset, offset + limit);
      return { items, nextCursor: offset + items.length < children.length ? items.at(-1)!.id : null };
    });
    const store = createAppStore(fake.api);
    await store.initialize();
    store.toggleConversation("c1");
    await waitFor(() => expect(store.getSnapshot().childPagesById.c1?.loading).toBe(false));
    await store.loadChildPage("c1");
    store.toggleConversation("sibling-30");
    await waitFor(() => expect(store.getSnapshot().childPagesById["sibling-30"]?.loading).toBe(false));
    const previousIds = store.getSnapshot().childPagesById.c1!.pages.flatMap(page => page.ids);

    fake.emit({ kind: "runChanged", sequence: "1", conversationId: "c1", runId: "run-c1" });
    await waitFor(() => expect(store.getSnapshot().conversationVersions.c1).toBe(1));
    await waitFor(() => expect(store.getSnapshot().childPagesById.c1?.loading).toBe(false));
    expect(store.getSnapshot().childPagesById.c1!.pages.flatMap(page => page.ids)).toEqual(previousIds);
    expect(store.getSnapshot().expandedById["sibling-30"]).toBe(true);
    expect(store.getSnapshot().childPagesById.c1?.evicted).toBe(false);

    children = [child("newest", "c1"), ...children];
    await store.loadChildPage("c1", true);
    expect(store.getSnapshot().childPagesById.c1!.pages.flatMap(page => page.ids)).toEqual(children.slice(0, 40).map(item => item.id));
    expect(store.getSnapshot().childPagesById.c1?.evicted).toBe(true);
    await store.loadChildPage("c1");
    expect(store.getSnapshot().childPagesById.c1!.pages.flatMap(page => page.ids)).toEqual(children.slice(0, 60).map(item => item.id));
    await store.loadChildPage("c1");
    expect(store.getSnapshot().childPagesById.c1?.nextCursor).toBeNull();
    await store.loadChildPage("c1", true);
    expect(store.getSnapshot().childPagesById.c1!.pages.flatMap(page => page.ids)).toEqual(children.map(item => item.id));
    expect(store.getSnapshot().childPagesById.c1?.evicted).toBe(false);
  });

  it("keeps eviction explicit when a terminal child page still has an earlier cache gap", async () => {
    const fake = createFakeApi();
    fake.api.listChildConversations = vi.fn(async ({ parentId, cursor, limit }) => {
      const total = parentId === "c1" ? 61 : 40;
      const offset = Number(cursor ?? 0);
      return { items: Array.from({ length: Math.min(limit, total - offset) }, (_, index) => child(`${parentId}-${offset + index}`, parentId)),
        nextCursor: offset + limit < total ? String(offset + limit) : null };
    });
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadChildPage("c1");
    await store.loadChildPage("c2");
    await store.loadChildPage("c2");
    await store.loadChildPage("c1");
    await store.loadChildPage("c1");
    await store.loadChildPage("c1");
    const page = store.getSnapshot().childPagesById.c1!;
    expect(page.nextCursor).toBeNull();
    expect(page.pages[0]?.cursor).not.toBeNull();
    expect(page.pages.flatMap(item => item.ids)).not.toContain("c1-0");
    expect(page.evicted).toBe(true);
  });

  it("reasserts eviction when the shared budget prunes a complete child reload", async () => {
    const fake = createFakeApi();
    fake.api.listChildConversations = vi.fn().mockResolvedValue({
      items: Array.from({ length: 20 }, (_, index) => child(`sibling-${index}`, "c1")), nextCursor: null,
    });
    const chain = Array.from({ length: 63 }, (_, index) => child(`deep-${index}`, index ? `deep-${index - 1}` : "c2"));
    fake.api.loadConversationPath = vi.fn().mockResolvedValue({
      items: [conversation("c2"), ...chain], truncated: false, ownerConversationId: "c2",
    });
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadChildPage("c1");
    await store.selectConversation("deep-62");
    expect(store.getSnapshot().childPagesById.c1?.evicted).toBe(true);
    await store.loadChildPage("c1", true);
    expect(store.getSnapshot().childPagesById.c1?.nextCursor).toBeNull();
    expect(store.getSnapshot().childPagesById.c1?.pages).toEqual([]);
    expect(store.getSnapshot().childPagesById.c1?.evicted).toBe(true);
    expect(store.getSnapshot().selectedConversationId).toBe("deep-62");
  });

  it("keeps an existing paged window intact if a refresh fails partway through", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadChildPage("c1");
    await store.loadChildPage("c1");
    const previous = store.getSnapshot().childPagesById.c1!;
    const load = fake.api.listChildConversations;
    fake.api.listChildConversations = vi.fn(async request => {
      if (request.cursor !== null) throw new Error("Later page unavailable");
      return load(request);
    });
    await store.loadChildPage("c1", true);
    expect(store.getSnapshot().childPagesById.c1?.pages).toEqual(previous.pages);
    expect(store.getSnapshot().childPagesById.c1?.nextCursor).toEqual(previous.nextCursor);
    expect(store.getSnapshot().childPagesById.c1?.error).toBe("Later page unavailable");
  });

  it("reloads a closed cached branch when it is opened again", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    store.toggleConversation("c1");
    await waitFor(() => expect(store.getSnapshot().childPagesById.c1?.loading).toBe(false));
    store.toggleConversation("c1");
    vi.mocked(fake.api.listChildConversations).mockResolvedValue({ items: [child("later", "c1")], nextCursor: null });
    store.toggleConversation("c1");
    await waitFor(() => expect(store.getSnapshot().conversationsById.later).toBeDefined());
  });

  it("refreshes a selected hidden child's history without reading its hidden child pages", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    store.toggleConversation("c1");
    await waitFor(() => expect(store.getSnapshot().conversationsById.reviewer).toBeDefined());
    store.toggleConversation("reviewer");
    await waitFor(() => expect(store.getSnapshot().conversationsById.researcher).toBeDefined());
    await store.selectConversation("reviewer");
    store.toggleConversation("c1");
    vi.mocked(fake.api.listChildConversations).mockClear();
    fake.emit({ kind: "runChanged", sequence: "1", conversationId: "c1", runId: "run-c1" });
    await waitFor(() => expect(store.getSnapshot().conversationVersions.reviewer).toBe(1));
    expect(fake.api.listChildConversations).not.toHaveBeenCalled();
    expect(store.getSnapshot().selectedConversationId).toBe("reviewer");
  });

  it("loads missing child windows for visible ancestors revealed by path selection", async () => {
    const fake = createFakeApi();
    fake.api.loadConversationPath = vi.fn(async () => ({ items: [
      conversation("c1", { hasChildren: true }),
      { ...child("reviewer", "c1"), hasChildren: true }, child("researcher", "reviewer"),
    ], truncated: false, ownerConversationId: "c1" }));
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.selectConversation("researcher");
    await waitFor(() => expect(store.getSnapshot().childPagesById.reviewer?.loading).toBe(false));
    expect(store.getSnapshot().childPagesById.c1?.pages).toHaveLength(1);
    expect(store.getSnapshot().selectedConversationId).toBe("researcher");
  });

  it("keeps the selected child's bound run through a parent turn and a root-only full refresh", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadChildPage("c1");
    await store.selectConversation("reviewer");
    fake.api.loadConversation = vi.fn(async ({ conversationId }) => conversationId === "c1"
      ? conversation("c1", { currentRunId: "run-new" }) : child("reviewer", "c1", "completed"));
    fake.emit({ kind: "runChanged", sequence: "1", conversationId: "c1", runId: "run-new" });
    await waitFor(() => expect(store.getSnapshot().conversationsById.c1?.currentRunId).toBe("run-new"));
    expect(store.getSnapshot().selectedConversationId).toBe("reviewer");
    expect(store.getSnapshot().conversationsById.reviewer?.currentRunId).toBe("run-c1");
    await store.retry();
    await waitFor(() => expect(store.getSnapshot().conversationsById.reviewer?.runStatus).toBe("completed"));
    expect(store.getSnapshot().selectedConversationId).toBe("reviewer");
    expect(store.getSnapshot().conversationIds).toEqual(["c1", "c2"]);
  });

  it("retries failed child pages without changing selection or disclosure", async () => {
    const fake = createFakeApi();
    fake.api.listChildConversations = vi.fn().mockRejectedValueOnce(new Error("Unavailable"))
      .mockResolvedValue({ items: [child("reviewer", "c1")], nextCursor: null });
    const store = createAppStore(fake.api);
    await store.initialize();
    store.toggleConversation("c1");
    await waitFor(() => expect(store.getSnapshot().childPagesById.c1?.error).toBe("Unavailable"));
    await store.loadChildPage("c1", true);
    expect(store.getSnapshot().expandedById.c1).toBe(true);
    expect(store.getSnapshot().selectedConversationId).toBe("c1");
    expect(store.getSnapshot().childPagesById.c1?.error).toBeNull();
  });

  it("bounds descendants across branches while preserving the selected loaded path", async () => {
    const fake = createFakeApi();
    let page = 0;
    fake.api.listChildConversations = vi.fn(async ({ parentId, cursor }) => {
      if (parentId === "reviewer") return { items: [child("researcher", "reviewer")], nextCursor: null };
      page += 1;
      return { items: cursor === null ? [child("reviewer", "c1")] :
        Array.from({ length: 20 }, (_, index) => child(`later-${page}-${index}`, "c1")), nextCursor: `next-${page}` };
    });
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadChildPage("c1");
    await store.loadChildPage("reviewer");
    await store.selectConversation("researcher");
    for (let index = 0; index < 6; index++) await store.loadChildPage("c1");
    const snapshot = store.getSnapshot();
    expect(snapshot.selectedConversationId).toBe("researcher");
    expect(snapshot.selectedPath?.ids).toEqual(["c1", "reviewer", "researcher"]);
    expect(Object.values(snapshot.conversationsById).filter(item => item.parentId !== null).length).toBeLessThanOrEqual(80);
    expect(snapshot.childPagesById.c1?.evicted).toBe(true);
    await store.selectConversation("c2");
    expect(store.getSnapshot().conversationsById.researcher).toBeUndefined();
  });

  it("does not retain omitted ancestry beyond the 80-node budget across deep selections", async () => {
    const fake = createFakeApi();
    const chain = Array.from({ length: 110 }, (_, index) => child(`deep-${index}`, index ? `deep-${index - 1}` : "c1"));
    fake.api.loadConversationPath = vi.fn(async ({ conversationId }): Promise<ConversationPath> => {
      const index = chain.findIndex(item => item.id === conversationId);
      return { items: chain.slice(Math.max(0, index - 63), index + 1), truncated: index >= 63, ownerConversationId: "c1" };
    });
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.selectConversation("deep-79");
    await store.selectConversation("deep-99");
    expect(store.getSnapshot().selectedPath?.truncated).toBe(true);
    expect(Object.values(store.getSnapshot().conversationsById).filter(item => item.parentId !== null).length).toBeLessThanOrEqual(80);
  });

  it("fences child pages and truncated selected paths when their root is archived", async () => {
    const fake = createFakeApi();
    let finish!: (page: ConversationPage) => void;
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.selectConversation("detached-child");
    fake.api.listChildConversations = vi.fn(() => new Promise<ConversationPage>(resolve => { finish = resolve; }));
    const loading = store.loadChildPage("c1", true);
    await store.archiveConversation("c1");
    finish({ items: [child("stale", "c1")], nextCursor: null });
    await loading;
    expect(store.getSnapshot().selectedConversationId).toBe("c2");
    expect(store.getSnapshot().conversationsById["detached-child"]).toBeUndefined();
    expect(store.getSnapshot().conversationsById.stale).toBeUndefined();
    expect(store.getSnapshot().childPagesById.c1).toBeUndefined();
  });

  it("does not republish an archived root from a full refresh that began before archive", async () => {
    const fake = createFakeApi();
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadChildPage("c1");
    await store.selectConversation("reviewer");
    let finish!: (bootstrap: BootstrapSnapshot) => void;
    vi.mocked(fake.api.getBootstrap).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    const refreshing = store.retry();
    await waitFor(() => expect(fake.calls.conversations).toBe(4));
    await store.archiveConversation("c1");
    fake.api.listConversations = vi.fn().mockResolvedValue({ items: [conversation("c2")], nextCursor: null });
    finish({ providers: [] });
    await refreshing;
    expect(store.getSnapshot().conversationIds).toEqual(["c2"]);
    expect(store.getSnapshot().selectedConversationId).toBe("c2");
    expect(store.getSnapshot().conversationsById.reviewer).toBeUndefined();
  });

  it("ignores path failures from before an authoritative full refresh", async () => {
    const fake = createFakeApi();
    let reject!: (reason: Error) => void;
    fake.api.loadConversationPath = vi.fn(() => new Promise<ConversationPath>((_resolve, fail) => { reject = fail; }));
    const store = createAppStore(fake.api);
    await store.initialize();
    const selecting = store.selectConversation("missing");
    await store.retry();
    reject(new Error("Stale path failure"));
    await selecting;
    expect(store.getSnapshot().error).toBeNull();
    expect(store.getSnapshot().selectedConversationId).toBe("c1");
  });

  it("rejects a child page attributed to another parent", async () => {
    const fake = createFakeApi();
    fake.api.listChildConversations = vi.fn().mockResolvedValue({ items: [child("wrong", "c2")], nextCursor: null });
    const store = createAppStore(fake.api);
    await store.initialize();
    await store.loadChildPage("c1");
    expect(store.getSnapshot().childPagesById.c1?.error).toMatch(/ownership/);
    expect(store.getSnapshot().conversationsById.wrong).toBeUndefined();
  });
});
