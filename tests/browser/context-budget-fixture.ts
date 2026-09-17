import type { AppApi } from "../../src/app/store";
import type { AppEvent, ContextBudget, ConversationSummary, TimelineItem } from "../../src/bridge/types";

// Invented, browser-memory preferences. No tokens, native settings, or providers are invoked.
export const contextBudgetScenario = new URLSearchParams(location.search).get("context");
const preferences = new Map<string, ContextBudget>();
const accepted = new Map<string, ContextBudget>();
const saves: Array<{ conversationId: string; preference: ContextBudget }> = [];
const submissions: Parameters<AppApi["submitMessage"]>[0][] = [];
const listeners = new Set<(event: AppEvent) => void>();
const pendingSaves: Array<() => void> = [];
const compacting = new Set<string>();
const boundaries = new Set<string>();
const completions: TimelineItem[] = [];
let sequence = 12000;
let rejectSave = false;
let holdSave = false;

export function failNextContextBudgetSave() { rejectSave = true; }
export function holdNextContextBudgetSave() { holdSave = true; }
export function completeContextBudgetSaves() { pendingSaves.splice(0).forEach(resolve => resolve()); }
export function contextBudgetFixtureStats() { return { saves: [...saves], submissions: [...submissions] }; }

function changed(conversationId: string) {
  listeners.forEach(listener => listener({ kind: "conversationChanged", sequence: String(++sequence), conversationId }));
}

export function beginSyntheticCompaction(conversationId = "conversation-0") {
  compacting.add(conversationId);
  changed(conversationId);
}

export function clearSyntheticCompaction(conversationId = "conversation-0") {
  compacting.delete(conversationId);
  changed(conversationId);
}

export function completeSyntheticCompaction(boundary = "synthetic-boundary", conversationId = "conversation-0") {
  const key = JSON.stringify([conversationId, boundary]);
  if (boundaries.has(key)) return;
  boundaries.add(key);
  compacting.delete(conversationId);
  const owner = conversationId.replace("conversation-", "");
  completions.push({
    id: `synthetic-compaction-${boundaries.size}`, conversationId, runId: `run-${owner}`, agentId: `root-${owner}`,
    sequence: String(3000 + boundaries.size), kind: "lifecycle", role: null, provider: "codex",
    presentation: "normal", content: "Context compacted", operation: null, contentBytes: "17", truncated: false,
  });
  changed(conversationId);
}

export function withContextBudgetFixture(base: AppApi): AppApi {
  function decorate(conversation: ConversationSummary): ConversationSummary {
    if (conversation.parentId !== null) return conversation;
    return {
      ...conversation,
      contextBudget: preferences.get(conversation.id) ?? { kind: "tokens", tokens: 300000 },
      runContextBudget: accepted.get(conversation.id) ?? null,
      ...(compacting.has(conversation.id) ? { runStatus: "running", rollupStatus: "active" } as const : {}),
    };
  }
  return {
    ...base,
    listConversations: async request => {
      const page = await base.listConversations(request);
      return { ...page, items: page.items.map(decorate) };
    },
    loadConversation: async request => decorate(await base.loadConversation(request)),
    loadTimeline: async request => {
      const page = await base.loadTimeline(request);
      const conversation = await base.loadConversation({ conversationId: request.conversationId });
      const owner = request.conversationId.replace("conversation-", "");
      return {
        ...page,
        items: request.cursor ? page.items : [...page.items, ...completions.filter(item => item.conversationId === request.conversationId)],
        activeCompaction: compacting.has(request.conversationId) && conversation.currentRunId !== null && conversation.provider !== null
          ? { runId: conversation.currentRunId, agentId: `root-${owner}`, provider: conversation.provider } : null,
      };
    },
    loadConversationPath: async request => {
      const path = await base.loadConversationPath(request);
      return { ...path, items: path.items.map(decorate) };
    },
    listenToAppEvents: async listener => {
      listeners.add(listener);
      // This wrapper is the outer fixture layer: keep one ordering across both settings.
      const unsubscribe = await base.listenToAppEvents(event => listener({ ...event, sequence: String(++sequence) }));
      return () => { listeners.delete(listener); unsubscribe(); };
    },
    setContextBudget: async request => {
      const conversation = await base.loadConversation({ conversationId: request.conversationId });
      if (!conversation.capabilities.canSend) throw new Error("Recorded children have no editable context budget.");
      saves.push(structuredClone(request));
      if (holdSave) {
        holdSave = false;
        await new Promise<void>(resolve => pendingSaves.push(resolve));
      }
      if (rejectSave) {
        rejectSave = false;
        throw new Error("Synthetic context budget save failed. Your previous preference is unchanged.");
      }
      preferences.set(request.conversationId, structuredClone(request.preference));
      changed(request.conversationId);
    },
    submitMessage: async request => {
      const result = await base.submitMessage(request);
      submissions.push(structuredClone(request));
      accepted.set(request.conversationId, structuredClone(request.contextBudget ?? { kind: "providerDefault" }));
      changed(request.conversationId);
      return result;
    },
  };
}
