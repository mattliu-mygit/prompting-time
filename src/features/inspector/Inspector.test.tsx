import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { createSignal, flush, Show } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type { ConversationSummary, ProviderInstallation, TimelineItem } from "../../bridge/types";
import type { AppActions, ConversationActions } from "../../app/store";
import { render, settle } from "../../test/render";
import { Inspector } from "./Inspector";

function actions(overrides: Partial<AppActions> = {}): AppActions {
  return {
    loadDiagnostics: vi.fn(), loadTimeline: vi.fn(), loadEventDetail: vi.fn(), loadApprovals: vi.fn(),
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
    }),
    listRunAudits: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadRunAudit: vi.fn(),
    ...overrides,
  };
}

const conversation: ConversationSummary = {
  contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null, thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
  parentId: null, hasChildren: false, summary: null,
  capabilities: { canSend: true, canInterrupt: true, canArchive: true, canRoute: true, unavailableReason: null },
  id: "conversation-1", title: "Work", workspaceId: "workspace-1", archived: false,
  projectRoot: "/repo", routingProfile: "balanced", currentRunId: "run-1", provider: "codex", runStatus: "running",
  rollupStatus: "active",
};

const providers: ProviderInstallation[] = [
  { id: "codex", installed: true, available: true, version: "0.144.1", diagnostic: null, capabilities: ["steering"] },
  { id: "claude", installed: true, available: false, version: "2.1.205", diagnostic: "Sign in", capabilities: [] },
];

/** Settles after a test resolves a pending request. */
async function act(run: () => unknown) {
  await run();
  await settle();
}

/** Renders the Inspector as App does: a new one per conversation, updated in place otherwise. */
function renderInspector(initial: { conversation: ConversationSummary; refreshVersion: number; actions: AppActions }) {
  const [current, setCurrent] = createSignal(initial.conversation);
  const [refreshVersion, setRefreshVersion] = createSignal(initial.refreshVersion);
  const view = render(() => (
    <Show when={current().id} keyed>
      {/* A keyed child must take the value to remount when it changes. */}
      {(id: string) => <Inspector conversation={{ ...current(), id }} providers={providers} refreshVersion={refreshVersion()} actions={initial.actions} />}
    </Show>
  ));
  return {
    ...view,
    rerender(next: { conversation?: ConversationSummary; refreshVersion?: number; actions?: AppActions }) {
      if (next.conversation) setCurrent(next.conversation);
      if (next.refreshVersion !== undefined) setRefreshVersion(next.refreshVersion);
      flush();
    },
  };
}

const click = (element: Element) => {
  fireEvent.click(element);
  flush();
};

describe("Inspector", () => {
  it("opens diagnostics independently before workspace inspection returns and never refreshes them implicitly", async () => {
    const api = actions({ inspectWorkspace: vi.fn(() => new Promise<never>(() => {})), loadDiagnostics: vi.fn().mockResolvedValue({ items: [], nextCursor: null }) });
    const view = renderInspector({ conversation: conversation, refreshVersion: 0, actions: api });
    expect(api.loadDiagnostics).not.toHaveBeenCalled();
    click(screen.getByRole("button", { name: "Expand diagnostics" }));
    await waitFor(() => expect(api.loadDiagnostics).toHaveBeenCalledWith({ conversationId: conversation.id, cursor: null, limit: 30 }));
    view.rerender({ conversation: conversation, refreshVersion: 20, actions: api });
    expect(api.loadDiagnostics).toHaveBeenCalledTimes(1);
    click(screen.getByRole("button", { name: "Refresh diagnostics" }));
    await waitFor(() => expect(api.loadDiagnostics).toHaveBeenCalledTimes(2));
  });

  it("pages diagnostics chronologically with four retained pages and lazy detail", async () => {
    const item = (sequence: number): TimelineItem => ({ id: `d${sequence}`, sequence: String(sequence), conversationId: conversation.id,
      runId: "run-1", agentId: "root", provider: "codex", kind: "diagnostic", presentation: "telemetry", role: null,
      content: `notification ${sequence}`, operation: null, contentBytes: "2000", truncated: true });
    const api = actions({ loadDiagnostics: vi.fn(({ cursor }) => {
      const end = cursor === null ? 150 : Number(cursor);
      return Promise.resolve({ items: Array.from({ length: 30 }, (_, index) => item(end - 29 + index)), nextCursor: end > 30 ? String(end - 30) : null });
    }), loadEventDetail: vi.fn().mockResolvedValue({ id: "d150", content: "exact notification", operation: null, contentBytes: "18", truncated: false }) });
    renderInspector({ conversation: conversation, refreshVersion: 0, actions: api });
    click(await screen.findByRole("button", { name: "Expand diagnostics" }));
    await screen.findByText("notification 150");
    expect(api.loadEventDetail).not.toHaveBeenCalled();
    const diagnostics = screen.getByRole("region", { name: "Diagnostics" });
    click(within(diagnostics).getAllByRole("button", { name: "Show full provider activity" }).at(-1)!);
    expect(await screen.findByText("exact notification")).toBeVisible();
    const enabledOlder = () => waitFor(() => {
      const button = screen.getByRole("button", { name: "Load older diagnostics" });
      expect(button).toBeEnabled();
      return button;
    });
    for (let page = 0; page < 4; page += 1) {
      click(await enabledOlder());
      await waitFor(() => expect(api.loadDiagnostics).toHaveBeenCalledTimes(page + 2));
    }
    // The last page reaches the first notification, so nothing older is left to load.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Load older diagnostics" })).not.toBeInTheDocument());
    expect(screen.queryByText("notification 150")).not.toBeInTheDocument();
    const entries = diagnostics.querySelectorAll("article");
    expect(entries).toHaveLength(120);
    expect(entries[0]).toHaveTextContent("notification 1");
    expect(entries[119]).toHaveTextContent("notification 120");
    click(screen.getByRole("button", { name: "Reload newest diagnostics" }));
    expect(await screen.findByText("notification 150")).toBeVisible();
    expect(diagnostics.querySelectorAll("article")).toHaveLength(30);
  });

  it("invalidates pending diagnostics on close and conversation switch and provides retry", async () => {
    const pending: Array<(page: { items: TimelineItem[]; nextCursor: string | null }) => void> = [];
    const api = actions({ loadDiagnostics: vi.fn()
      .mockRejectedValueOnce(new Error("diagnostic read failed"))
      .mockImplementation(() => new Promise((resolve) => { pending.push(resolve); })) });
    const view = renderInspector({ conversation: conversation, refreshVersion: 0, actions: api });
    click(await screen.findByRole("button", { name: "Expand diagnostics" }));
    expect(await screen.findByText("diagnostic read failed")).toBeVisible();
    click(screen.getByRole("button", { name: "Retry diagnostics" }));
    await waitFor(() => expect(api.loadDiagnostics).toHaveBeenCalledTimes(2));
    click(screen.getByRole("button", { name: "Collapse diagnostics" }));
    await act(async () => pending[0]!({ items: [], nextCursor: "stale" }));
    click(screen.getByRole("button", { name: "Expand diagnostics" }));
    await waitFor(() => expect(api.loadDiagnostics).toHaveBeenCalledTimes(3));
    view.rerender({ conversation: { ...conversation, id: "other" }, refreshVersion: 0, actions: api });
    await settle();
    await act(async () => pending[1]!({ items: [], nextCursor: "stale" }));
    expect(screen.getByRole("button", { name: "Expand diagnostics" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "Load older diagnostics" })).not.toBeInTheDocument();
  });

  it("never publishes a stale workspace result after the selected conversation changes", async () => {
    const baseline = await actions().inspectWorkspace({ conversationId: conversation.id });
    const resolvers = new Map<string, (value: typeof baseline) => void>();
    const inspectWorkspace = vi.fn(({ conversationId }: { conversationId: string }) => (
      new Promise<typeof baseline>((resolve) => resolvers.set(conversationId, resolve))
    ));
    const api = actions({ inspectWorkspace });
    const nextConversation = { ...conversation, id: "conversation-2", title: "Next" };
    const view = renderInspector({ conversation: conversation, refreshVersion: 0, actions: api });
    view.rerender({ conversation: nextConversation, refreshVersion: 0, actions: api });
    await waitFor(() => expect(resolvers.has("conversation-2")).toBe(true));
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
    renderInspector({ conversation: conversation, refreshVersion: 0, actions: api });
    await screen.findByRole("heading", { name: "Provider run history" });

    click(screen.getByRole("button", { name: "Inspect Claude run 1" }));
    expect(await screen.findByText("Exact context sent to Claude.")).toBeVisible();
    expect(screen.getByText("Switched to Claude.")).toBeVisible();
    click(screen.getByRole("button", { name: "Inspect Codex run 2" }));
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
    renderInspector({ conversation: conversation, refreshVersion: 0, actions: api });
    click(await screen.findByRole("button", { name: "Load older provider runs" }));
    click(screen.getByRole("button", { name: "Inspect Claude run 1" }));
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
    const view = renderInspector({ conversation: conversation, refreshVersion: 0, actions: api });

    view.rerender({ conversation: { ...conversation, currentRunId: "run-2" }, refreshVersion: 1, actions: api });
    view.rerender({ conversation: { ...conversation, currentRunId: "run-3" }, refreshVersion: 2, actions: api });
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
    const view = renderInspector({ conversation: conversation, refreshVersion: 0, actions: api });
    for (let version = 1; version <= 100; version += 1) {
      view.rerender({ conversation: conversation, refreshVersion: version, actions: api });
    }
    expect(inspectWorkspace).toHaveBeenCalledTimes(1);
    await act(async () => resolve((await actions().inspectWorkspace({ conversationId: conversation.id }))));
    const refresh = await screen.findByRole("button", { name: "Refresh inspector" });
    expect(screen.getByText(/New conversation activity is available/)).toBeVisible();
    click(refresh);
    click(refresh);
    expect(inspectWorkspace).toHaveBeenCalledTimes(2);
  });

  it("shows exact routing, workspace, handoff, provider versions, and cleanup evidence", async () => {
    renderInspector({ conversation: conversation, refreshVersion: 0, actions: actions() });
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
    renderInspector({ conversation: conversation, refreshVersion: 0, actions: actions() });
    await screen.findByText("Continued with Codex for this line of work.");
    click(screen.getByRole("button", { name: "Collapse routing" }));
    expect(screen.queryByText("Continued with Codex for this line of work.")).not.toBeInTheDocument();
    click(screen.getByRole("button", { name: "Expand routing" }));
    expect(screen.getByText("Continued with Codex for this line of work.")).toBeVisible();
  });
});
