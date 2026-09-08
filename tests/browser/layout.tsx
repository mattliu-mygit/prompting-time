import { createRoot } from "react-dom/client";
import { App } from "../../src/app/App";
import { createAppStore, type AppApi } from "../../src/app/store";
import type { ConversationSummary } from "../../src/bridge/types";
import "../../src/styles/tokens.css";
import "../../src/styles/app.css";

// Invented data only. This entry point never uses the native bridge or providers.
const conversations: ConversationSummary[] = Array.from({ length: 60 }, (_, index) => ({
  id: `conversation-${index}`,
  title: `Synthetic conversation ${index}`,
  routingProfile: "balanced",
  workspaceId: null,
  archived: false,
  projectRoot: null,
  currentRunId: `run-${index}`,
  provider: "codex",
  runStatus: "running",
  rollupStatus: "active",
  agents: [
    { id: `root-${index}`, parentId: null, provider: "codex", label: "Root agent", summary: null, status: "running" },
    { id: `child-${index}`, parentId: `root-${index}`, provider: "codex", label: `Synthetic child ${index}`, summary: "Reviewing invented work", status: "running" },
  ],
  agentsTruncated: false,
}));

async function unsupported(): Promise<never> {
  throw new Error("This read-only layout fixture does not support that action.");
}

const api: AppApi = {
  getBootstrap: async () => ({
    providers: [
      { id: "codex", installed: true, available: true, version: "synthetic", diagnostic: null, capabilities: ["steering", "interruption"] },
      { id: "claude", installed: false, available: false, version: null, diagnostic: "Synthetic unavailable provider", capabilities: [] },
    ],
  }),
  listConversations: async () => ({ items: conversations, nextCursor: null }),
  loadConversation: async ({ conversationId }) => conversations.find(({ id }) => id === conversationId)!,
  loadAgentTree: async () => ({ runId: "run-0", items: [], nextCursor: null }),
  listenToAppEvents: async () => () => {},
  loadTimeline: async ({ conversationId }) => ({
    items: Array.from({ length: 80 }, (_, index) => ({
      id: `event-${index}`, conversationId, runId: "run-0", agentId: "root-0",
      sequence: String(index + 1), kind: "message", role: "assistant", provider: "codex",
      content: `Synthetic message ${index}. This is invented layout content.`, contentBytes: "60", truncated: false,
    })),
    nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
  }),
  loadApprovals: async () => ({ items: [], nextCursor: null }),
  inspectWorkspace: async () => ({
    workspace: {
      mode: "projectless",
      changes: Array.from({ length: 60 }, (_, index) => ({ kind: "present", relativePath: `synthetic-file-${index}.txt` })),
      truncated: false,
    },
    executionPath: "/synthetic/layout", ownedWorktree: false,
    cleanup: { eligible: false, blocker: "notOwned" }, currentRun: null,
    routing: null, handoff: null, activeDescendantCount: 1, agentsTruncated: false,
  }),
  listRunAudits: async () => ({ items: [], nextCursor: null }),
  loadEventDetail: unsupported,
  loadApprovalDetail: unsupported,
  loadApprovalQuestions: unsupported,
  submitMessage: unsupported,
  steerRun: unsupported,
  respondToApproval: unsupported,
  interruptRun: unsupported,
  inspectProject: unsupported,
  createConversation: unsupported,
  archiveConversation: unsupported,
  loadRunAudit: unsupported,
};

createRoot(document.getElementById("root")!).render(<App store={createAppStore(api)} />);
