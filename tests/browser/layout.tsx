import { createRoot } from "react-dom/client";
import { App } from "../../src/app/App";
import { createAppStore, type AppApi } from "../../src/app/store";
import type { AgentSnapshot, ConversationSummary } from "../../src/bridge/types";
import { chatApproval, chatDiagnostics, chatScenario, chatTimeline, listenToChatEvents } from "./chat-fixture";
import { composerMessages, composerScenario, steerComposerRun, submitComposerMessage } from "./composer-fixture";
import "../../src/styles/tokens.css";
import "../../src/styles/app.css";

// Invented data only. This entry point never uses the native bridge or providers.
const chatMode = new URLSearchParams(location.search).get("chat");
const longLabels = new URLSearchParams(location.search).get("labels") === "long";
const syntheticStatus = composerScenario === "send" || composerScenario === "failure" || composerScenario === "pending" ? "completed"
  : chatMode === "failure" ? "failed"
  : chatMode === "approval" ? "waiting"
  : chatMode === "reading" || chatMode === "density" ? "completed" : "running";
const conversations: ConversationSummary[] = Array.from({ length: 60 }, (_, index) => ({
  id: `conversation-${index}`,
  title: longLabels && index === 0 ? "Synthetic conversation with an intentionally long title for checking the compact header" : `Synthetic conversation ${index}`,
  routingProfile: "balanced",
  workspaceId: null,
  archived: false,
  projectRoot: null,
  currentRunId: `run-${index}`,
  provider: "codex",
  runStatus: syntheticStatus,
  rollupStatus: syntheticStatus === "failed" ? "failed" : syntheticStatus === "waiting" ? "needsAttention" : syntheticStatus === "completed" ? "completed" : "active",
  agents: [
    { id: `root-${index}`, parentId: null, provider: "codex", label: longLabels && index === 0 ? `Synthetic root ${"reviewing invented work ".repeat(8)}` : "Root agent", summary: null, status: syntheticStatus },
    { id: `child-${index}`, parentId: `root-${index}`, provider: "codex", label: longLabels && index === 0 ? `Synthetic child ${"reviewing a long description of invented work ".repeat(12)}` : `Synthetic child ${index}`, summary: "Reviewing invented work", status: syntheticStatus },
    ...(chatScenario ? [{ id: `grandchild-${index}`, parentId: `child-${index}`, provider: "codex", label: "Synthetic test agent", summary: "Checking invented test results", status: syntheticStatus } satisfies AgentSnapshot] : []),
  ],
  agentsTruncated: false,
}));

async function unsupported(): Promise<never> {
  throw new Error("This synthetic layout fixture does not support that action.");
}

const folderScenario = new URLSearchParams(location.search).get("folder");

const api: AppApi = {
  getBootstrap: async () => ({
    providers: [
      { id: "codex", installed: true, available: true, version: "synthetic", diagnostic: null, capabilities: ["steering", "interruption"] },
      { id: "claude", installed: false, available: false, version: null, diagnostic: "Synthetic unavailable provider", capabilities: [] },
    ],
  }),
  listConversations: async () => ({ items: conversations, nextCursor: null }),
  loadConversation: async ({ conversationId }) => conversations.find(({ id }) => id === conversationId)!,
  loadAgentTree: async ({ conversationId }) => {
    const conversation = conversations.find(({ id }) => id === conversationId)!;
    return { runId: conversation.currentRunId, items: conversation.agents.map((agent, depth) => ({ agent, depth })), nextCursor: null };
  },
  listenToAppEvents: async (handler) => listenToChatEvents(handler),
  loadTimeline: async ({ conversationId, cursor, limit }) => composerScenario ? ({
    items: composerMessages(conversationId), nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
  }) : chatScenario && !conversationId.startsWith("created-") ? chatTimeline(conversationId, cursor, limit) : ({
    items: Array.from({ length: conversationId.startsWith("created-") ? 0 : 80 }, (_, index) => ({
      id: `event-${index}`, conversationId, runId: "run-0", agentId: "root-0",
      sequence: String(index + 1), kind: "message", role: "assistant", provider: "codex",
      presentation: "normal",
      content: `Synthetic message ${index}. This is invented layout content.`, contentBytes: "60", truncated: false,
    })),
    nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
  }),
  loadDiagnostics: async ({ conversationId, cursor, limit }) => chatScenario
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
    agentsTruncated: false,
  }),
  listRunAudits: async () => ({ items: [], nextCursor: null }),
  loadEventDetail: unsupported,
  loadApprovalDetail: async ({ approvalId }) => {
    if (!chatScenario) return unsupported();
    const conversationId = approvalId.replace(/-approval$/, "");
    const approval = chatApproval(conversationId);
    return { ...approval, input: null, details: null, questionCount: 0, truncated: false };
  },
  loadApprovalQuestions: unsupported,
  submitMessage: composerScenario ? submitComposerMessage : unsupported,
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
      routingProfile: request.routingProfile, workspaceId: `synthetic-workspace-${conversations.length}`,
      projectRoot: request.workspace.kind === "projectless" ? null : request.workspace.path,
      archived: false, currentRunId: null, provider: null, runStatus: null,
      rollupStatus: null, agents: [], agentsTruncated: false,
    };
    conversations.unshift(conversation);
    return conversation;
  },
  archiveConversation: unsupported,
  loadRunAudit: unsupported,
};

createRoot(document.getElementById("root")!).render(<App store={createAppStore(api)} />);
