import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import axe from "axe-core";
import { describe, expect, it, vi } from "vitest";
import type { ApprovalSnapshot, ConversationSummary, ProviderInstallation } from "../../bridge/types";
import type { AppActions, ConversationActions } from "../../app/store";
import { ApprovalCard } from "./ApprovalCard";
import { Inspector } from "./Inspector";

function actions(overrides: Partial<AppActions> = {}): AppActions {
  return {
    loadTimeline: vi.fn(), loadEventDetail: vi.fn(), loadApprovals: vi.fn(),
    loadApprovalDetail: vi.fn().mockResolvedValue({
      id: "approval-1", status: "pending", responsePending: false, operation: "Run command", scope: "This command",
      agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
      input: null, details: { kind: "commandExecution", command: "cargo test", cwd: "/work" },
      questionCount: 0, truncated: false,
    }),
    loadApprovalQuestions: vi.fn().mockResolvedValue({ items: [], totalCount: 0, nextCursor: null }),
    submitMessage: vi.fn(), steerRun: vi.fn(),
    respondToApproval: vi.fn().mockResolvedValue(undefined), interruptRun: vi.fn(),
    inspectWorkspace: vi.fn().mockResolvedValue({
      workspace: { mode: "isolated", changes: [{ kind: "modified", relativePath: "src/app.ts" }], truncated: false },
      executionPath: "/app-support/worktrees/conversation-1",
      ownedWorktree: true,
      cleanup: { eligible: false, blocker: "modifiedTrackedFiles" },
      currentRun: { id: "run-1", provider: "codex", status: "running" },
      routing: {
        provider: "codex", profile: "balanced", reason: "continuity", taskKind: "implementation",
        overrideProvider: null, eligibleProviders: ["codex"], requiredCapabilities: ["steering"],
        evaluations: [
          { provider: "codex", eligible: true, blockers: [] },
          { provider: "claude", eligible: false, blockers: [{ kind: "unavailable", value: "unauthenticated" }] },
        ],
        rationale: [
          { kind: "continuity", provider: "codex" },
          { kind: "rankedCandidates", candidates: [{ provider: "codex", recentRootRuns: "3", stableOrder: 0 }] },
        ], explanation: "Continued with Codex for this line of work.",
      },
      handoff: "Imported context: preserve the API boundary.",
      activeDescendantCount: 7,
      agentsTruncated: false,
    }),
    listRunAudits: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadRunAudit: vi.fn(),
    ...overrides,
  };
}

const approval: ApprovalSnapshot = {
  id: "approval-1", runId: "run-1", agentId: "root/reviewer", provider: "codex",
  operation: "Run command", scope: "This command", status: "pending", responsePending: false,
  agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
};

const conversation: ConversationSummary = {
  id: "conversation-1", title: "Work", workspaceId: "workspace-1", archived: false,
  projectRoot: "/repo", routingProfile: "balanced", currentRunId: "run-1", provider: "codex", runStatus: "running",
  rollupStatus: "active", agents: [], agentsTruncated: false,
};

const providers: ProviderInstallation[] = [
  { id: "codex", installed: true, available: true, version: "0.144.1", diagnostic: null, capabilities: ["steering"] },
  { id: "claude", installed: true, available: false, version: "2.1.205", diagnostic: "Sign in", capabilities: [] },
];

describe("ApprovalCard", () => {
  it("recovers an initial detail failure through an accessible focused retry", async () => {
    const api = actions({
      loadApprovalDetail: vi.fn()
        .mockRejectedValueOnce(new Error("Detail unavailable"))
        .mockResolvedValueOnce({
          id: "approval-1", status: "pending", responsePending: false,
          agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
          operation: "Run command", scope: "This command", input: null, details: null,
          questionCount: 0, truncated: false,
        }),
    });
    render(<ApprovalCard approval={approval} actions={api} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Run command" }));
    const retry = await screen.findByRole("button", { name: "Retry request details" });
    expect(retry).toHaveFocus();
    fireEvent.click(retry);
    expect(await screen.findByRole("button", { name: "Allow Run command" })).toBeEnabled();
    expect(api.loadApprovalDetail).toHaveBeenCalledTimes(2);
  });

  it("immediately reconciles a terminal exact detail", async () => {
    const terminal = {
      id: "approval-1", status: "denied" as const, responsePending: false,
      agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
      operation: "Run command", scope: "This command", input: null, details: null,
      questionCount: 0, truncated: false,
    };
    const onReconcile = vi.fn();
    render(<ApprovalCard approval={approval} actions={actions({ loadApprovalDetail: vi.fn().mockResolvedValue(terminal) })} onReconcile={onReconcile} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Run command" }));
    await waitFor(() => expect(onReconcile).toHaveBeenCalledWith(terminal));
    expect(screen.getByRole("button", { name: "Allow Run command" })).toBeDisabled();
  });

  it("recovers an initial question-page failure without remounting the approval", async () => {
    const api = actions({
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "approval-1", status: "pending", responsePending: false,
        agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
        operation: "Run command", scope: "This command", input: null, details: null,
        questionCount: 1, truncated: false,
      }),
      loadApprovalQuestions: vi.fn()
        .mockRejectedValueOnce(new Error("Questions unavailable"))
        .mockResolvedValueOnce({
          items: [{ id: "question-1", header: "Reason", question: "Why?", options: null, isOther: false, isSecret: false, truncated: false }],
          totalCount: 1, nextCursor: null,
        }),
    });
    render(<ApprovalCard approval={approval} actions={api} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Run command" }));
    const retry = await screen.findByRole("button", { name: "Retry request details" });
    expect(retry).toHaveFocus();
    fireEvent.click(retry);

    expect(await screen.findByRole("textbox", { name: "Answer" })).toBeEnabled();
    expect(api.loadApprovalQuestions).toHaveBeenCalledTimes(2);
  });

  it("shows exact operation context and permits only one response click", async () => {
    const api = actions();
    render(<ApprovalCard approval={approval} agentPath="Root/Reviewer" actions={api} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Run command" }));
    expect(await screen.findByText("cargo test")).toBeVisible();
    expect(screen.getByText("Root/Reviewer")).toBeVisible();
    const allow = screen.getByRole("button", { name: "Allow Run command" });
    fireEvent.click(allow);
    fireEvent.click(allow);
    await waitFor(() => expect(api.respondToApproval).toHaveBeenCalledTimes(1));
    expect(allow).toBeDisabled();
  });

  it("reconciles a stale response against durable approval state", async () => {
    const onReconcile = vi.fn().mockResolvedValue(undefined);
    const api = actions({ respondToApproval: vi.fn().mockRejectedValue(new Error("Approval is no longer pending")) });
    render(<ApprovalCard approval={approval} actions={api} onReconcile={onReconcile} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Run command" }));
    await screen.findByText("cargo test");
    fireEvent.click(screen.getByRole("button", { name: "Deny Run command" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Approval is no longer pending");
    expect(screen.getByRole("status")).toHaveFocus();
    expect(onReconcile).toHaveBeenCalled();
  });

  it("allows a retry only when durable state confirms no response is owned", async () => {
    const api = actions({ respondToApproval: vi.fn().mockRejectedValue(new Error("Connection closed")) });
    render(<ApprovalCard approval={approval} actions={api} onReconcile={vi.fn().mockResolvedValue(approval)} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Run command" }));
    await screen.findByText("cargo test");
    const deny = screen.getByRole("button", { name: "Deny Run command" });
    fireEvent.click(deny);
    await waitFor(() => expect(deny).toBeEnabled());
    expect(screen.getByRole("status")).toHaveTextContent("Connection closed");
  });

  it("pages exact questions and submits answers by app-owned question ID", async () => {
    const api = actions({
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "approval-1", status: "pending", responsePending: false, operation: "Choose rollout", scope: "This request", input: null,
        agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
        details: null, questionCount: 2, truncated: true,
      }),
      loadApprovalQuestions: vi.fn()
        .mockResolvedValueOnce({
          items: [{ id: "question-1", header: "Provider", question: "Which provider?", options: [{ label: "Codex", description: "Use Codex" }], isOther: false, isSecret: false, truncated: false }],
          totalCount: 2, nextCursor: "questions-2",
        })
        .mockResolvedValueOnce({
          items: [{ id: "question-2", header: "Reason", question: "Why?", options: null, isOther: true, isSecret: false, truncated: false }],
          totalCount: 2, nextCursor: null,
        }),
    });
    render(<ApprovalCard approval={{ ...approval, operation: "Choose rollout" }} actions={api} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Choose rollout" }));
    fireEvent.click(await screen.findByRole("radio", { name: /Codex/ }));
    fireEvent.click(screen.getByRole("button", { name: "Load more questions" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "Answer" }), { target: { value: "Best fit" } });
    fireEvent.click(screen.getByRole("button", { name: "Answer Choose rollout" }));
    await waitFor(() => expect(api.respondToApproval).toHaveBeenCalledWith({
      approvalId: "approval-1",
      response: { kind: "answers", value: { "question-1": ["Codex"], "question-2": ["Best fit"] } },
    }));
  });

  it("coalesces repeated question-page disclosure and deduplicates durable question IDs", async () => {
    let resolveNextPage!: (page: {
      items: Array<{
        id: string; header: string; question: string; options: null;
        isOther: boolean; isSecret: boolean; truncated: boolean;
      }>;
      totalCount: number;
      nextCursor: string | null;
    }) => void;
    const nextPage = new Promise<Parameters<typeof resolveNextPage>[0]>((resolve) => { resolveNextPage = resolve; });
    const api = actions({
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "approval-1", status: "pending", responsePending: false, operation: "Answer questions", scope: "This request", input: null,
        agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
        details: null, questionCount: 2, truncated: true,
      }),
      loadApprovalQuestions: vi.fn()
        .mockResolvedValueOnce({
          items: [{ id: "question-1", header: "First", question: "First?", options: null, isOther: false, isSecret: false, truncated: false }],
          totalCount: 2, nextCursor: "questions-2",
        })
        .mockReturnValueOnce(nextPage),
    });
    render(<ApprovalCard approval={{ ...approval, operation: "Answer questions" }} actions={api} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Answer questions" }));
    const loadMore = await screen.findByRole("button", { name: "Load more questions" });

    fireEvent.click(loadMore);
    fireEvent.click(loadMore);
    expect(api.loadApprovalQuestions).toHaveBeenCalledTimes(2);

    await act(async () => resolveNextPage({
      items: [
        { id: "question-1", header: "First", question: "First?", options: null, isOther: false, isSecret: false, truncated: false },
        { id: "question-2", header: "Second", question: "Second?", options: null, isOther: false, isSecret: false, truncated: false },
      ],
      totalCount: 2,
      nextCursor: null,
    }));

    expect(screen.getAllByRole("textbox")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Load more questions" })).not.toBeInTheDocument();
  });

  it("blocks arbitrary answers when non-other choices are incomplete", async () => {
    const api = actions({
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "approval-1", status: "pending", responsePending: false, operation: "Choose rollout", scope: "This request", input: null,
        agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
        details: null, questionCount: 1, truncated: true,
      }),
      loadApprovalQuestions: vi.fn().mockResolvedValue({
        items: [{ id: "question-1", header: "Provider", question: "Which provider?", options: null, isOther: false, isSecret: false, truncated: true }],
        totalCount: 1, nextCursor: null,
      }),
    });
    render(<ApprovalCard approval={{ ...approval, operation: "Choose rollout" }} actions={api} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Choose rollout" }));

    expect(await screen.findByText(/Exact choices are unavailable/)).toBeVisible();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Answer Choose rollout" })).toBeDisabled();
    expect(api.respondToApproval).not.toHaveBeenCalled();
  });

  it("accepts a complete canonical free-text question with null options", async () => {
    const api = actions({
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "approval-1", status: "pending", responsePending: false, operation: "Explain rollout", scope: "This request", input: null,
        agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
        details: null, questionCount: 1, truncated: false,
      }),
      loadApprovalQuestions: vi.fn().mockResolvedValue({
        items: [{ id: "question-1", header: "Reason", question: "Why?", options: null, isOther: false, isSecret: true, truncated: false }],
        totalCount: 1, nextCursor: null,
      }),
    });
    render(<ApprovalCard approval={{ ...approval, operation: "Explain rollout" }} actions={api} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Explain rollout" }));

    fireEvent.change(await screen.findByLabelText("Answer"), { target: { value: "Because it is safer" } });
    fireEvent.click(screen.getByRole("button", { name: "Answer Explain rollout" }));
    await waitFor(() => expect(api.respondToApproval).toHaveBeenCalledWith({
      approvalId: "approval-1", response: { kind: "answers", value: { "question-1": ["Because it is safer"] } },
    }));
  });

  it("uses complete canonical detail choices when the paged preview is truncated", async () => {
    const api = actions({
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "approval-1", status: "pending", responsePending: false, operation: "Choose rollout", scope: "This request",
        agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
        input: { questions: [{
          id: "question-1", header: "Provider", question: "Which provider?",
          options: [{ label: "Codex", description: "Use Codex" }, { label: "Claude", description: "Use Claude" }],
          isOther: false, isSecret: false,
        }], autoResolutionMs: null },
        details: null, questionCount: 1, truncated: false,
      }),
      loadApprovalQuestions: vi.fn().mockResolvedValue({
        items: [{ id: "question-1", header: "Provider", question: "Which provider?", options: null, isOther: false, isSecret: false, truncated: true }],
        totalCount: 1, nextCursor: null,
      }),
    });
    render(<ApprovalCard approval={{ ...approval, operation: "Choose rollout" }} actions={api} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Choose rollout" }));

    fireEvent.click(await screen.findByRole("radio", { name: /Claude/ }));
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Answer Choose rollout" }));
    await waitFor(() => expect(api.respondToApproval).toHaveBeenCalledWith({
      approvalId: "approval-1", response: { kind: "answers", value: { "question-1": ["Claude"] } },
    }));
  });

  it("has no detectable axe violations in an approval prompt", async () => {
    const { container } = render(<ApprovalCard approval={approval} actions={actions()} onReconcile={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Review Run command" }));
    await screen.findByText("cargo test");
    const result = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(result.violations.map(({ id }) => id)).toEqual([]);
  });
});

describe("Inspector", () => {
  it("never publishes a stale workspace result after the selected conversation changes", async () => {
    const baseline = await actions().inspectWorkspace({ conversationId: conversation.id });
    const resolvers = new Map<string, (value: typeof baseline) => void>();
    const inspectWorkspace = vi.fn(({ conversationId }: { conversationId: string }) => (
      new Promise<typeof baseline>((resolve) => resolvers.set(conversationId, resolve))
    ));
    const api = actions({ inspectWorkspace });
    const nextConversation = { ...conversation, id: "conversation-2", title: "Next" };
    const view = render(<Inspector conversation={conversation} providers={providers} refreshVersion={0} actions={api} />);
    view.rerender(<Inspector conversation={nextConversation} providers={providers} refreshVersion={0} actions={api} />);

    await act(async () => resolvers.get("conversation-2")?.({ ...baseline, executionPath: "/new-selection" }));
    expect(await screen.findByText("/new-selection")).toBeVisible();
    await act(async () => resolvers.get("conversation-1")?.({ ...baseline, executionPath: "/stale-selection" }));
    expect(screen.queryByText("/stale-selection")).not.toBeInTheDocument();
  });

  it("pages historical provider runs and lazily shows each exact route and handoff", async () => {
    const codexRouting = (await actions().inspectWorkspace({ conversationId: conversation.id })).routing!;
    const api = actions({
      listRunAudits: vi.fn().mockResolvedValue({
        items: [
          { id: "run-2", provider: "claude", status: "completed", reason: "manualOverride", routingTruncated: false, hasHandoff: true },
          { id: "run-1", provider: "codex", status: "completed", reason: "continuity", routingTruncated: false, hasHandoff: false },
        ],
        nextCursor: null,
      }),
      loadRunAudit: vi.fn().mockImplementation(({ runId }: { runId: string }) => Promise.resolve({
        id: runId, provider: runId === "run-1" ? "codex" : "claude", status: "completed",
        routing: runId === "run-1" ? { ...codexRouting, explanation: "Historical Codex route." } : { ...codexRouting, provider: "claude", reason: "manualOverride", explanation: "Switched to Claude." },
        reason: runId === "run-1" ? "continuity" : "manualOverride", routingTruncated: false,
        handoff: runId === "run-1" ? null : "Exact context sent to Claude.", handoffTruncated: false,
      })),
    });
    render(<Inspector conversation={conversation} providers={providers} refreshVersion={0} actions={api} />);
    await screen.findByRole("heading", { name: "Provider run history" });

    fireEvent.click(screen.getByRole("button", { name: "Inspect Claude run 1" }));
    expect(await screen.findByText("Exact context sent to Claude.")).toBeVisible();
    expect(screen.getByText("Switched to Claude.")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Inspect Codex run 2" }));
    expect(await screen.findByText("Historical Codex route.")).toBeVisible();
    expect(api.loadRunAudit).toHaveBeenNthCalledWith(2, { conversationId: conversation.id, runId: "run-1" });
  });

  it("keeps run paging and explicit detail reads independently convergent", async () => {
    const routing = (await actions().inspectWorkspace({ conversationId: conversation.id })).routing!;
    let resolveOlder!: (value: { items: Array<{ id: string; provider: "codex"; status: "completed"; reason: "continuity"; routingTruncated: false; hasHandoff: boolean }>; nextCursor: string | null }) => void;
    const older = new Promise<Parameters<typeof resolveOlder>[0]>((resolve) => { resolveOlder = resolve; });
    const listRunAudits = vi.fn()
      .mockResolvedValueOnce({ items: [{ id: "run-2", provider: "claude", status: "completed", reason: "manualOverride", routingTruncated: false, hasHandoff: true }], nextCursor: "older" })
      .mockImplementationOnce(() => older);
    const api = actions({
      listRunAudits,
      loadRunAudit: vi.fn().mockResolvedValue({ id: "run-2", provider: "claude", status: "completed", routing, reason: "manualOverride", routingTruncated: false, handoff: "detail", handoffTruncated: false }),
    });
    render(<Inspector conversation={conversation} providers={providers} refreshVersion={0} actions={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Load older provider runs" }));
    fireEvent.click(screen.getByRole("button", { name: "Inspect Claude run 1" }));
    expect(await screen.findByText("detail")).toBeVisible();
    await act(async () => resolveOlder({ items: [{ id: "run-1", provider: "codex", status: "completed", reason: "continuity", routingTruncated: false, hasHandoff: false }], nextCursor: null }));

    expect(await screen.findByRole("button", { name: "Inspect Codex run 2" })).toBeVisible();
    expect(screen.queryByText("Loading provider runs…")).not.toBeInTheDocument();
  });

  it("coalesces streamed run-history invalidations and converges on the newest run", async () => {
    type RunPage = Awaited<ReturnType<AppActions["listRunAudits"]>>;
    const resolvers: Array<(value: RunPage) => void> = [];
    let inFlight = 0;
    let maxInFlight = 0;
    const listRunAudits = vi.fn(() => new Promise<RunPage>((resolve) => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      resolvers.push((value) => { inFlight -= 1; resolve(value); });
    }));
    const api = actions({ listRunAudits });
    const view = render(<Inspector conversation={conversation} providers={providers} refreshVersion={0} actions={api} />);

    view.rerender(<Inspector conversation={{ ...conversation, currentRunId: "run-2" }} providers={providers} refreshVersion={1} actions={api} />);
    view.rerender(<Inspector conversation={{ ...conversation, currentRunId: "run-3" }} providers={providers} refreshVersion={2} actions={api} />);
    expect(listRunAudits).toHaveBeenCalledTimes(1);
    expect(maxInFlight).toBe(1);

    await act(async () => resolvers.shift()?.({
      items: [{ id: "run-1", provider: "codex", status: "completed", reason: "continuity", routingTruncated: false, hasHandoff: false }],
      nextCursor: null,
    }));
    await waitFor(() => expect(listRunAudits).toHaveBeenCalledTimes(2));
    await act(async () => resolvers.shift()?.({
      items: [{ id: "run-3", provider: "claude", status: "running", reason: "manualOverride", routingTruncated: false, hasHandoff: true }],
      nextCursor: null,
    }));

    expect(await screen.findByRole("button", { name: "Inspect Claude run 1" })).toBeVisible();
    expect(maxInFlight).toBe(1);
  });

  it("does not inspect Git work for streamed invalidations and coalesces explicit refreshes", async () => {
    let resolve!: (value: Awaited<ReturnType<ConversationActions["inspectWorkspace"]>>) => void;
    const inspectWorkspace = vi.fn(() => new Promise<Awaited<ReturnType<ConversationActions["inspectWorkspace"]>>>((next) => { resolve = next; }));
    const api = actions({ inspectWorkspace });
    const view = render(<Inspector conversation={conversation} providers={providers} refreshVersion={0} actions={api} />);
    for (let version = 1; version <= 100; version += 1) {
      view.rerender(<Inspector conversation={conversation} providers={providers} refreshVersion={version} actions={api} />);
    }
    expect(inspectWorkspace).toHaveBeenCalledTimes(1);
    await act(async () => resolve((await actions().inspectWorkspace({ conversationId: conversation.id }))));
    const refresh = await screen.findByRole("button", { name: "Refresh inspector" });
    expect(screen.getByText(/New conversation activity is available/)).toBeVisible();
    fireEvent.click(refresh);
    fireEvent.click(refresh);
    expect(inspectWorkspace).toHaveBeenCalledTimes(2);
  });

  it("shows exact routing, workspace, handoff, provider versions, and cleanup evidence", async () => {
    render(<Inspector conversation={conversation} providers={providers} refreshVersion={0} actions={actions()} />);
    expect(await screen.findByText("Continued with Codex for this line of work.")).toBeVisible();
    expect(screen.getByText("7 active descendants")).toBeVisible();
    expect(screen.getByText("src/app.ts")).toBeVisible();
    expect(screen.getByText("/app-support/worktrees/conversation-1")).toBeVisible();
    expect(screen.getByText("Prompting Time owned")).toBeVisible();
    expect(screen.getByText(/Codex \(3 recent root runs, stable order 0\)/)).toBeVisible();
    expect(screen.getByText(/Cleanup blocked: Modified tracked files/)).toBeVisible();
    expect(screen.getByText("Codex 0.144.1")).toBeVisible();
    expect(screen.getByText("Imported context: preserve the API boundary.")).toBeVisible();
    expect(screen.getByText(/Claude: unavailable.*unauthenticated/i)).toBeVisible();
  });

  it("keeps section collapse as local presentation state", async () => {
    render(<Inspector conversation={conversation} providers={providers} refreshVersion={0} actions={actions()} />);
    await screen.findByText("Continued with Codex for this line of work.");
    fireEvent.click(screen.getByRole("button", { name: "Collapse routing" }));
    expect(screen.queryByText("Continued with Codex for this line of work.")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Expand routing" }));
    expect(screen.getByText("Continued with Codex for this line of work.")).toBeVisible();
  });
});
