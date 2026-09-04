import { createContext, useContext, useSyncExternalStore } from "react";
import type {
  AgentSnapshot,
  AgentStatus,
  AgentTreePage,
  AppEvent,
  ApprovalDetailSnapshot,
  ApprovalPage,
  ApprovalQuestionPage,
  BootstrapSnapshot,
  ConversationPage,
  ConversationSummary,
  CreateConversationRequest,
  EventDetailSnapshot,
  InspectorSnapshot,
  ProviderId,
  RunAuditDetailSnapshot,
  RunAuditPage,
  ProjectPathSnapshot,
  RespondToApprovalRequest,
  SubmissionSnapshot,
  TimelinePage,
} from "../bridge/types";

const PAGE_SIZE = 200;
const AGENT_LOAD_BATCH_SIZE = 8;
const AGENT_PAGE_SIZE = 20;
const MAX_AGENT_PAGES = 4;
const MAX_PAGES = 10_000;
const MAX_SEQUENCE = (1n << 64n) - 1n;

export type EffectiveStatus = "idle" | AgentStatus;
export type StatusFilter = "all" | EffectiveStatus;

export type AppApi = {
  getBootstrap(): Promise<BootstrapSnapshot>;
  listConversations(request: { cursor: string | null; limit: number }): Promise<ConversationPage>;
  loadConversation(request: { conversationId: string }): Promise<ConversationSummary>;
  loadAgentTree(request: {
    conversationId: string;
    cursor: string | null;
    limit: number;
  }): Promise<AgentTreePage>;
  listenToAppEvents(handler: (event: AppEvent) => void): Promise<() => void>;
  loadTimeline(request: { conversationId: string; cursor: string | null; limit: number }): Promise<TimelinePage>;
  loadEventDetail(request: { eventId: string }): Promise<EventDetailSnapshot>;
  loadApprovals(request: { conversationId: string; cursor: string | null; limit: number; kind: "pending" | "history" }): Promise<ApprovalPage>;
  loadApprovalDetail(request: { approvalId: string }): Promise<ApprovalDetailSnapshot>;
  loadApprovalQuestions(request: { approvalId: string; cursor: string | null; limit: number }): Promise<ApprovalQuestionPage>;
  submitMessage(request: { conversationId: string; text: string; providerOverride: ProviderId | null; commandId: string }): Promise<SubmissionSnapshot>;
  steerRun(request: { runId: string; text: string }): Promise<void>;
  respondToApproval(request: RespondToApprovalRequest): Promise<void>;
  interruptRun(request: { runId: string }): Promise<void>;
  inspectWorkspace(request: { conversationId: string }): Promise<InspectorSnapshot>;
  inspectProject(request: { path: string }): Promise<ProjectPathSnapshot>;
  createConversation(request: CreateConversationRequest): Promise<ConversationSummary>;
  archiveConversation(request: { conversationId: string }): Promise<void>;
  listRunAudits(request: { conversationId: string; cursor: string | null; limit: number }): Promise<RunAuditPage>;
  loadRunAudit(request: { conversationId: string; runId: string }): Promise<RunAuditDetailSnapshot>;
};

export type ConversationActions = Pick<
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
>;

export type AppActions = ConversationActions & Pick<AppApi, "listRunAudits" | "loadRunAudit">;

export type NormalizedConversation = Omit<ConversationSummary, "agents"> & {
  summaryAgentsTruncated: boolean;
  summaryAgentIds: readonly string[];
  agentIds: readonly string[];
};

export type AgentWindowSnapshot = Readonly<{
  conversationId: string;
  runId: string;
  pages: readonly (readonly string[])[];
  nextCursor: string | null;
  loading: boolean;
  error: string | null;
  evicted: boolean;
}>;

export type AppSnapshot = Readonly<{
  phase: "idle" | "loading" | "ready" | "error";
  error: string | null;
  bootstrap: BootstrapSnapshot | null;
  conversationsById: Readonly<Record<string, NormalizedConversation>>;
  conversationIds: readonly string[];
  agentsById: Readonly<Record<string, AgentSnapshot>>;
  agentWindow: AgentWindowSnapshot | null;
  selectedConversationId: string | null;
  selectedAgentId: string | null;
  statusFilter: StatusFilter;
  queuedCount: number;
  lastSequence: string | null;
  conversationVersions: Readonly<Record<string, number>>;
}>;

export type AppStore = {
  getSnapshot(): AppSnapshot;
  subscribe(listener: () => void): () => void;
  initialize(): Promise<void>;
  retry(): Promise<void>;
  dispose(): void;
  loadAgentPage(conversationId: string, restart?: boolean): Promise<void>;
  createConversation(request: CreateConversationRequest): Promise<void>;
  archiveConversation(conversationId: string): Promise<void>;
  inspectProject(path: string): Promise<ProjectPathSnapshot>;
  selectConversation(conversationId: string, agentId?: string): void;
  setStatusFilter(filter: StatusFilter): void;
  refreshConversation(conversationId: string): void;
  readonly actions: AppActions;
};

const emptySnapshot: AppSnapshot = freezeSnapshot({
  phase: "idle",
  error: null,
  bootstrap: null,
  conversationsById: {},
  conversationIds: [],
  agentsById: {},
  agentWindow: null,
  selectedConversationId: null,
  selectedAgentId: null,
  statusFilter: "all",
  queuedCount: 0,
  lastSequence: null,
  conversationVersions: {},
});

export function createAppStore(api: AppApi): AppStore {
  let snapshot = emptySnapshot;
  const listeners = new Set<() => void>();
  let initializePromise: Promise<void> | null = null;
  let refreshPromise: Promise<void> | null = null;
  let refreshRequested = false;
  let fullRefreshEpoch = 0;
  let successfulSynchronizations = 0;
  let eventRevision = 0;
  let rolloverExpected = false;
  let targetedRefreshScheduled = false;
  let targetedRefreshPromise: Promise<void> | null = null;
  const targetedConversationVersions = new Map<string, number>();
  const pendingConversationRefreshes = new Set<string>();
  let agentLoadGeneration = 0;
  let agentLoadPromise: Promise<void> | null = null;
  let agentLoadConversation: string | null = null;
  let queuedAgentRestart: string | null = null;
  let inspectPromise: Promise<InspectorSnapshot> | null = null;
  let queuedInspectRequest: { conversationId: string } | null = null;
  let queuedInspectWaiters: Array<{
    resolve(value: InspectorSnapshot): void;
    reject(reason: unknown): void;
  }> = [];
  let disposed = false;
  let unlisten: (() => void) | null = null;

  function publish(next: AppSnapshot) {
    if (next === snapshot) return;
    snapshot = freezeSnapshot(next);
    listeners.forEach((listener) => listener());
  }

  function update(changes: Partial<AppSnapshot>) {
    publish({ ...snapshot, ...changes });
  }

  async function synchronize() {
    fullRefreshEpoch += 1;
    const revisionAtStart = eventRevision;
    const [bootstrap, conversations] = await Promise.all([
      api.getBootstrap(),
      loadAllConversations(api),
    ]);
    const activeConversations = conversations.filter(({ archived }) => !archived);
    const base = normalizeConversations(activeConversations);
    const retained = retainAgentState(snapshot, base.normalized, base.agentsById);
    const { normalized, agentsById, agentWindow } = retained;
    if (disposed) return;
    if (revisionAtStart !== eventRevision) {
      refreshRequested = true;
      return;
    }

    const conversationIds = activeConversations.map(({ id }) => id);
    const selectedConversationId = snapshot.selectedConversationId
      && normalized[snapshot.selectedConversationId]
      ? snapshot.selectedConversationId
      : activeConversations[0]?.id ?? null;
    const selectedConversation = selectedConversationId
      ? normalized[selectedConversationId]
      : null;
    const selectedAgentId = selectedConversation
      ? selectedConversation.agentIds.includes(snapshot.selectedAgentId ?? "")
        ? snapshot.selectedAgentId
        : null
      : snapshot.selectedAgentId;

    const conversationVersions = successfulSynchronizations === 0
      ? snapshot.conversationVersions
      : incrementAllConversationVersions(snapshot);
    successfulSynchronizations += 1;
    publish({
      ...snapshot,
      phase: "ready",
      error: null,
      bootstrap,
      conversationsById: normalized,
      conversationIds,
      agentsById,
      agentWindow,
      selectedConversationId,
      selectedAgentId,
      queuedCount: countQueued(normalized),
      conversationVersions,
    });
    if (agentWindow) void loadAgentPage(agentWindow.conversationId, true);
  }

  function requestRefresh(): Promise<void> {
    refreshRequested = true;
    if (refreshPromise) return refreshPromise;
    refreshPromise = (async () => {
      while (refreshRequested && !disposed) {
        refreshRequested = false;
        try {
          await synchronize();
        } catch (reason) {
          if (!disposed) {
            update({
              phase: "error",
              error: reason instanceof Error ? reason.message : "Prompting Time could not load.",
            });
          }
        }
      }
    })().finally(() => {
      refreshPromise = null;
    });
    return refreshPromise;
  }

  function requestConversationRefresh(conversationId: string) {
    targetedConversationVersions.set(
      conversationId,
      (targetedConversationVersions.get(conversationId) ?? 0) + 1,
    );
    pendingConversationRefreshes.add(conversationId);
    scheduleTargetedRefresh();
  }

  function scheduleTargetedRefresh() {
    if (targetedRefreshScheduled || targetedRefreshPromise || disposed) return;
    targetedRefreshScheduled = true;
    queueMicrotask(() => {
      targetedRefreshScheduled = false;
      if (disposed || targetedRefreshPromise) return;
      targetedRefreshPromise = drainTargetedRefreshes().finally(() => {
        targetedRefreshPromise = null;
        if (pendingConversationRefreshes.size > 0) scheduleTargetedRefresh();
      });
    });
  }

  async function drainTargetedRefreshes() {
    while (pendingConversationRefreshes.size > 0 && !disposed) {
      const ids = [...pendingConversationRefreshes].slice(0, AGENT_LOAD_BATCH_SIZE);
      const versions = new Map(ids.map((id) => [id, targetedConversationVersions.get(id) ?? 0]));
      ids.forEach((id) => pendingConversationRefreshes.delete(id));
      const epoch = fullRefreshEpoch;
      let refreshed: Array<{
        id: string;
        normalized: Record<string, NormalizedConversation>;
        agentsById: Record<string, AgentSnapshot>;
      }>;
      try {
        refreshed = await Promise.all(ids.map(async (id) => {
          const conversation = await api.loadConversation({ conversationId: id });
          const result = normalizeConversations([conversation]);
          return { id, ...result };
        }));
      } catch {
        void requestRefresh();
        return;
      }
      if (disposed || epoch !== fullRefreshEpoch) continue;
      refreshed.forEach((result) => {
        if (versions.get(result.id) !== targetedConversationVersions.get(result.id)) return;
        mergeConversation(result.id, result.normalized, result.agentsById);
      });
    }
  }

  function mergeConversation(
    conversationId: string,
    normalized: Record<string, NormalizedConversation>,
    refreshedAgents: Record<string, AgentSnapshot>,
  ) {
    const refreshedConversation = normalized[conversationId];
    if (!refreshedConversation) return;
    if (refreshedConversation.archived) {
      removeConversation(conversationId);
      return;
    }
    const previousConversation = snapshot.conversationsById[conversationId];
    const sameRun = previousConversation?.currentRunId === refreshedConversation.currentRunId;
    const retainedIds = sameRun ? previousConversation.agentIds : [];
    const agentIds = uniqueAgentIds([
      ...refreshedConversation.summaryAgentIds,
      ...retainedIds,
    ]);
    const retainedWindow = sameRun && snapshot.agentWindow?.conversationId === conversationId
      ? { ...snapshot.agentWindow, loading: false }
      : snapshot.agentWindow?.conversationId === conversationId
        ? null
        : snapshot.agentWindow;
    if (snapshot.agentWindow?.conversationId === conversationId) agentLoadGeneration += 1;
    const conversation = Object.freeze({
      ...refreshedConversation,
      agentsTruncated: retainedWindow?.conversationId === conversationId
        ? retainedWindow.nextCursor !== null || retainedWindow.evicted
        : refreshedConversation.summaryAgentsTruncated,
      agentIds: Object.freeze(agentIds),
    });
    const conversationsById = { ...snapshot.conversationsById, [conversationId]: conversation };
    const agentsById = { ...snapshot.agentsById };
    previousConversation?.agentIds.forEach((id) => {
      if (!agentIds.includes(id)) delete agentsById[id];
    });
    Object.assign(agentsById, refreshedAgents);
    const conversationIds = [
      conversationId,
      ...snapshot.conversationIds.filter((id) => id !== conversationId),
    ];
    const selectedConversationId = snapshot.selectedConversationId
      ?? (conversation.archived ? null : conversationId);
    const selectedAgentId = selectedConversationId === conversationId
      && snapshot.selectedAgentId
      && !conversation.agentIds.includes(snapshot.selectedAgentId)
      ? null
      : snapshot.selectedAgentId;
    publish({
      ...snapshot,
      conversationsById,
      conversationIds,
      agentsById,
      agentWindow: retainedWindow,
      selectedConversationId,
      selectedAgentId,
      queuedCount: countQueued(conversationsById),
      conversationVersions: {
        ...snapshot.conversationVersions,
        [conversationId]: (snapshot.conversationVersions[conversationId] ?? 0) + 1,
      },
    });
    if (sameRun && retainedWindow?.conversationId === conversationId) {
      void loadAgentPage(conversationId, true);
    }
  }

  function removeConversation(conversationId: string) {
    targetedConversationVersions.delete(conversationId);
    pendingConversationRefreshes.delete(conversationId);
    const removed = snapshot.conversationsById[conversationId];
    if (!removed) return;
    const conversationsById = { ...snapshot.conversationsById };
    delete conversationsById[conversationId];
    const agentsById = { ...snapshot.agentsById };
    removed.agentIds.forEach((id) => delete agentsById[id]);
    const conversationIds = snapshot.conversationIds.filter((id) => id !== conversationId);
    const conversationVersions = { ...snapshot.conversationVersions };
    delete conversationVersions[conversationId];
    const selectionRemoved = snapshot.selectedConversationId === conversationId;
    if (snapshot.agentWindow?.conversationId === conversationId) agentLoadGeneration += 1;
    publish({
      ...snapshot,
      conversationsById,
      conversationIds,
      agentsById,
      agentWindow: snapshot.agentWindow?.conversationId === conversationId ? null : snapshot.agentWindow,
      selectedConversationId: selectionRemoved ? conversationIds[0] ?? null : snapshot.selectedConversationId,
      selectedAgentId: selectionRemoved ? null : snapshot.selectedAgentId,
      queuedCount: countQueued(conversationsById),
      conversationVersions,
    });
  }

  async function createConversation(request: CreateConversationRequest) {
    const conversation = await api.createConversation(request);
    if (disposed) return;
    const { normalized, agentsById: createdAgents } = normalizeConversations([conversation]);
    const created = normalized[conversation.id];
    if (!created || created.archived) throw new Error("Prompting Time created an unavailable conversation.");
    const previousWindow = snapshot.agentWindow;
    let conversationsById = { ...snapshot.conversationsById, [conversation.id]: created };
    let agentsById = { ...snapshot.agentsById, ...createdAgents };
    if (previousWindow) {
      ({ conversationsById, agentsById } = pruneConversationAgents(
        snapshot, conversationsById, agentsById, previousWindow.conversationId,
      ));
      agentLoadGeneration += 1;
    }
    publish({
      ...snapshot,
      conversationsById,
      conversationIds: [conversation.id, ...snapshot.conversationIds.filter((id) => id !== conversation.id)],
      agentsById,
      agentWindow: null,
      selectedConversationId: conversation.id,
      selectedAgentId: null,
      queuedCount: countQueued(conversationsById),
      conversationVersions: { ...snapshot.conversationVersions, [conversation.id]: 0 },
    });
  }

  async function archiveConversation(conversationId: string) {
    if (!snapshot.conversationsById[conversationId]) return;
    await api.archiveConversation({ conversationId });
    if (!disposed) removeConversation(conversationId);
  }

  function receiveEvent(event: AppEvent) {
    if (disposed) return;
    const nextSequence = parseSequence(event.sequence);
    const currentSequence = parseSequence(snapshot.lastSequence);
    if (nextSequence === null) {
      eventRevision += 1;
      update({ lastSequence: null });
      void requestRefresh();
      return;
    }
    const isRolloverReset = rolloverExpected
      && currentSequence === MAX_SEQUENCE
      && nextSequence > 0n
      && nextSequence < MAX_SEQUENCE;
    if (!isRolloverReset && currentSequence !== null && nextSequence <= currentSequence) {
      return;
    }
    const hasGap = isRolloverReset
      ? nextSequence !== 1n
      : currentSequence !== null && nextSequence !== currentSequence + 1n;
    if (isRolloverReset) rolloverExpected = false;
    if (event.kind === "reloadRequired" && nextSequence === MAX_SEQUENCE) {
      rolloverExpected = true;
    }
    eventRevision += 1;
    const fullRefresh = event.kind === "reloadRequired" || hasGap || snapshot.phase !== "ready";
    update({ lastSequence: nextSequence.toString() });
    if (fullRefresh) {
      void requestRefresh();
    } else {
      requestConversationRefresh(event.conversationId);
    }
  }

  async function initialize() {
    if (disposed || snapshot.phase === "ready") return;
    if (initializePromise) return initializePromise;
    initializePromise = (async () => {
      update({ phase: "loading", error: null });
      try {
        if (!unlisten) {
          const stopListening = await api.listenToAppEvents(receiveEvent);
          if (disposed) {
            stopListening();
            return;
          }
          unlisten = stopListening;
        }
        await requestRefresh();
      } catch (reason) {
        if (!disposed) {
          update({
            phase: "error",
            error: reason instanceof Error ? reason.message : "Prompting Time could not start.",
          });
        }
      }
    })().finally(() => {
      initializePromise = null;
    });
    return initializePromise;
  }

  function loadAgentPage(conversationId: string, restart = false): Promise<void> {
    if (agentLoadPromise) {
      if (restart || agentLoadConversation !== conversationId) queuedAgentRestart = conversationId;
      return agentLoadPromise;
    }
    agentLoadConversation = conversationId;
    agentLoadPromise = (async () => {
      await performAgentPage(conversationId, restart);
      while (queuedAgentRestart) {
        const nextConversation = queuedAgentRestart;
        queuedAgentRestart = null;
        agentLoadConversation = nextConversation;
        await performAgentPage(nextConversation, true);
      }
    })().finally(() => {
      agentLoadPromise = null;
      agentLoadConversation = null;
      if (queuedAgentRestart) void loadAgentPage(queuedAgentRestart, true);
    });
    return agentLoadPromise;
  }

  async function performAgentPage(conversationId: string, restart: boolean) {
    if (disposed) return;
    const conversation = snapshot.conversationsById[conversationId];
    if (!conversation?.currentRunId || conversation.archived) return;
    const currentWindow = snapshot.agentWindow;
    const startsNewWindow = restart
      || currentWindow?.conversationId !== conversationId
      || currentWindow.runId !== conversation.currentRunId;
    if (!startsNewWindow && (currentWindow.loading || currentWindow.nextCursor === null)) return;

    const cursor = startsNewWindow ? null : currentWindow.nextCursor;
    const generation = ++agentLoadGeneration;
    const epoch = fullRefreshEpoch;
    let conversationsById = { ...snapshot.conversationsById };
    let agentsById = { ...snapshot.agentsById };
    if (startsNewWindow) {
      if (currentWindow) {
        ({ conversationsById, agentsById } = pruneConversationAgents(
          snapshot,
          conversationsById,
          agentsById,
          currentWindow.conversationId,
        ));
      }
      ({ conversationsById, agentsById } = pruneConversationAgents(
        snapshot,
        conversationsById,
        agentsById,
        conversationId,
      ));
    }
    publish({
      ...snapshot,
      conversationsById,
      agentsById,
      agentWindow: {
        conversationId,
        runId: conversation.currentRunId,
        pages: startsNewWindow ? [] : currentWindow.pages,
        nextCursor: cursor,
        loading: true,
        error: null,
        evicted: startsNewWindow ? false : currentWindow.evicted,
      },
    });

    try {
      const page = await api.loadAgentTree({
        conversationId,
        cursor,
        limit: AGENT_PAGE_SIZE,
      });
      if (disposed || generation !== agentLoadGeneration || epoch !== fullRefreshEpoch) return;
      const latestConversation = snapshot.conversationsById[conversationId];
      const latestWindow = snapshot.agentWindow;
      if (
        !latestConversation
        || latestConversation.currentRunId !== conversation.currentRunId
        || latestWindow?.conversationId !== conversationId
        || latestWindow.runId !== conversation.currentRunId
      ) return;
      if (page.runId !== conversation.currentRunId) {
        update({
          agentWindow: {
            ...latestWindow,
            loading: false,
            error: "Agent activity changed while loading. Reload the conversation.",
          },
        });
        requestConversationRefresh(conversationId);
        return;
      }
      if (page.nextCursor !== null && page.nextCursor === cursor) {
        update({
          agentWindow: {
            ...latestWindow,
            loading: false,
            error: "Agent pagination did not advance.",
          },
        });
        return;
      }

      const nextAgents = { ...snapshot.agentsById };
      const pageIds: string[] = [];
      page.items.forEach(({ agent }) => {
        nextAgents[agent.id] = Object.freeze({ ...agent });
        if (!pageIds.includes(agent.id)) pageIds.push(agent.id);
      });
      const allPages = [...latestWindow.pages, Object.freeze(pageIds)];
      const evicted = latestWindow.evicted || allPages.length > MAX_AGENT_PAGES;
      const pages = allPages.slice(-MAX_AGENT_PAGES);
      const retainedIds = uniqueAgentIds([
        ...latestConversation.summaryAgentIds,
        ...selectedAgentPath(snapshot, conversationId),
        ...pages.flat(),
      ]);
      latestConversation.agentIds.forEach((id) => {
        if (!retainedIds.includes(id)) delete nextAgents[id];
      });
      const nextConversation = Object.freeze({
        ...latestConversation,
        agentsTruncated: page.nextCursor !== null || evicted,
        agentIds: Object.freeze(retainedIds),
      });
      publish({
        ...snapshot,
        conversationsById: {
          ...snapshot.conversationsById,
          [conversationId]: nextConversation,
        },
        agentsById: nextAgents,
        agentWindow: {
          conversationId,
          runId: conversation.currentRunId,
          pages,
          nextCursor: page.nextCursor,
          loading: false,
          error: null,
          evicted,
        },
      });
    } catch (reason) {
      if (disposed || generation !== agentLoadGeneration) return;
      const latestWindow = snapshot.agentWindow;
      if (latestWindow?.conversationId !== conversationId) return;
      update({
        agentWindow: {
          ...latestWindow,
          loading: false,
          error: reason instanceof Error ? reason.message : "Prompting Time could not load agents.",
        },
      });
    }
  }

  function inspectWorkspace(request: { conversationId: string }): Promise<InspectorSnapshot> {
    if (inspectPromise) {
      queuedInspectRequest = request;
      return new Promise((resolve, reject) => queuedInspectWaiters.push({ resolve, reject }));
    }
    inspectPromise = api.inspectWorkspace(request).finally(() => {
      inspectPromise = null;
      const next = queuedInspectRequest;
      const waiters = queuedInspectWaiters;
      queuedInspectRequest = null;
      queuedInspectWaiters = [];
      if (!next) return;
      void inspectWorkspace(next).then(
        (value) => waiters.forEach(({ resolve }) => resolve(value)),
        (reason) => waiters.forEach(({ reject }) => reject(reason)),
      );
    });
    return inspectPromise;
  }

  return {
    getSnapshot: () => snapshot,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    initialize,
    loadAgentPage,
    createConversation,
    archiveConversation,
    inspectProject: (path) => api.inspectProject({ path }),
    retry: async () => {
      if (disposed) return;
      if (!unlisten) {
        update({ phase: "idle", error: null });
        await initialize();
      } else {
        update({ phase: "loading", error: null });
        await requestRefresh();
      }
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      unlisten?.();
      unlisten = null;
      listeners.clear();
    },
    selectConversation(conversationId, agentId) {
      const conversation = snapshot.conversationsById[conversationId];
      if (!conversation) return;
      const selectedAgentId = agentId && conversation.agentIds.includes(agentId) ? agentId : null;
      let conversationsById = { ...snapshot.conversationsById };
      let agentsById = { ...snapshot.agentsById };
      const previousSelectedConversationId = snapshot.selectedConversationId;
      if (
        previousSelectedConversationId
        && snapshot.agentWindow?.conversationId !== previousSelectedConversationId
      ) {
        ({ conversationsById, agentsById } = pruneConversationAgents(
          {
            ...snapshot,
            selectedConversationId: conversationId,
            selectedAgentId,
          },
          conversationsById,
          agentsById,
          previousSelectedConversationId,
        ));
      }
      publish({
        ...snapshot,
        conversationsById,
        agentsById,
        selectedConversationId: conversationId,
        selectedAgentId,
      });
    },
    setStatusFilter(statusFilter) {
      if (snapshot.statusFilter !== statusFilter) update({ statusFilter });
    },
    refreshConversation(conversationId) {
      if (!snapshot.conversationsById[conversationId]) return;
      requestConversationRefresh(conversationId);
    },
    actions: {
      loadTimeline: api.loadTimeline,
      loadEventDetail: api.loadEventDetail,
      loadApprovals: api.loadApprovals,
      loadApprovalDetail: api.loadApprovalDetail,
      loadApprovalQuestions: api.loadApprovalQuestions,
      submitMessage: api.submitMessage,
      steerRun: api.steerRun,
      respondToApproval: api.respondToApproval,
      interruptRun: api.interruptRun,
      inspectWorkspace,
      listRunAudits: api.listRunAudits,
      loadRunAudit: api.loadRunAudit,
    },
  };
}

function incrementAllConversationVersions(snapshot: AppSnapshot): Record<string, number> {
  return Object.fromEntries(
    snapshot.conversationIds.map((id) => [id, (snapshot.conversationVersions[id] ?? 0) + 1]),
  );
}

async function loadAllConversations(api: AppApi): Promise<ConversationSummary[]> {
  const conversations: ConversationSummary[] = [];
  const seenCursors = new Set<string>();
  let cursor: string | null = null;
  for (let pageCount = 0; pageCount < MAX_PAGES; pageCount += 1) {
    const page = await api.listConversations({ cursor, limit: PAGE_SIZE });
    conversations.push(...page.items);
    if (page.nextCursor === null) return conversations;
    if (seenCursors.has(page.nextCursor)) throw new Error("Conversation pagination did not advance.");
    seenCursors.add(page.nextCursor);
    cursor = page.nextCursor;
  }
  throw new Error("Conversation pagination exceeded its safety bound.");
}

function normalizeConversations(
  conversations: ConversationSummary[],
): {
  normalized: Record<string, NormalizedConversation>;
  agentsById: Record<string, AgentSnapshot>;
} {
  const normalized: Record<string, NormalizedConversation> = {};
  const agentsById: Record<string, AgentSnapshot> = {};
  conversations.forEach((conversation) => {
    const agentIds: string[] = [];
    conversation.agents.forEach((agent) => {
      if (agentsById[agent.id]) return;
      agentsById[agent.id] = Object.freeze({ ...agent });
      agentIds.push(agent.id);
    });
    normalized[conversation.id] = Object.freeze({
      id: conversation.id,
      title: conversation.title,
      routingProfile: conversation.routingProfile,
      workspaceId: conversation.workspaceId,
      archived: conversation.archived,
      projectRoot: conversation.projectRoot,
      currentRunId: conversation.currentRunId,
      provider: conversation.provider,
      runStatus: conversation.runStatus,
      rollupStatus: conversation.rollupStatus,
      agentsTruncated: conversation.agentsTruncated,
      summaryAgentsTruncated: conversation.agentsTruncated,
      summaryAgentIds: Object.freeze([...agentIds]),
      agentIds: Object.freeze(agentIds),
    });
  });
  return { normalized, agentsById };
}

function retainAgentState(
  previous: AppSnapshot,
  normalized: Record<string, NormalizedConversation>,
  summaryAgents: Record<string, AgentSnapshot>,
) {
  const conversations = { ...normalized };
  const agentsById = { ...summaryAgents };
  const previousWindow = previous.agentWindow;
  const windowConversation = previousWindow
    ? conversations[previousWindow.conversationId]
    : null;
  const agentWindow = previousWindow
    && windowConversation?.currentRunId === previousWindow.runId
    && !windowConversation.archived
    ? { ...previousWindow, loading: false, error: null }
    : null;

  const retainForConversation = (conversationId: string, ids: readonly string[]) => {
    const conversation = conversations[conversationId];
    const oldConversation = previous.conversationsById[conversationId];
    if (!conversation || oldConversation?.currentRunId !== conversation.currentRunId) return;
    const retainedIds = ids.filter((id) => previous.agentsById[id] !== undefined);
    retainedIds.forEach((id) => {
      if (!agentsById[id]) agentsById[id] = previous.agentsById[id]!;
    });
    const agentIds = uniqueAgentIds([...conversation.summaryAgentIds, ...retainedIds]);
    conversations[conversationId] = Object.freeze({
      ...conversation,
      agentsTruncated: agentWindow?.conversationId === conversationId
        ? agentWindow.nextCursor !== null || agentWindow.evicted
        : conversation.summaryAgentsTruncated,
      agentIds: Object.freeze(agentIds),
    });
  };

  if (agentWindow) {
    retainForConversation(
      agentWindow.conversationId,
      previous.conversationsById[agentWindow.conversationId]?.agentIds ?? [],
    );
  }
  if (previous.selectedConversationId) {
    retainForConversation(
      previous.selectedConversationId,
      selectedAgentPath(previous, previous.selectedConversationId),
    );
  }
  return { normalized: conversations, agentsById, agentWindow };
}

function pruneConversationAgents(
  snapshot: AppSnapshot,
  conversationsById: Record<string, NormalizedConversation>,
  agentsById: Record<string, AgentSnapshot>,
  conversationId: string,
) {
  const conversation = conversationsById[conversationId];
  if (!conversation) return { conversationsById, agentsById };
  const retainedIds = uniqueAgentIds([
    ...conversation.summaryAgentIds,
    ...selectedAgentPath(snapshot, conversationId),
  ]);
  conversation.agentIds.forEach((id) => {
    if (!retainedIds.includes(id)) delete agentsById[id];
  });
  return {
    conversationsById: {
      ...conversationsById,
      [conversationId]: Object.freeze({
        ...conversation,
        agentsTruncated: conversation.summaryAgentsTruncated,
        agentIds: Object.freeze(retainedIds),
      }),
    },
    agentsById,
  };
}

function selectedAgentPath(snapshot: AppSnapshot, conversationId: string): string[] {
  if (snapshot.selectedConversationId !== conversationId || !snapshot.selectedAgentId) return [];
  const path: string[] = [];
  const seen = new Set<string>();
  let current: AgentSnapshot | undefined = snapshot.agentsById[snapshot.selectedAgentId];
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    path.unshift(current.id);
    current = current.parentId ? snapshot.agentsById[current.parentId] : undefined;
  }
  return path;
}

function uniqueAgentIds(ids: readonly string[]): string[] {
  return [...new Set(ids)];
}

function parseSequence(sequence: string | null): bigint | null {
  if (sequence === null || !/^\d+$/.test(sequence)) return null;
  try {
    const parsed = BigInt(sequence);
    return parsed <= MAX_SEQUENCE ? parsed : null;
  } catch {
    return null;
  }
}

function countQueued(
  conversations: Readonly<Record<string, NormalizedConversation>>,
): number {
  return Object.values(conversations).filter(
    (conversation) => !conversation.archived
      && effectiveConversationStatus(conversation) === "queued",
  ).length;
}

function freezeSnapshot(snapshot: AppSnapshot): AppSnapshot {
  return Object.freeze({
    ...snapshot,
    conversationIds: Object.freeze([...snapshot.conversationIds]),
    conversationsById: Object.freeze(snapshot.conversationsById),
    agentsById: Object.freeze(snapshot.agentsById),
    agentWindow: snapshot.agentWindow ? Object.freeze({
      ...snapshot.agentWindow,
      pages: Object.freeze(snapshot.agentWindow.pages.map((page) => Object.freeze([...page]))),
    }) : null,
    conversationVersions: Object.freeze(snapshot.conversationVersions),
  });
}

export function effectiveConversationStatus(
  conversation: Pick<ConversationSummary, "runStatus" | "rollupStatus">,
): EffectiveStatus {
  if (conversation.runStatus === "queued") return "queued";
  switch (conversation.rollupStatus) {
    case "needsAttention": return "waiting";
    case "active": return "running";
    case "failed": return "failed";
    case "interrupted": return "interrupted";
    case "completed": return "completed";
    default: return conversation.runStatus ?? "idle";
  }
}

export function selectVisibleConversations(snapshot: AppSnapshot): ConversationSummary[] {
  return snapshot.conversationIds.flatMap((id) => {
    const conversation = snapshot.conversationsById[id];
    if (!conversation || conversation.archived) return [];
    if (
      snapshot.statusFilter !== "all"
      && effectiveConversationStatus(conversation) !== snapshot.statusFilter
    ) return [];
    return [{
      ...conversation,
      agents: conversation.agentIds.flatMap((agentId) => {
        const agent = snapshot.agentsById[agentId];
        return agent ? [agent] : [];
      }),
    }];
  });
}

export const AppStoreContext = createContext<AppStore | null>(null);

export function useAppStore<T>(selector: (snapshot: AppSnapshot) => T): T {
  const store = useContext(AppStoreContext);
  if (!store) throw new Error("useAppStore must be used within AppStoreContext.Provider.");
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  return selector(snapshot);
}
