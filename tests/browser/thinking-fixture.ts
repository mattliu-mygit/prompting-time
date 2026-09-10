import type { AppApi } from "../../src/app/store";
import type { AppEvent, ConversationSummary, ThinkingConfiguration, ThinkingDecision, ThinkingPreference } from "../../src/bridge/types";

// Invented browser-memory settings only; this fixture does not run the Rust policy.
export const thinkingScenario = new URLSearchParams(location.search).get("thinking");
const preferences = new Map<string, ThinkingPreference>();
const decisions = new Map<string, ThinkingDecision>();
const configurations = new Map<string, ThinkingConfiguration>();
const providers = new Map<string, "codex" | "claude">();
const saves: Array<{ conversationId: string; preference: ThinkingPreference }> = [];
const submissions: Parameters<AppApi["submitMessage"]>[0][] = [];
const listeners = new Set<(event: AppEvent) => void>();
let sequence = 9000;
let rejectSave = false;

export function failNextThinkingSave() { rejectSave = true; }
export function thinkingFixtureStats() { return { saves: [...saves], submissions: [...submissions] }; }

function changed(conversationId: string) {
  listeners.forEach(listener => listener({ kind: "conversationChanged", sequence: String(++sequence), conversationId }));
}

export function withThinkingFixture(base: AppApi): AppApi {
  function decorate(conversation: ConversationSummary): ConversationSummary {
    if (conversation.parentId !== null) return conversation;
    return {
      ...conversation,
      thinkingPreference: preferences.get(conversation.id) ?? { kind: "auto" },
      thinkingDecision: decisions.get(conversation.id) ?? null,
      thinkingConfiguration: configurations.get(conversation.id) ?? null,
      provider: providers.get(conversation.id) ?? conversation.provider,
    };
  }
  return {
    ...base,
    getBootstrap: async () => {
      const bootstrap = await base.getBootstrap();
      return { ...bootstrap, providers: bootstrap.providers.map(provider => ({ ...provider, available: true, installed: true, version: "synthetic", diagnostic: null })) };
    },
    listConversations: async request => {
      const page = await base.listConversations(request);
      return { ...page, items: page.items.map(decorate) };
    },
    loadConversation: async request => decorate(await base.loadConversation(request)),
    loadConversationPath: async request => {
      const path = await base.loadConversationPath(request);
      return { ...path, items: path.items.map(decorate) };
    },
    listenToAppEvents: async listener => {
      listeners.add(listener);
      const unsubscribe = await base.listenToAppEvents(listener);
      return () => { listeners.delete(listener); unsubscribe(); };
    },
    setThinkingPreference: async request => {
      const conversation = await base.loadConversation({ conversationId: request.conversationId });
      if (!conversation.capabilities.canSend) throw new Error("Recorded children have no editable thinking preference.");
      saves.push(structuredClone(request));
      if (rejectSave) {
        rejectSave = false;
        throw new Error("Synthetic settings save failed. Your previous preference is unchanged.");
      }
      preferences.set(request.conversationId, structuredClone(request.preference));
      changed(request.conversationId);
    },
    submitMessage: async request => {
      const result = await base.submitMessage(request);
      submissions.push(structuredClone(request));
      const preference = request.thinking ?? { kind: "providerDefault" };
      const requestedEffort = preference.kind === "manual" ? preference.level : preference.kind === "auto" ? "high" : null;
      const decision: ThinkingDecision = { preference, requestedEffort, reason: "Synthetic decision for browser acceptance; no model or Rust policy was invoked." };
      const provider = request.providerOverride ?? "codex";
      decisions.set(request.conversationId, decision);
      providers.set(request.conversationId, provider);
      configurations.set(request.conversationId, {
        model: `synthetic-${provider}-model`,
        supportedEfforts: provider === "codex" ? ["low", "medium", "high", "xhigh"] : ["low", "medium", "high", "max"],
        configuredEffort: requestedEffort ?? "medium",
      });
      changed(request.conversationId);
      return { ...result, provider, thinkingDecision: decision };
    },
  };
}
