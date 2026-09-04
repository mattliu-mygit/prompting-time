import { createContext, useContext, useSyncExternalStore } from "react";
import type {
  AgentSnapshot,
  AgentStatus,
  AgentTreePage,
  AppEvent,
  BootstrapSnapshot,
  ConversationPage,
  ConversationSummary,
} from "../bridge/types";

const PAGE_SIZE = 200;
const AGENT_LOAD_BATCH_SIZE = 8;
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
};

export type NormalizedConversation = Omit<ConversationSummary, "agents"> & {
  agentIds: readonly string[];
};

export type AppSnapshot = Readonly<{
  phase: "idle" | "loading" | "ready" | "error";
  error: string | null;
  bootstrap: BootstrapSnapshot | null;
  conversationsById: Readonly<Record<string, NormalizedConversation>>;
  conversationIds: readonly string[];
  agentsById: Readonly<Record<string, AgentSnapshot>>;
  selectedConversationId: string | null;
  selectedAgentId: string | null;
  statusFilter: StatusFilter;
  queuedCount: number;
  lastSequence: string | null;
}>;

export type AppStore = {
  getSnapshot(): AppSnapshot;
  subscribe(listener: () => void): () => void;
  initialize(): Promise<void>;
  retry(): Promise<void>;
  dispose(): void;
  selectConversation(conversationId: string, agentId?: string): void;
  setStatusFilter(filter: StatusFilter): void;
};

const emptySnapshot: AppSnapshot = freezeSnapshot({
  phase: "idle",
  error: null,
  bootstrap: null,
  conversationsById: {},
  conversationIds: [],
  agentsById: {},
  selectedConversationId: null,
  selectedAgentId: null,
  statusFilter: "all",
  queuedCount: 0,
  lastSequence: null,
});

export function createAppStore(api: AppApi): AppStore {
  let snapshot = emptySnapshot;
  const listeners = new Set<() => void>();
  let initializePromise: Promise<void> | null = null;
  let refreshPromise: Promise<void> | null = null;
  let refreshRequested = false;
  let fullRefreshEpoch = 0;
  let eventRevision = 0;
  let rolloverExpected = false;
  let targetedRefreshScheduled = false;
  let targetedRefreshPromise: Promise<void> | null = null;
  const targetedConversationVersions = new Map<string, number>();
  const pendingConversationRefreshes = new Set<string>();
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
    const { normalized, agentsById } = await normalizeConversations(api, conversations);
    if (disposed) return;
    if (revisionAtStart !== eventRevision) {
      refreshRequested = true;
      return;
    }

    const conversationIds = conversations.map(({ id }) => id);
    const selectedConversationId = snapshot.selectedConversationId
      ?? conversations.find(({ archived }) => !archived)?.id
      ?? null;
    const selectedConversation = selectedConversationId
      ? normalized[selectedConversationId]
      : null;
    const selectedAgentId = selectedConversation
      ? selectedConversation.agentIds.includes(snapshot.selectedAgentId ?? "")
        ? snapshot.selectedAgentId
        : null
      : snapshot.selectedAgentId;

    publish({
      ...snapshot,
      phase: "ready",
      error: null,
      bootstrap,
      conversationsById: normalized,
      conversationIds,
      agentsById,
      selectedConversationId,
      selectedAgentId,
      queuedCount: countQueued(normalized),
    });
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
          const result = await normalizeConversations(api, [conversation]);
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
    const conversation = normalized[conversationId];
    if (!conversation) return;
    const conversationsById = { ...snapshot.conversationsById, [conversationId]: conversation };
    const agentsById = { ...snapshot.agentsById };
    snapshot.conversationsById[conversationId]?.agentIds.forEach((id) => delete agentsById[id]);
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
      selectedConversationId,
      selectedAgentId,
      queuedCount: countQueued(conversationsById),
    });
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
    update({ lastSequence: nextSequence.toString() });
    if (event.kind === "reloadRequired" || hasGap || snapshot.phase !== "ready") {
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

  return {
    getSnapshot: () => snapshot,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    initialize,
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
      update({
        selectedConversationId: conversationId,
        selectedAgentId: agentId && conversation.agentIds.includes(agentId) ? agentId : null,
      });
    },
    setStatusFilter(statusFilter) {
      if (snapshot.statusFilter !== statusFilter) update({ statusFilter });
    },
  };
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

async function normalizeConversations(
  api: AppApi,
  conversations: ConversationSummary[],
): Promise<{
  normalized: Record<string, NormalizedConversation>;
  agentsById: Record<string, AgentSnapshot>;
}> {
  const loadedAgents = new Map<string, LoadedAgents>();
  const truncated = conversations.filter(
    ({ agentsTruncated, archived }) => agentsTruncated && !archived,
  );
  for (let start = 0; start < truncated.length; start += AGENT_LOAD_BATCH_SIZE) {
    const batch = truncated.slice(start, start + AGENT_LOAD_BATCH_SIZE);
    const results = await Promise.all(batch.map(async (conversation) => ({
      conversation,
      page: await loadAllAgents(api, conversation),
    })));
    results.forEach(({ conversation, page }) => loadedAgents.set(conversation.id, page));
  }

  const normalized: Record<string, NormalizedConversation> = {};
  const agentsById: Record<string, AgentSnapshot> = {};
  conversations.forEach((conversation) => {
    const loaded = loadedAgents.get(conversation.id);
    const agents = loaded?.agents ?? conversation.agents;
    const agentIds: string[] = [];
    agents.forEach((agent) => {
      if (agentsById[agent.id]) return;
      agentsById[agent.id] = Object.freeze({ ...agent });
      agentIds.push(agent.id);
    });
    normalized[conversation.id] = Object.freeze({
      id: conversation.id,
      title: conversation.title,
      workspaceId: conversation.workspaceId,
      archived: conversation.archived,
      projectRoot: conversation.projectRoot,
      currentRunId: conversation.currentRunId,
      provider: conversation.provider,
      runStatus: conversation.runStatus,
      rollupStatus: conversation.rollupStatus,
      agentsTruncated: loaded ? !loaded.complete : conversation.agentsTruncated,
      agentIds: Object.freeze(agentIds),
    });
  });
  return { normalized, agentsById };
}

async function loadAllAgents(
  api: AppApi,
  conversation: ConversationSummary,
): Promise<LoadedAgents> {
  const agents: AgentSnapshot[] = [];
  const seenAgentIds = new Set<string>();
  const seenCursors = new Set<string>();
  let cursor: string | null = null;
  for (let pageCount = 0; pageCount < MAX_PAGES; pageCount += 1) {
    const page = await api.loadAgentTree({
      conversationId: conversation.id,
      cursor,
      limit: PAGE_SIZE,
    });
    if (page.runId !== conversation.currentRunId) {
      return { agents: conversation.agents, complete: false };
    }
    page.items.forEach(({ agent }) => {
      if (!seenAgentIds.has(agent.id)) {
        seenAgentIds.add(agent.id);
        agents.push(agent);
      }
    });
    if (page.nextCursor === null) return { agents, complete: true };
    if (seenCursors.has(page.nextCursor)) throw new Error("Agent pagination did not advance.");
    seenCursors.add(page.nextCursor);
    cursor = page.nextCursor;
  }
  throw new Error("Agent pagination exceeded its safety bound.");
}

type LoadedAgents = {
  agents: readonly AgentSnapshot[];
  complete: boolean;
};

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
