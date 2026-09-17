import { createContext, useContext, useSyncExternalStore } from "react";
import type {
  ConversationPath,
  RunStatus,
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
  DiagnosticsPage,
  ThinkingPreference,
  ContextBudget,
} from "../bridge/types";

const PAGE_SIZE = 200;
const REFRESH_BATCH_SIZE = 8;
const CHILD_PAGE_SIZE = 20;
const MAX_RETAINED_CHILDREN = 80;
const MAX_PAGES = 10_000;
const MAX_SEQUENCE = (1n << 64n) - 1n;

export type EffectiveStatus = "idle" | RunStatus;
export type StatusFilter = "all" | EffectiveStatus;

export type AppApi = {
  getBootstrap(): Promise<BootstrapSnapshot>;
  listConversations(request: { cursor: string | null; limit: number }): Promise<ConversationPage>;
  loadConversation(request: { conversationId: string }): Promise<ConversationSummary>;
  listChildConversations(request: { parentId: string; cursor: string | null; limit: number }): Promise<ConversationPage>;
  loadConversationPath(request: { conversationId: string }): Promise<ConversationPath>;
  listenToAppEvents(handler: (event: AppEvent) => void): Promise<() => void>;
  loadTimeline(request: { conversationId: string; cursor: string | null; limit: number }): Promise<TimelinePage>;
  loadDiagnostics(request: { conversationId: string; cursor: string | null; limit: number }): Promise<DiagnosticsPage>;
  loadEventDetail(request: { eventId: string }): Promise<EventDetailSnapshot>;
  loadApprovals(request: { conversationId: string; cursor: string | null; limit: number; kind: "pending" | "history" }): Promise<ApprovalPage>;
  loadApprovalDetail(request: { approvalId: string }): Promise<ApprovalDetailSnapshot>;
  loadApprovalQuestions(request: { approvalId: string; cursor: string | null; limit: number }): Promise<ApprovalQuestionPage>;
  submitMessage(request: { conversationId: string; text: string; providerOverride: ProviderId | null; commandId: string; thinking: ThinkingPreference; contextBudget: ContextBudget }): Promise<SubmissionSnapshot>;
  setThinkingPreference(request: { conversationId: string; preference: ThinkingPreference }): Promise<void>;
  setContextBudget(request: { conversationId: string; preference: ContextBudget }): Promise<void>;
  steerRun(request: { conversationId: string; runId: string; text: string }): Promise<void>;
  respondToApproval(request: RespondToApprovalRequest): Promise<void>;
  interruptRun(request: { conversationId: string; runId: string }): Promise<void>;
  inspectWorkspace(request: { conversationId: string }): Promise<InspectorSnapshot>;
  inspectProject(request: { path: string }): Promise<ProjectPathSnapshot>;
  pickProjectDirectory(): Promise<string | null>;
  createConversation(request: CreateConversationRequest): Promise<ConversationSummary>;
  archiveConversation(request: { conversationId: string }): Promise<void>;
  listRunAudits(request: { conversationId: string; cursor: string | null; limit: number }): Promise<RunAuditPage>;
  loadRunAudit(request: { conversationId: string; runId: string }): Promise<RunAuditDetailSnapshot>;
};

export type ConversationActions = Pick<
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
>;

export type AppActions = ConversationActions & Pick<AppApi, "listRunAudits" | "loadRunAudit">;

type DraftSubmission = Readonly<{
  pending: boolean;
  providerOverride: ProviderId | null;
  command: Readonly<{ id: string; text: string; provider: ProviderId | null; thinking: ThinkingPreference; contextBudget: ContextBudget }> | null;
  error: string | null;
}>;

export type ChildPageSnapshot = Readonly<{
  pages: readonly Readonly<{ cursor: string | null; ids: readonly string[]; order: number }>[];
  nextCursor: string | null;
  loading: boolean;
  error: string | null;
  evicted: boolean;
}>;

export type ComposerView = Readonly<{ choice: "auto" | ProviderId; preview: boolean }>;
export type SelectedPath = Readonly<{ ids: readonly string[]; truncated: boolean }>;

export type AppSnapshot = Readonly<{
  phase: "idle" | "loading" | "ready" | "error";
  error: string | null;
  bootstrap: BootstrapSnapshot | null;
  conversationsById: Readonly<Record<string, ConversationSummary>>;
  conversationIds: readonly string[];
  childPagesById: Readonly<Record<string, ChildPageSnapshot>>;
  expandedById: Readonly<Record<string, boolean>>;
  executionOwners: Readonly<Record<string, string>>;
  selectedPath: SelectedPath | null;
  selectedConversationId: string | null;
  statusFilter: StatusFilter;
  queuedCount: number;
  lastSequence: string | null;
  conversationVersions: Readonly<Record<string, number>>;
  draftsById: Readonly<Record<string, Readonly<{ text: string }>>>;
  composerViewsById: Readonly<Record<string, ComposerView>>;
  submissionsById: Readonly<Record<string, DraftSubmission>>;
  thinkingSavesById: Readonly<Record<string, Readonly<{ pending: boolean; preference: ThinkingPreference | null; error: string | null }>>>;
  contextBudgetSavesById: Readonly<Record<string, Readonly<{ pending: boolean; preference: ContextBudget | null; error: string | null }>>>;
}>;

export type AppStore = {
  getSnapshot(): AppSnapshot;
  subscribe(listener: () => void): () => void;
  initialize(): Promise<void>;
  retry(): Promise<void>;
  dispose(): void;
  loadChildPage(parentId: string, restart?: boolean): Promise<void>;
  toggleConversation(conversationId: string): void;
  createConversation(request: CreateConversationRequest): Promise<void>;
  archiveConversation(conversationId: string): Promise<void>;
  inspectProject(path: string): Promise<ProjectPathSnapshot>;
  pickProjectDirectory(): Promise<string | null>;
  selectConversation(conversationId: string): Promise<void>;
  setComposerView(conversationId: string, patch: Partial<ComposerView>): void;
  setStatusFilter(filter: StatusFilter): void;
  refreshConversation(conversationId: string): void;
  setDraft(conversationId: string, text: string): void;
  resetDraftCommand(conversationId: string): void;
  setThinkingPreference(conversationId: string, preference: ThinkingPreference): Promise<void>;
  setContextBudget(conversationId: string, preference: ContextBudget): Promise<void>;
  submitDraft(conversationId: string, providerOverride: ProviderId | null, runId: string | null): Promise<boolean>;
  readonly actions: AppActions;
};

const emptySnapshot: AppSnapshot = freezeSnapshot({
  phase: "idle",
  error: null,
  bootstrap: null,
  conversationsById: {},
  conversationIds: [],
  childPagesById: {},
  expandedById: {},
  executionOwners: {},
  selectedPath: null,
  selectedConversationId: null,
  statusFilter: "all",
  queuedCount: 0,
  lastSequence: null,
  conversationVersions: {},
  draftsById: {},
  composerViewsById: {},
  submissionsById: {},
  thinkingSavesById: {},
  contextBudgetSavesById: {},
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
  let childLoadPromise: Promise<void> | null = null;
  const pendingChildLoads = new Map<string, boolean>();
  const childGenerations = new Map<string, number>();
  let pageOrder = 0;
  let selectionGeneration = 0;
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

  function setDraft(conversationId: string, text: string) {
    if (disposed || (snapshot.draftsById[conversationId]?.text ?? "") === text) return;
    const draftsById = { ...snapshot.draftsById };
    if (text) draftsById[conversationId] = Object.freeze({ text });
    else delete draftsById[conversationId];
    update({ draftsById });
    resetDraftCommand(conversationId);
  }

  function setSubmission(conversationId: string, submission: DraftSubmission | null) {
    if (disposed) return;
    const submissionsById = { ...snapshot.submissionsById };
    if (submission) submissionsById[conversationId] = Object.freeze(submission);
    else delete submissionsById[conversationId];
    update({ submissionsById });
  }

  function resetDraftCommand(conversationId: string) {
    const submission = snapshot.submissionsById[conversationId];
    if (submission?.command && !submission.pending) {
      setSubmission(conversationId, { ...submission, command: null });
    }
  }

  async function setThinkingPreference(conversationId: string, preference: ThinkingPreference) {
    if (disposed || snapshot.thinkingSavesById[conversationId]?.pending) return;
    const setSave = (pending: boolean, error: string | null) => {
      if (!disposed) update({ thinkingSavesById: { ...snapshot.thinkingSavesById,
        [conversationId]: Object.freeze({ pending, preference: pending ? preference : null, error }) } });
    };
    const conversation = snapshot.conversationsById[conversationId];
    if (!conversation?.capabilities.canSend || conversation.archived || conversation.parentId !== null) {
      setSave(false, "This conversation is read-only.");
      return;
    }
    setSave(true, null);
    try {
      await api.setThinkingPreference({ conversationId, preference });
      if (disposed) return;
      // Only patch the acknowledged preference into the current summary, never a captured run.
      const current = snapshot.conversationsById[conversationId];
      if (current) {
        eventRevision += 1;
        requestConversationRefresh(conversationId);
        update({ conversationsById: { ...snapshot.conversationsById,
          [conversationId]: Object.freeze({ ...current, thinkingPreference: Object.freeze({ ...preference }) }) } });
      }
      setSave(false, null);
    } catch (reason) {
      setSave(false, reason instanceof Error ? reason.message : "Thinking could not be saved.");
    }
  }

  async function setContextBudget(conversationId: string, preference: ContextBudget) {
    if (disposed || snapshot.contextBudgetSavesById[conversationId]?.pending) return;
    const setSave = (pending: boolean, error: string | null) => {
      if (!disposed) update({ contextBudgetSavesById: { ...snapshot.contextBudgetSavesById,
        [conversationId]: Object.freeze({ pending, preference: pending ? preference : null, error }) } });
    };
    const conversation = snapshot.conversationsById[conversationId];
    if (!conversation?.capabilities.canSend || conversation.archived || conversation.parentId !== null) {
      setSave(false, "This conversation is read-only.");
      return;
    }
    setSave(true, null);
    try {
      await api.setContextBudget({ conversationId, preference });
      if (disposed) return;
      const current = snapshot.conversationsById[conversationId];
      if (current) {
        eventRevision += 1;
        requestConversationRefresh(conversationId);
        update({ conversationsById: { ...snapshot.conversationsById,
          [conversationId]: Object.freeze({ ...current, contextBudget: Object.freeze({ ...preference }) }) } });
      }
      setSave(false, null);
    } catch (reason) {
      setSave(false, reason instanceof Error ? reason.message : "Context budget could not be saved.");
    }
  }

  async function submitDraft(conversationId: string, providerOverride: ProviderId | null, runId: string | null) {
    const draft = snapshot.draftsById[conversationId];
    const previous = snapshot.submissionsById[conversationId];
    const conversation = snapshot.conversationsById[conversationId];
    if (disposed || !conversation?.capabilities.canSend || !draft?.text.trim() || previous?.pending
      || snapshot.thinkingSavesById[conversationId]?.pending || snapshot.contextBudgetSavesById[conversationId]?.pending) return false;
    const command = runId ? null : previous?.command
      && previous.command.text === draft.text
      && previous.command.provider === providerOverride
      ? previous.command
      : Object.freeze({ id: globalThis.crypto.randomUUID(), text: draft.text, provider: providerOverride,
        thinking: Object.freeze({ ...conversation.thinkingPreference }),
        contextBudget: Object.freeze({ ...conversation.contextBudget }) });
    setSubmission(conversationId, { pending: true, providerOverride, command, error: null });
    try {
      if (runId) await api.steerRun({ conversationId, runId, text: draft.text });
      else if (command) await api.submitMessage({
        conversationId,
        text: command.text,
        providerOverride: command.provider,
        commandId: command.id,
        thinking: command.thinking,
        contextBudget: command.contextBudget,
      });
      if (disposed) return false;
      // Identity protects even an edit away from and back to the submitted text.
      if (snapshot.draftsById[conversationId] === draft) setDraft(conversationId, "");
      setSubmission(conversationId, null);
      return true;
    } catch (reason) {
      if (disposed) return false;
      const ambiguous = reason instanceof Error && "code" in reason && reason.code === "outcome-unknown";
      setSubmission(conversationId, snapshot.draftsById[conversationId] === draft ? {
        pending: false,
        providerOverride,
        command: ambiguous ? command : null,
        error: reason instanceof Error ? reason.message : "Prompting Time could not submit this request.",
      } : null);
      return false;
    }
  }

  async function synchronize() {
    fullRefreshEpoch += 1;
    const epoch = fullRefreshEpoch;
    const revisionAtStart = eventRevision;
    const [bootstrap, conversations] = await Promise.all([
      api.getBootstrap().then((bootstrap) => {
        // Startup diagnostics remain useful even if conversation loading fails.
        if (!disposed && epoch === fullRefreshEpoch && revisionAtStart === eventRevision) {
          update({ bootstrap });
        }
        return bootstrap;
      }),
      loadAllConversations(api),
    ]);
    const activeConversations = conversations.filter(({ archived, parentId }) => !archived && parentId === null);
    if (disposed) return;
    if (revisionAtStart !== eventRevision) {
      refreshRequested = true;
      return;
    }

    const conversationIds = activeConversations.map(({ id }) => id);
    const rootIds = new Set(conversationIds);
    const normalized = normalizeConversations(activeConversations);
    const executionOwners = Object.fromEntries(conversationIds.map(id => [id, id]));
    for (const conversation of Object.values(snapshot.conversationsById)) {
      const owner = snapshot.executionOwners[conversation.id];
      if (conversation.parentId !== null && owner && rootIds.has(owner)) {
        normalized[conversation.id] = conversation;
        executionOwners[conversation.id] = owner;
      }
    }
    const selectedConversationId = snapshot.selectedConversationId && normalized[snapshot.selectedConversationId]
      ? snapshot.selectedConversationId : conversationIds[0] ?? null;
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
      executionOwners,
      selectedConversationId,
      selectedPath: selectedConversationId === snapshot.selectedConversationId ? snapshot.selectedPath : null,
      queuedCount: countQueued(normalized),
      conversationVersions,
    });
    pruneNavigation();
    if (snapshot.selectedConversationId && snapshot.conversationsById[snapshot.selectedConversationId]?.parentId !== null) {
      requestConversationRefresh(snapshot.selectedConversationId);
    }
    for (const id of Object.keys(snapshot.childPagesById)) {
      if (snapshot.expandedById[id] && isVisibleConversation(snapshot, id)) void loadChildPage(id, true);
    }
    revealSelectedRoot();
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
      const ids = [...pendingConversationRefreshes].slice(0, REFRESH_BATCH_SIZE);
      const versions = new Map(ids.map((id) => [id, targetedConversationVersions.get(id) ?? 0]));
      ids.forEach((id) => pendingConversationRefreshes.delete(id));
      const epoch = fullRefreshEpoch;
      let refreshed: Array<{ id: string; conversation: ConversationSummary }>;
      try {
        refreshed = await Promise.all(ids.map(async (id) => {
          const conversation = await api.loadConversation({ conversationId: id });
          return { id, conversation };
        }));
      } catch {
        void requestRefresh();
        return;
      }
      if (disposed || epoch !== fullRefreshEpoch) continue;
      refreshed.forEach((result) => {
        if (versions.get(result.id) !== targetedConversationVersions.get(result.id)) return;
        mergeConversation(result.conversation);
      });
    }
  }

  function mergeConversation(conversation: ConversationSummary) {
    if (conversation.archived) {
      removeConversation(conversation.id);
      return;
    }
    const owner = conversation.parentId === null ? conversation.id : snapshot.executionOwners[conversation.id];
    if (!owner || (conversation.parentId !== null && !snapshot.conversationsById[owner])) return;
    const conversationsById = { ...snapshot.conversationsById, [conversation.id]: Object.freeze(conversation) };
    publish({
      ...snapshot,
      conversationsById,
      executionOwners: { ...snapshot.executionOwners, [conversation.id]: owner },
      conversationIds: conversation.parentId === null
        ? [conversation.id, ...snapshot.conversationIds.filter(id => id !== conversation.id)] : snapshot.conversationIds,
      selectedConversationId: snapshot.selectedConversationId ?? conversation.id,
      queuedCount: countQueued(conversationsById),
      conversationVersions: {
        ...snapshot.conversationVersions,
        [conversation.id]: (snapshot.conversationVersions[conversation.id] ?? 0) + 1,
      },
    });
    if (snapshot.expandedById[conversation.id] && isVisibleConversation(snapshot, conversation.id)
      && (conversation.hasChildren || snapshot.childPagesById[conversation.id])) {
      void loadChildPage(conversation.id, true);
    }
    revealSelectedRoot();
  }

  function removeConversation(conversationId: string) {
    const removedIds = new Set(Object.keys(snapshot.conversationsById).filter(id =>
      id === conversationId || snapshot.executionOwners[id] === conversationId));
    if (!removedIds.size) return;
    eventRevision += 1;
    selectionGeneration += 1;
    const conversationsById = { ...snapshot.conversationsById };
    const childPagesById = { ...snapshot.childPagesById };
    const executionOwners = { ...snapshot.executionOwners };
    const expandedById = { ...snapshot.expandedById };
    const conversationVersions = { ...snapshot.conversationVersions };
    for (const id of removedIds) {
      delete conversationsById[id];
      delete childPagesById[id];
      delete executionOwners[id];
      delete expandedById[id];
      delete conversationVersions[id];
      childGenerations.set(id, (childGenerations.get(id) ?? 0) + 1);
      pendingChildLoads.delete(id);
      targetedConversationVersions.delete(id);
      pendingConversationRefreshes.delete(id);
    }
    const conversationIds = snapshot.conversationIds.filter(id => !removedIds.has(id));
    const selectionRemoved = removedIds.has(snapshot.selectedConversationId ?? "");
    publish({
      ...snapshot, conversationsById, conversationIds, childPagesById, executionOwners, expandedById, conversationVersions,
      selectedConversationId: selectionRemoved ? conversationIds[0] ?? null : snapshot.selectedConversationId,
      selectedPath: selectionRemoved ? null : snapshot.selectedPath,
      queuedCount: countQueued(conversationsById),
    });
  }

  async function createConversation(request: CreateConversationRequest) {
    const conversation = await api.createConversation(request);
    if (disposed) return;
    if (conversation.archived || conversation.parentId !== null) throw new Error("Prompting Time created an unavailable conversation.");
    mergeConversation(conversation);
    await selectConversation(conversation.id);
  }

  async function archiveConversation(conversationId: string) {
    if (!snapshot.conversationsById[conversationId]?.capabilities.canArchive) return;
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
      for (const conversation of Object.values(snapshot.conversationsById)) {
        if (conversation.id === event.conversationId || snapshot.executionOwners[conversation.id] !== event.conversationId) continue;
        if (event.kind === "runChanged" && conversation.currentRunId !== event.runId) continue;
        if (conversation.id === snapshot.selectedConversationId || isVisibleConversation(snapshot, conversation.id)) {
          requestConversationRefresh(conversation.id);
        }
      }
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

  function revealSelectedRoot() {
    const id = snapshot.selectedConversationId;
    const conversation = id ? snapshot.conversationsById[id] : undefined;
    if (!id || !conversation || conversation.parentId !== null || !conversation.hasChildren
      || Object.hasOwn(snapshot.expandedById, id)) return;
    update({ expandedById: { ...snapshot.expandedById, [id]: true } });
    void loadChildPage(id, true);
  }

  function toggleConversation(conversationId: string) {
    const conversation = snapshot.conversationsById[conversationId];
    if (!conversation) return;
    const open = !snapshot.expandedById[conversationId];
    update({ expandedById: { ...snapshot.expandedById, [conversationId]: open } });
    if (open) void loadChildPage(conversationId, true);
  }

  async function selectConversation(conversationId: string) {
    const generation = ++selectionGeneration;
    let path = loadedConversationPath(snapshot, conversationId);
    if (path.truncated || !snapshot.conversationsById[conversationId]) {
      const epoch = fullRefreshEpoch;
      try {
        const result = await api.loadConversationPath({ conversationId });
        if (disposed || generation !== selectionGeneration || epoch !== fullRefreshEpoch) return;
        if (!result.items.some(item => item.id === conversationId) || !snapshot.conversationsById[result.ownerConversationId]) return;
        const conversationsById = { ...snapshot.conversationsById, ...normalizeConversations(result.items) };
        const executionOwners = { ...snapshot.executionOwners };
        result.items.forEach(item => { executionOwners[item.id] = result.ownerConversationId; });
        update({ conversationsById, executionOwners });
        path = { ids: result.items.map(item => item.id), truncated: result.truncated };
      } catch (reason) {
        if (generation === selectionGeneration && epoch === fullRefreshEpoch && !disposed) {
          update({ error: reason instanceof Error ? reason.message : "Conversation unavailable." });
        }
        return;
      }
    }
    const expandedById = { ...snapshot.expandedById };
    path.ids.slice(0, -1).forEach(id => { expandedById[id] = true; });
    update({ selectedConversationId: conversationId, selectedPath: path, expandedById, error: null });
    pruneNavigation();
    revealSelectedRoot();
    for (const id of path.ids.slice(0, -1)) {
      if (snapshot.conversationsById[id]?.hasChildren && !snapshot.childPagesById[id]
        && isVisibleConversation(snapshot, id)) void loadChildPage(id, true);
    }
  }

  function setComposerView(conversationId: string, patch: Partial<ComposerView>) {
    if (disposed) return;
    const current = snapshot.composerViewsById[conversationId] ?? { choice: "auto", preview: false };
    const next = { ...current, ...patch };
    if (next.choice === current.choice && next.preview === current.preview) return;
    update({ composerViewsById: { ...snapshot.composerViewsById, [conversationId]: Object.freeze(next) } });
  }

  function loadChildPage(parentId: string, restart = false): Promise<void> {
    if (disposed || !snapshot.conversationsById[parentId]) return Promise.resolve();
    if (!pendingChildLoads.has(parentId) || restart) pendingChildLoads.set(parentId, restart);
    if (childLoadPromise) return childLoadPromise;
    childLoadPromise = (async () => {
      while (pendingChildLoads.size > 0 && !disposed) {
        const [id, resets] = pendingChildLoads.entries().next().value!;
        pendingChildLoads.delete(id);
        await performChildPage(id, resets);
      }
    })().finally(() => { childLoadPromise = null; });
    return childLoadPromise;
  }

  async function performChildPage(parentId: string, restart: boolean) {
    const parent = snapshot.conversationsById[parentId];
    if (!parent || parent.archived) return;
    const previous = snapshot.childPagesById[parentId];
    if (!restart && previous && (previous.loading || previous.nextCursor === null)) return;
    let cursor = restart ? null : previous?.nextCursor ?? null;
    const generation = (childGenerations.get(parentId) ?? 0) + 1;
    childGenerations.set(parentId, generation);
    const epoch = fullRefreshEpoch;
    const window: ChildPageSnapshot = {
      pages: previous?.pages ?? [], nextCursor: previous?.nextCursor ?? null,
      loading: true, error: null, evicted: previous?.evicted ?? false,
    };
    update({ childPagesById: { ...snapshot.childPagesById, [parentId]: window } });
    try {
      const previousIds = new Set(previous?.pages.flatMap(page => page.ids));
      const targetCount = restart ? Math.min(previousIds.size, MAX_RETAINED_CHILDREN) : 0;
      const items: ConversationSummary[] = [];
      const refreshedPages: ChildPageSnapshot["pages"][number][] = [];
      const seenCursors = new Set<string>();
      // Refresh the visible window atomically from the newest cursor. Reusing an old
      // later-page cursor after discovering new children would leave a silent gap.
      do {
        const page = await api.listChildConversations({ parentId, cursor, limit: CHILD_PAGE_SIZE });
        if (disposed || epoch !== fullRefreshEpoch || generation !== childGenerations.get(parentId)
          || !snapshot.conversationsById[parentId]) return;
        if (page.items.some(item => item.parentId !== parentId || item.archived)) throw new Error("Child conversation ownership changed while loading.");
        if (page.nextCursor !== null && (page.nextCursor === cursor || seenCursors.has(page.nextCursor))) {
          throw new Error("Conversation pagination did not advance.");
        }
        if (page.nextCursor !== null) seenCursors.add(page.nextCursor);
        items.push(...page.items);
        refreshedPages.push({ cursor, ids: page.items.map(item => item.id), order: ++pageOrder });
        cursor = page.nextCursor;
        if (refreshedPages.length >= MAX_RETAINED_CHILDREN && cursor !== null && items.length < targetCount) {
          throw new Error("Conversation pagination exceeded its safety bound.");
        }
      } while (cursor !== null && items.length < targetCount);
      const latest = snapshot.childPagesById[parentId]!;
      const pages = restart ? refreshedPages : [...latest.pages, ...refreshedPages];
      const executionOwners = { ...snapshot.executionOwners };
      items.forEach(item => { executionOwners[item.id] = snapshot.executionOwners[parentId] ?? parentId; });
      const refreshedIds = new Set(items.map(item => item.id));
      const displaced = restart && [...previousIds].some(id => !refreshedIds.has(id));
      // Only a fresh scan through the end proves there are no earlier cache gaps.
      const recovered = restart && cursor === null;
      update({
        conversationsById: { ...snapshot.conversationsById, ...normalizeConversations(items) },
        executionOwners,
        childPagesById: { ...snapshot.childPagesById, [parentId]: {
          pages, nextCursor: cursor, loading: false, error: null, evicted: !recovered && (latest.evicted || displaced),
        } },
      });
      pruneNavigation();
    } catch (reason) {
      if (disposed || epoch !== fullRefreshEpoch || generation !== childGenerations.get(parentId)
        || !snapshot.childPagesById[parentId]) return;
      update({ childPagesById: { ...snapshot.childPagesById, [parentId]: {
        ...snapshot.childPagesById[parentId]!, loading: false,
        error: reason instanceof Error ? reason.message : "Prompting Time could not load conversations.",
      } } });
    }
  }

  function pruneNavigation() {
    const childPagesById = { ...snapshot.childPagesById };
    const pinned = new Set(snapshot.selectedPath?.ids ?? []);
    if (snapshot.selectedConversationId) pinned.add(snapshot.selectedConversationId);
    const retained = () => {
      const ids = new Set([...snapshot.conversationIds, ...pinned]);
      const addPath = (id: string) => {
        const seen = new Set<string>();
        let current: string | null = id;
        while (current && !seen.has(current) && snapshot.conversationsById[current]) {
          seen.add(current); ids.add(current);
          current = snapshot.conversationsById[current]!.parentId;
        }
      };
      Object.entries(childPagesById).forEach(([id, window]) => {
        if (!snapshot.conversationsById[id] || !snapshot.conversationsById[snapshot.executionOwners[id] ?? id]) return;
        if (window.pages.length) addPath(id);
        window.pages.forEach(page => page.ids.forEach(addPath));
      });
      return ids;
    };
    let ids = retained();
    const candidates = Object.entries(childPagesById).flatMap(([parentId, window]) =>
      window.pages.map(page => ({ parentId, page }))).sort((left, right) =>
      Number(Boolean(snapshot.expandedById[left.parentId])) - Number(Boolean(snapshot.expandedById[right.parentId]))
      || left.page.order - right.page.order);
    for (const { parentId, page } of candidates) {
      if ([...ids].filter(id => snapshot.conversationsById[id]?.parentId !== null).length <= MAX_RETAINED_CHILDREN) break;
      const window = childPagesById[parentId]!;
      childPagesById[parentId] = { ...window, pages: window.pages.filter(item => item !== page), evicted: true };
      ids = retained();
    }
    const conversationsById = Object.fromEntries([...ids].flatMap(id => {
      const conversation = snapshot.conversationsById[id];
      return conversation ? [[id, conversation]] : [];
    }));
    const executionOwners = Object.fromEntries([...ids].map(id => [id, snapshot.executionOwners[id] ?? id]));
    const expandedById = Object.fromEntries(Object.entries(snapshot.expandedById).filter(([id]) => ids.has(id)));
    for (const id of Object.keys(childPagesById)) {
      if (ids.has(id)) continue;
      delete childPagesById[id];
      childGenerations.set(id, (childGenerations.get(id) ?? 0) + 1);
      pendingChildLoads.delete(id);
    }
    update({ conversationsById, childPagesById, executionOwners, expandedById });
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
    setDraft,
    resetDraftCommand,
    setThinkingPreference,
    setContextBudget,
    submitDraft,
    loadChildPage,
    toggleConversation,
    selectConversation,
    setComposerView,
    createConversation,
    archiveConversation,
    inspectProject: (path) => api.inspectProject({ path }),
    pickProjectDirectory: () => api.pickProjectDirectory(),
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
      update({ draftsById: {}, composerViewsById: {}, submissionsById: {}, thinkingSavesById: {}, contextBudgetSavesById: {} });
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
      loadDiagnostics: api.loadDiagnostics,
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
    Object.keys(snapshot.conversationsById).map((id) => [id, (snapshot.conversationVersions[id] ?? 0) + 1]),
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

function normalizeConversations(conversations: readonly ConversationSummary[]): Record<string, ConversationSummary> {
  return Object.fromEntries(conversations.map(conversation => [conversation.id, Object.freeze({ ...conversation })]));
}

export function loadedConversationPath(snapshot: AppSnapshot, id: string): SelectedPath {
  const ids: string[] = [];
  const seen = new Set<string>();
  let current: string | null = id;
  while (current !== null) {
    if (ids.length === 64) return { ids: ids.reverse(), truncated: true };
    const conversation: ConversationSummary | undefined = snapshot.conversationsById[current];
    if (!conversation || seen.has(current)) return { ids: ids.reverse(), truncated: true };
    seen.add(current);
    ids.push(current);
    current = conversation.parentId;
  }
  return { ids: ids.reverse(), truncated: false };
}

function isVisibleConversation(snapshot: AppSnapshot, id: string): boolean {
  const path = loadedConversationPath(snapshot, id);
  return !path.truncated && path.ids.slice(0, -1).every(parentId => snapshot.expandedById[parentId]);
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
  conversations: Readonly<Record<string, ConversationSummary>>,
): number {
  return Object.values(conversations).filter(
    (conversation) => !conversation.archived && conversation.parentId === null
      && effectiveConversationStatus(conversation) === "queued",
  ).length;
}

function freezeSnapshot(snapshot: AppSnapshot): AppSnapshot {
  return Object.freeze({
    ...snapshot,
    conversationIds: Object.freeze([...snapshot.conversationIds]),
    conversationsById: Object.freeze(snapshot.conversationsById),
    childPagesById: Object.freeze(Object.fromEntries(Object.entries(snapshot.childPagesById).map(([id, window]) => [id, Object.freeze({
      ...window, pages: Object.freeze(window.pages.map(page => Object.freeze({ ...page, ids: Object.freeze([...page.ids]) }))),
    })]))),
    expandedById: Object.freeze(snapshot.expandedById),
    executionOwners: Object.freeze(snapshot.executionOwners),
    selectedPath: snapshot.selectedPath ? Object.freeze({ ...snapshot.selectedPath, ids: Object.freeze([...snapshot.selectedPath.ids]) }) : null,
    composerViewsById: Object.freeze(snapshot.composerViewsById),
    conversationVersions: Object.freeze(snapshot.conversationVersions),
    draftsById: Object.freeze(snapshot.draftsById),
    submissionsById: Object.freeze(snapshot.submissionsById),
    thinkingSavesById: Object.freeze(snapshot.thinkingSavesById),
    contextBudgetSavesById: Object.freeze(snapshot.contextBudgetSavesById),
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
    return [conversation];
  });
}

export const AppStoreContext = createContext<AppStore | null>(null);

export function useAppStore<T>(selector: (snapshot: AppSnapshot) => T): T {
  const store = useContext(AppStoreContext);
  if (!store) throw new Error("useAppStore must be used within AppStoreContext.Provider.");
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  return selector(snapshot);
}
