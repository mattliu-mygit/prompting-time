import { createRoot } from "react-dom/client";
import { App } from "../../src/app/App";
import { createAppStore, type AppApi } from "../../src/app/store";
import type { ConversationSummary } from "../../src/bridge/types";
import { chatApproval, chatDiagnostics, chatScenario, chatTimeline, listenToChatEvents } from "./chat-fixture";
import { composerMessages, composerScenario, steerComposerRun, submitComposerMessage } from "./composer-fixture";
import { listenToToolEvents, toolEventDetail, toolProvider, toolScenario, toolTimeline } from "./tool-operations-fixture";
import { archiveSyntheticRoot, listenToTreeEvents, treeConversationPath, treeIds, treeListChildren, treeListConversations, treeLoadConversation, treeScenario, treeTimeline } from "./conversation-tree-fixture";
import { thinkingScenario, withThinkingFixture } from "./thinking-fixture";
import { contextBudgetScenario, withContextBudgetFixture } from "./context-budget-fixture";
import "../../src/styles/tokens.css";
import "../../src/styles/app.css";

// Invented data only. This entry point never uses the native bridge or providers.
const chatMode = new URLSearchParams(location.search).get("chat");
const longLabels = new URLSearchParams(location.search).get("labels") === "long";
const syntheticProvider = toolScenario ? toolProvider : "codex";
const syntheticStatus = composerScenario === "send" || composerScenario === "failure" || composerScenario === "pending" ? "completed"
  : chatMode === "failure" ? "failed"
  : chatMode === "approval" ? "waiting"
  : chatMode === "reading" || chatMode === "density" ? "completed" : "running";
const conversations: ConversationSummary[] = Array.from({ length: 60 }, (_, index) => ({
  id: `conversation-${index}`,
  parentId: null, hasChildren: true, summary: null,
  capabilities: { canSend: true, canInterrupt: true, canArchive: true, canRoute: true, unavailableReason: null },
  title: longLabels && index === 0 ? "Synthetic conversation with an intentionally long title for checking the compact header" : `Synthetic conversation ${index}`,
  routingProfile: "balanced",
  thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
  contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null,
  workspaceId: null,
  archived: false,
  projectRoot: null,
  currentRunId: `run-${index}`,
  provider: syntheticProvider,
  runStatus: syntheticStatus,
  rollupStatus: syntheticStatus === "failed" ? "failed" : syntheticStatus === "waiting" ? "needsAttention" : syntheticStatus === "completed" ? "completed" : "active",
}));

const children: ConversationSummary[] = conversations.flatMap((root, index) => {
  const child: ConversationSummary = {
    ...root, id: `child-${index}`, parentId: root.id, hasChildren: chatScenario,
    title: longLabels && index === 0 ? `Synthetic child ${"reviewing a long description of invented work ".repeat(12)}` : `Synthetic child ${index}`,
    summary: "Reviewing invented work",
    capabilities: { canSend: false, canInterrupt: false, canArchive: false, canRoute: false, unavailableReason: "This provider exposes recorded child activity, not an independently controllable chat." },
  };
  return [child, ...(chatScenario ? [{ ...child, id: `grandchild-${index}`, parentId: child.id, hasChildren: false, title: "Synthetic test conversation" }] : [])];
});

function loadSyntheticConversation(conversationId: string) {
  const found = [...conversations, ...children].find(({ id }) => id === conversationId);
  if (!found) throw new Error("Synthetic conversation is unavailable.");
  return found;
}

async function unsupported(): Promise<never> {
  throw new Error("This synthetic layout fixture does not support that action.");
}

const folderScenario = new URLSearchParams(location.search).get("folder");

const api: AppApi = {
  getBootstrap: async () => ({
    providers: [
      { id: "codex", installed: true, available: true, version: "synthetic", diagnostic: null, capabilities: ["steering", "interruption"] },
      { id: "claude", installed: toolScenario, available: toolScenario, version: toolScenario ? "synthetic" : null, diagnostic: toolScenario ? null : "Synthetic unavailable provider", capabilities: [] },
    ],
  }),
  listConversations: async () => ({ items: conversations, nextCursor: null }),
  loadConversation: async ({ conversationId }) => loadSyntheticConversation(conversationId),
  listChildConversations: async ({ parentId }) => ({ items: children.filter((child) => child.parentId === parentId), nextCursor: null }),
  loadConversationPath: async ({ conversationId }) => {
    const items = [loadSyntheticConversation(conversationId)];
    while (items[0].parentId) items.unshift(loadSyntheticConversation(items[0].parentId));
    return { items, truncated: false, ownerConversationId: items[0].id };
  },
  listenToAppEvents: async (handler) => toolScenario ? listenToToolEvents(handler) : listenToChatEvents(handler),
  loadTimeline: async ({ conversationId, cursor, limit }) => loadSyntheticConversation(conversationId).parentId !== null ? ({
    items: [{
      id: `${conversationId}-activity`, conversationId, runId: loadSyntheticConversation(conversationId).currentRunId!,
      agentId: conversationId, sequence: "1", kind: "message", role: "assistant", provider: syntheticProvider,
      presentation: "normal", content: "Captured synthetic child activity.", operation: null, contentBytes: "34", truncated: false,
    }],
    nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null, activeCompaction: null,
  }) : composerScenario ? ({
    items: composerMessages(conversationId), nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null, activeCompaction: null,
  }) : toolScenario && !conversationId.startsWith("created-") ? toolTimeline(conversationId, cursor, limit)
    : chatScenario && !conversationId.startsWith("created-") ? chatTimeline(conversationId, cursor, limit) : ({
    items: Array.from({ length: conversationId.startsWith("created-") ? 0 : 80 }, (_, index) => ({
      id: `event-${index}`, conversationId, runId: "run-0", agentId: "root-0",
      sequence: String(index + 1), kind: "message", role: "assistant", provider: "codex",
      presentation: "normal",
      content: `Synthetic message ${index}. This is invented layout content.`, operation: null, contentBytes: "60", truncated: false,
    })),
    nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null, activeCompaction: null,
  }),
  loadDiagnostics: async ({ conversationId, cursor, limit }) => chatScenario && loadSyntheticConversation(conversationId).parentId === null
    ? chatDiagnostics(conversationId, cursor, limit) : { items: [], nextCursor: null },
  loadApprovals: async () => ({ items: [], nextCursor: null }),
  inspectWorkspace: async () => ({
    workspace: {
      mode: "projectless",
      changes: Array.from({ length: 60 }, (_, index) => ({ kind: "present", relativePath: `synthetic-file-${index}.txt` })),
      truncated: false,
    },
    executionPath: "/synthetic/layout", ownedWorktree: false,
    cleanup: { eligible: false, blocker: "notOwned" }, currentRun: null,
    routing: null, handoff: null,
    activeDescendantCount: syntheticStatus === "running" || syntheticStatus === "waiting" ? (chatScenario ? 2 : 1) : 0,
  }),
  listRunAudits: async () => ({ items: [], nextCursor: null }),
  loadEventDetail: async ({ eventId }) => toolScenario ? toolEventDetail(eventId) : unsupported(),
  loadApprovalDetail: async ({ approvalId }) => {
    if (!chatScenario) return unsupported();
    const conversationId = approvalId.replace(/-approval$/, "");
    const approval = chatApproval(conversationId);
    return { ...approval, input: null, details: null, questionCount: 0, truncated: false };
  },
  loadApprovalQuestions: unsupported,
  submitMessage: composerScenario ? submitComposerMessage : unsupported,
  setThinkingPreference: unsupported,
  setContextBudget: unsupported,
  steerRun: composerScenario ? steerComposerRun : unsupported,
  respondToApproval: unsupported,
  interruptRun: unsupported,
  pickProjectDirectory: async () => {
    if (folderScenario === "cancel") return null;
    if (folderScenario === "error") throw new Error("Synthetic picker failure. Try again or continue without a folder.");
    return folderScenario === "non-git" ? "/synthetic/plain-folder" : "/synthetic/project";
  },
  inspectProject: async () => ({ isGit: folderScenario !== "non-git" }),
  createConversation: async (request) => {
    const conversation: ConversationSummary = {
      id: `created-${conversations.length}`, title: request.title,
      parentId: null, hasChildren: false, summary: null,
      capabilities: { canSend: true, canInterrupt: true, canArchive: true, canRoute: true, unavailableReason: null },
      routingProfile: request.routingProfile, workspaceId: `synthetic-workspace-${conversations.length}`,
      thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
      contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null,
      projectRoot: request.workspace.kind === "projectless" ? null : request.workspace.path,
      archived: false, currentRunId: null, provider: null, runStatus: null,
      rollupStatus: null,
    };
    conversations.unshift(conversation);
    return conversation;
  },
  archiveConversation: unsupported,
  loadRunAudit: unsupported,
};

const fixtureApi = treeScenario ? {
  ...api,
  listConversations: async () => treeListConversations(),
  loadConversation: async (request: { conversationId: string }) => treeLoadConversation(request),
  listChildConversations: async (request: Parameters<typeof treeListChildren>[0]) => treeListChildren(request),
  loadConversationPath: async (request: { conversationId: string }) => treeConversationPath(request),
  loadTimeline: async ({ conversationId, cursor, limit }: Parameters<AppApi["loadTimeline"]>[0]) => treeTimeline(conversationId, cursor, limit),
  loadDiagnostics: async () => ({ items: [], nextCursor: null }),
  listenToAppEvents: async (handler: Parameters<AppApi["listenToAppEvents"]>[0]) => listenToTreeEvents(handler),
  createConversation: unsupported,
  archiveConversation: async ({ conversationId }: { conversationId: string }) => {
    if (conversationId !== treeIds.root) return unsupported();
    archiveSyntheticRoot();
  },
} : api;

const thinkingApi = thinkingScenario ? withThinkingFixture(fixtureApi) : fixtureApi;
const scenarioApi = contextBudgetScenario ? withContextBudgetFixture(thinkingApi) : thinkingApi;
createRoot(document.getElementById("root")!).render(<App store={createAppStore(scenarioApi)} />);
