import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppEvent,
  AgentTreePage,
  ApprovalDetailSnapshot,
  ApprovalPage,
  ApprovalQuestionPage,
  ArchiveConversationRequest,
  BootstrapSnapshot,
  CommandError,
  ConversationPage,
  ConversationSummary,
  CreateConversationRequest,
  InspectWorkspaceRequest,
  InspectProjectRequest,
  ProjectPathSnapshot,
  InspectorSnapshot,
  InterruptRunRequest,
  ListConversationsRequest,
  LoadConversationRequest,
  LoadAgentTreeRequest,
  LoadApprovalDetailRequest,
  LoadApprovalQuestionsRequest,
  LoadApprovalsRequest,
  LoadEventDetailRequest,
  LoadTimelineRequest,
  ListRunAuditsRequest,
  LoadRunAuditRequest,
  EventDetailSnapshot,
  RespondToApprovalRequest,
  SteerRunRequest,
  SubmissionSnapshot,
  SubmitMessageRequest,
  TimelinePage,
  RunAuditPage,
  RunAuditDetailSnapshot,
} from "./types";

const APP_EVENT_NAME = "prompting-time://app-event";

export class BridgeError extends Error {
  readonly code: string;
  readonly action: string | null;

  constructor(code: string, message: string, action: string | null) {
    super(message);
    this.name = "BridgeError";
    this.code = code;
    this.action = action;
  }
}

function isCommandError(value: unknown): value is CommandError {
  if (typeof value !== "object" || value === null) return false;
  const error = value as Record<string, unknown>;
  return typeof error.code === "string"
    && typeof error.message === "string"
    && (typeof error.action === "string" || error.action === null);
}

function decodeError(error: unknown): BridgeError {
  if (isCommandError(error)) {
    return new BridgeError(error.code, error.message, error.action);
  }
  return new BridgeError(
    "internal",
    "Prompting Time could not complete the request.",
    null,
  );
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return args === undefined
      ? await invoke<T>(command)
      : await invoke<T>(command, args);
  } catch (error) {
    throw decodeError(error);
  }
}

async function submitCall<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    if (isCommandError(error)) throw decodeError(error);
    throw new BridgeError(
      "outcome-unknown",
      "Prompting Time could not confirm whether the message was accepted.",
      "Retry this logical send.",
    );
  }
}

export function getBootstrap(): Promise<BootstrapSnapshot> {
  return call("bootstrap");
}

export function listConversations(request: ListConversationsRequest): Promise<ConversationPage> {
  return call("list_conversations", { request });
}

export function loadConversation(request: LoadConversationRequest): Promise<ConversationSummary> {
  return call("load_conversation", { request });
}

export function loadTimeline(request: LoadTimelineRequest): Promise<TimelinePage> {
  return call("load_timeline", { request });
}

export function loadAgentTree(request: LoadAgentTreeRequest): Promise<AgentTreePage> {
  return call("load_agent_tree", { request });
}

export function loadEventDetail(request: LoadEventDetailRequest): Promise<EventDetailSnapshot> {
  return call("load_event_detail", { request });
}

export function loadApprovals(request: LoadApprovalsRequest): Promise<ApprovalPage> {
  return call("load_approvals", { request });
}

export function loadApprovalDetail(
  request: LoadApprovalDetailRequest,
): Promise<ApprovalDetailSnapshot> {
  return call("load_approval_detail", { request });
}

export function loadApprovalQuestions(
  request: LoadApprovalQuestionsRequest,
): Promise<ApprovalQuestionPage> {
  return call("load_approval_questions", { request });
}

export function createConversation(request: CreateConversationRequest): Promise<ConversationSummary> {
  return call("create_conversation", { request });
}

export function submitMessage(request: SubmitMessageRequest): Promise<SubmissionSnapshot> {
  return submitCall("submit_message", { request });
}

export function steerRun(request: SteerRunRequest): Promise<void> {
  return call("steer_run", { request });
}

export function respondToApproval(request: RespondToApprovalRequest): Promise<void> {
  return call("respond_to_approval", { request });
}

export function interruptRun(request: InterruptRunRequest): Promise<void> {
  return call("interrupt_run", { request });
}

export function archiveConversation(request: ArchiveConversationRequest): Promise<void> {
  return call("archive_conversation", { request });
}

export function inspectWorkspace(request: InspectWorkspaceRequest): Promise<InspectorSnapshot> {
  return call("inspect_workspace", { request });
}

export function inspectProject(request: InspectProjectRequest): Promise<ProjectPathSnapshot> {
  return call("inspect_project", { request });
}

export function listRunAudits(request: ListRunAuditsRequest): Promise<RunAuditPage> {
  return call("list_run_audits", { request });
}

export function loadRunAudit(request: LoadRunAuditRequest): Promise<RunAuditDetailSnapshot> {
  return call("load_run_audit", { request });
}

export async function listenToAppEvents(handler: (event: AppEvent) => void): Promise<() => void> {
  return listen<AppEvent>(APP_EVENT_NAME, ({ payload }) => handler(payload));
}
