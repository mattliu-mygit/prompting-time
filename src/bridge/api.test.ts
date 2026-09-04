import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  BridgeError,
  archiveConversation,
  createConversation,
  getBootstrap,
  inspectWorkspace,
  inspectProject,
  interruptRun,
  listConversations,
  loadConversation,
  loadAgentTree,
  loadApprovalDetail,
  loadApprovalQuestions,
  loadApprovals,
  loadEventDetail,
  listRunAudits,
  loadRunAudit,
  listenToAppEvents,
  loadTimeline,
  respondToApproval,
  steerRun,
  submitMessage,
} from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);

describe("desktop bridge", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("uses the exact command names and camel-case request envelope", async () => {
    invokeMock.mockResolvedValue({ runId: "run-1", status: "queued" });

    await submitMessage({
      conversationId: "c-1",
      text: "hello",
      providerOverride: null,
      commandId: "cmd-1",
    });

    expect(invokeMock).toHaveBeenCalledWith("submit_message", {
      request: {
        conversationId: "c-1",
        text: "hello",
        providerOverride: null,
        commandId: "cmd-1",
      },
    });
  });

  it("wraps every request-response command", async () => {
    invokeMock.mockResolvedValue(undefined);
    await getBootstrap();
    await listConversations({ cursor: null, limit: 25 });
    await loadConversation({ conversationId: "c-1" });
    await loadTimeline({ conversationId: "c-1", cursor: null, limit: 50 });
    await loadAgentTree({ conversationId: "c-1", cursor: null, limit: 100 });
    await loadEventDetail({ eventId: "event-1" });
    await loadApprovals({ conversationId: "c-1", cursor: null, limit: 100, kind: "pending" });
    await loadApprovalDetail({ approvalId: "approval-1" });
    await loadApprovalQuestions({ approvalId: "approval-1", cursor: null, limit: 50 });
    await createConversation({
      title: "Projectless chat",
      objective: "Explore an idea",
      constraints: [],
      workspace: { kind: "projectless" },
      routingProfile: "balanced",
    });
    await steerRun({ runId: "run-1", text: "focus on tests" });
    await respondToApproval({
      approvalId: "approval-1",
      response: { kind: "approved" },
    });
    await interruptRun({ runId: "run-1" });
    await archiveConversation({ conversationId: "c-1" });
    await inspectWorkspace({ conversationId: "c-1" });
    await inspectProject({ path: "/repo" });
    await listRunAudits({ conversationId: "c-1", cursor: null, limit: 10 });
    await loadRunAudit({ conversationId: "c-1", runId: "run-1" });

    expect(invokeMock.mock.calls).toEqual([
      ["bootstrap"],
      ["list_conversations", { request: { cursor: null, limit: 25 } }],
      ["load_conversation", { request: { conversationId: "c-1" } }],
      [
        "load_timeline",
        { request: { conversationId: "c-1", cursor: null, limit: 50 } },
      ],
      [
        "load_agent_tree",
        { request: { conversationId: "c-1", cursor: null, limit: 100 } },
      ],
      ["load_event_detail", { request: { eventId: "event-1" } }],
      [
        "load_approvals",
        {
          request: {
            conversationId: "c-1",
            cursor: null,
            limit: 100,
            kind: "pending",
          },
        },
      ],
      ["load_approval_detail", { request: { approvalId: "approval-1" } }],
      [
        "load_approval_questions",
        { request: { approvalId: "approval-1", cursor: null, limit: 50 } },
      ],
      [
        "create_conversation",
        {
          request: {
            title: "Projectless chat",
            objective: "Explore an idea",
            constraints: [],
            workspace: { kind: "projectless" },
            routingProfile: "balanced",
          },
        },
      ],
      ["steer_run", { request: { runId: "run-1", text: "focus on tests" } }],
      [
        "respond_to_approval",
        {
          request: {
            approvalId: "approval-1",
            response: { kind: "approved" },
          },
        },
      ],
      ["interrupt_run", { request: { runId: "run-1" } }],
      ["archive_conversation", { request: { conversationId: "c-1" } }],
      ["inspect_workspace", { request: { conversationId: "c-1" } }],
      ["inspect_project", { request: { path: "/repo" } }],
      ["list_run_audits", { request: { conversationId: "c-1", cursor: null, limit: 10 } }],
      ["load_run_audit", { request: { conversationId: "c-1", runId: "run-1" } }],
    ]);
  });

  it("decodes sanitized command errors", async () => {
    invokeMock.mockRejectedValue({
      code: "provider-unavailable",
      message: "No provider is currently available.",
      action: "Log in to an installed provider and retry.",
    });

    await expect(getBootstrap()).rejects.toEqual(
      new BridgeError(
        "provider-unavailable",
        "No provider is currently available.",
        "Log in to an installed provider and retry.",
      ),
    );
  });

  it.each([
    ["opaque object", { message: "private provider payload", raw: { sessionId: "native-secret" } }],
    ["generic error", new Error("private provider payload")],
  ])("does not expose %s rejection payloads", async (_name, rejection) => {
    invokeMock.mockRejectedValue(rejection);

    await expect(getBootstrap()).rejects.toEqual(
      new BridgeError(
        "internal",
        "Prompting Time could not complete the request.",
        null,
      ),
    );
  });

  it("classifies only an unstructured submit rejection as outcome-unknown", async () => {
    invokeMock.mockRejectedValue(new Error("private transport rejection"));

    await expect(submitMessage({
      conversationId: "c-1",
      text: "hello",
      providerOverride: null,
      commandId: "cmd-1",
    })).rejects.toEqual(new BridgeError(
      "outcome-unknown",
      "Prompting Time could not confirm whether the message was accepted.",
      "Retry this logical send.",
    ));

    invokeMock.mockRejectedValue({ code: "internal", message: "Safe internal failure", action: null });
    await expect(submitMessage({ conversationId: "c-1", text: "hello", providerOverride: null, commandId: "cmd-2" }))
      .rejects.toEqual(new BridgeError("internal", "Safe internal failure", null));

    invokeMock.mockRejectedValue(new Error("private read rejection"));
    await expect(getBootstrap()).rejects.toEqual(new BridgeError(
      "internal", "Prompting Time could not complete the request.", null,
    ));
  });

  it("subscribes to the single app event and returns its unsubscriber", async () => {
    const unlisten = vi.fn();
    const handler = vi.fn();
    listenMock.mockResolvedValue(unlisten);

    const unsubscribe = await listenToAppEvents(handler);
    const event = {
      sequence: 1,
      kind: "conversationChanged" as const,
      conversationId: "c-1",
    };
    const listener = listenMock.mock.calls[0]?.[1];
    listener?.({ event: "prompting-time://app-event", id: 1, payload: event });
    unsubscribe();

    expect(listenMock).toHaveBeenCalledWith(
      "prompting-time://app-event",
      expect.any(Function),
    );
    expect(handler).toHaveBeenCalledWith(event);
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("keeps provider-native payload fields out of ordinary snapshots", async () => {
    invokeMock.mockResolvedValue({
      items: [
        {
          id: "event-1",
          conversationId: "c-1",
          runId: "run-1",
          agentId: "agent-1",
          sequence: 1,
          kind: "progress",
          content: "Working",
        },
      ],
      nextCursor: null,
    });

    const snapshot = await loadTimeline({
      conversationId: "c-1",
      cursor: null,
      limit: 50,
    });

    expect(JSON.stringify(snapshot)).not.toMatch(
      /native(Session|Item|Thread|Request|Payload|Id)/i,
    );
  });
});
