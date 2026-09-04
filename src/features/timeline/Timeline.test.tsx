import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AgentSnapshot, TimelineItem } from "../../bridge/types";
import type { ConversationActions } from "../../app/store";
import { Timeline } from "./Timeline";

function event(overrides: Partial<TimelineItem> & Pick<TimelineItem, "id" | "sequence">): TimelineItem {
  return {
    conversationId: "conversation-1",
    runId: "run-1",
    agentId: "root",
    kind: "message",
    role: "assistant",
    content: "Done",
    contentBytes: "4",
    truncated: false,
    provider: "codex",
    ...overrides,
  };
}

function actions(overrides: Partial<ConversationActions> = {}): ConversationActions {
  return {
    loadTimeline: vi.fn().mockResolvedValue({
      items: [
        event({ id: "user-1", sequence: "1", role: "user", content: "Please inspect this" }),
        event({ id: "assistant-1", sequence: "2", provider: "claude", content: "I found it" }),
        event({ id: "tool-1", sequence: "3", kind: "tool", role: null, content: "12 files", truncated: true }),
      ],
      nextCursor: "older-page",
      approvals: [],
      approvalsTruncated: false,
      approvalsNextCursor: null,
    }),
    loadEventDetail: vi.fn().mockResolvedValue({
      id: "tool-1",
      content: "Complete bounded tool output",
      contentBytes: "28",
      truncated: false,
    }),
    loadApprovals: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadApprovalDetail: vi.fn(),
    loadApprovalQuestions: vi.fn(),
    submitMessage: vi.fn(),
    steerRun: vi.fn(),
    respondToApproval: vi.fn(),
    interruptRun: vi.fn(),
    inspectWorkspace: vi.fn(),
    ...overrides,
  };
}

const agents: AgentSnapshot[] = [
  { id: "root", parentId: null, provider: "codex", label: "Root", summary: null, status: "running" },
  { id: "reviewer", parentId: "root", provider: "claude", label: "Reviewer", summary: "Checking API", status: "running" },
  { id: "tester", parentId: "reviewer", provider: "codex", label: "Tester", summary: null, status: "queued" },
];

describe("Timeline", () => {
  it("coalesces streamed newest-page invalidations into one in-flight read and one follow-up", async () => {
    const resolvers: Array<(page: ReturnType<typeof timelinePage>) => void> = [];
    let inFlight = 0;
    let maxInFlight = 0;
    const loadTimeline = vi.fn(() => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      return new Promise<ReturnType<typeof timelinePage>>((resolve) => {
        resolvers.push((page) => { inFlight -= 1; resolve(page); });
      });
    });
    const api = actions({ loadTimeline });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);
    for (let version = 1; version <= 100; version += 1) {
      view.rerender(<Timeline conversationId="conversation-1" refreshVersion={version} agents={[]} actions={api} />);
    }
    await waitFor(() => expect(loadTimeline).toHaveBeenCalledTimes(1));
    expect(maxInFlight).toBe(1);

    await act(async () => resolvers[0]!(timelinePage([event({ id: "old", sequence: "1", content: "old" })], null)));
    await waitFor(() => expect(loadTimeline).toHaveBeenCalledTimes(2));
    expect(maxInFlight).toBe(1);
    await act(async () => resolvers[1]!(timelinePage([event({ id: "latest", sequence: "2", content: "latest" })], null)));
    expect(await screen.findByText("latest")).toBeVisible();
    expect(loadTimeline).toHaveBeenCalledTimes(2);
  });

  it("mounts only the first 30 approval summaries and loads detail on explicit review", async () => {
    const first = Array.from({ length: 30 }, (_, index) => ({
      id: `approval-${index}`, runId: "run-1", agentId: "root", provider: "codex" as const,
      agentPath: ["Root"], agentPathTruncated: false,
      operation: `Action ${index}`, scope: "One action", status: "pending" as const, responsePending: false,
    }));
    const second = Array.from({ length: 30 }, (_, index) => ({
      ...first[index]!, id: `approval-${index + 30}`, operation: `Action ${index + 30}`,
    }));
    const api = actions({
      loadTimeline: vi.fn().mockResolvedValue({
        items: [], nextCursor: null, approvals: first, approvalsTruncated: true, approvalsNextCursor: "approvals-2",
      }),
      loadApprovals: vi.fn().mockResolvedValue({ items: second, nextCursor: null }),
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "approval-0", status: "pending", responsePending: false, agentPath: ["Root"], agentPathTruncated: false,
        operation: "Action 0", scope: "One action", input: null, details: null, questionCount: 0, truncated: false,
      }),
    });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);
    await screen.findByRole("heading", { name: "Needs your response" });
    expect(view.container.querySelectorAll(".approval-card")).toHaveLength(30);
    expect(api.loadApprovalDetail).not.toHaveBeenCalled();
    expect(api.loadApprovalQuestions).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Review Action 0" }));
    await screen.findByRole("button", { name: "Allow Action 0" });
    expect(api.loadApprovalDetail).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Load more approvals" }));
    await waitFor(() => expect(view.container.querySelectorAll(".approval-card")).toHaveLength(60));
    expect(api.loadApprovalDetail).toHaveBeenCalledTimes(1);
  });

  it("traverses every approval through a sliding four-page window and reloads the newest page", async () => {
    const approval = (index: number) => ({
      id: `approval-${index}`, runId: "run-1", agentId: "root", provider: "codex" as const,
      agentPath: ["Root"], agentPathTruncated: false,
      operation: `Action ${index}`, scope: "One action", status: "pending" as const, responsePending: false,
    });
    const all = Array.from({ length: 226 }, (_, index) => approval(index));
    const loadApprovals = vi.fn(({ cursor }: { cursor: string | null }) => {
      const offset = cursor === null ? 0 : Number(cursor);
      const nextOffset = Math.min(offset + 30, all.length);
      return Promise.resolve({
        items: all.slice(offset, nextOffset),
        nextCursor: nextOffset < all.length ? String(nextOffset) : null,
      });
    });
    const api = actions({
      loadTimeline: vi.fn().mockResolvedValue({
        items: [], nextCursor: null, approvals: all.slice(0, 30),
        approvalsTruncated: true, approvalsNextCursor: "30",
      }),
      loadApprovals,
    });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);

    for (let page = 1; page <= 7; page += 1) {
      fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
      await waitFor(() => expect(loadApprovals).toHaveBeenCalledTimes(page));
      const lastIndex = Math.min((page + 1) * 30 - 1, 225);
      expect(await screen.findByRole("button", { name: `Review Action ${lastIndex}` })).toBeVisible();
      expect(view.container.querySelectorAll(".approval-card").length).toBeLessThanOrEqual(120);
    }

    expect(screen.getByRole("button", { name: "Review Action 225" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Review Action 120" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Review Action 119" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Review Action 0" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Load more approvals" })).not.toBeInTheDocument();
    expect(loadApprovals.mock.calls.map(([request]) => request.cursor)).toEqual([
      "30", "60", "90", "120", "150", "180", "210",
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Reload newest approvals" }));
    await waitFor(() => expect(loadApprovals).toHaveBeenLastCalledWith({
      conversationId: "conversation-1", cursor: null, limit: 30, kind: "pending",
    }));
    expect(await screen.findByRole("button", { name: "Review Action 0" })).toBeVisible();
    expect(view.container.querySelectorAll(".approval-card")).toHaveLength(30);

    for (let page = 1; page <= 7; page += 1) {
      fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
      await waitFor(() => expect(loadApprovals).toHaveBeenCalledTimes(page + 8));
    }
    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={api} />);
    await waitFor(() => expect(loadApprovals).toHaveBeenCalledTimes(18));
    expect(loadApprovals.mock.calls.slice(-3).map(([request]) => request.cursor)).toEqual(["30", "60", "90"]);
    expect(await screen.findByRole("button", { name: "Review Action 0" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Review Action 119" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Review Action 120" })).not.toBeInTheDocument();
    expect(view.container.querySelectorAll(".approval-card")).toHaveLength(120);
    fireEvent.click(screen.getByRole("button", { name: "Load more approvals" }));
    await waitFor(() => expect(loadApprovals).toHaveBeenCalledTimes(19));
    expect(loadApprovals.mock.calls.at(-1)?.[0].cursor).toBe("120");
    expect(await screen.findByRole("button", { name: "Review Action 149" })).toBeVisible();
  });

  it("renders canonical roles, provider labels, recursive agents, and collapsed detail", async () => {
    const api = actions();
    render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={agents} actions={api} />);

    expect(await screen.findByRole("article", { name: "Codex user message" })).toHaveTextContent("Please inspect this");
    expect(screen.getByRole("article", { name: "Claude assistant message" })).toHaveTextContent("I found it");
    fireEvent.click(screen.getByRole("button", { name: "Show agent activity (2)" }));
    expect(screen.getByRole("article", { name: /Reviewer, Claude, Running/ })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Expand Reviewer" }));
    expect(screen.getByRole("article", { name: /Tester, Codex, Queued/ })).toHaveAttribute("data-depth", "2");
    expect(api.loadEventDetail).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Show tool output" }));
    expect(await screen.findByText("Complete bounded tool output")).toBeVisible();
    expect(api.loadEventDetail).toHaveBeenCalledWith({ eventId: "tool-1" });
    fireEvent.click(screen.getByRole("button", { name: "Hide tool output" }));
    expect(screen.queryByText("Complete bounded tool output")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show tool output" }));
    expect(await screen.findByText("Complete bounded tool output")).toBeVisible();
    expect(api.loadEventDetail).toHaveBeenCalledTimes(2);
  });

  it("loads the selected truncated run from the center without sidebar disclosure", async () => {
    const onLoadAgentPage = vi.fn();
    const view = render(<Timeline
      conversationId="conversation-1"
      refreshVersion={0}
      agents={[agents[0]!]}
      agentsTruncated
      agentWindow={null}
      onLoadAgentPage={onLoadAgentPage}
      actions={actions()}
    />);
    fireEvent.click(await screen.findByRole("button", { name: "Show agent activity (0)" }));
    expect(onLoadAgentPage).toHaveBeenCalledWith(true);

    view.rerender(<Timeline
      conversationId="conversation-1"
      refreshVersion={0}
      agents={agents}
      agentsTruncated
      agentWindow={{ conversationId: "conversation-1", runId: "run-1", pages: [["reviewer"]], nextCursor: "agents-2", loading: false, error: null, evicted: false }}
      onLoadAgentPage={onLoadAgentPage}
      actions={actions()}
    />);
    fireEvent.click(await screen.findByRole("button", { name: "Load more agent activity" }));
    expect(onLoadAgentPage).toHaveBeenLastCalledWith(false);
  });

  it("retries a failed center-owned agent page without sidebar disclosure", async () => {
    const onLoadAgentPage = vi.fn();
    render(<Timeline
      conversationId="conversation-1"
      refreshVersion={0}
      agents={[agents[0]!]}
      agentsTruncated
      agentWindow={{ conversationId: "conversation-1", runId: "run-1", pages: [], nextCursor: null, loading: false, error: "Agent page unavailable", evicted: false }}
      onLoadAgentPage={onLoadAgentPage}
      actions={actions()}
    />);

    fireEvent.click(await screen.findByRole("button", { name: "Show agent activity (0)" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Agent page unavailable");
    fireEvent.click(screen.getByRole("button", { name: "Retry agent activity" }));
    expect(onLoadAgentPage).toHaveBeenCalledWith(true);
  });

  it("ignores a stale approval page that resolves after a newer refresh", async () => {
    const first = { id: "first", runId: "run-1", agentId: "root", provider: "codex" as const, agentPath: ["Root"], agentPathTruncated: false, operation: "First", scope: "One", status: "pending" as const, responsePending: false };
    const stale = { ...first, id: "stale", operation: "Stale" };
    let resolvePage!: (value: { items: typeof stale[]; nextCursor: string | null }) => void;
    const page = new Promise<{ items: typeof stale[]; nextCursor: string | null }>((resolve) => { resolvePage = resolve; });
    const loadTimeline = vi.fn()
      .mockResolvedValueOnce({ items: [], nextCursor: null, approvals: [first], approvalsTruncated: true, approvalsNextCursor: "older" })
      .mockResolvedValueOnce({ items: [], nextCursor: null, approvals: [first], approvalsTruncated: true, approvalsNextCursor: "older" });
    const api = actions({ loadTimeline, loadApprovals: vi.fn(() => page) });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={api} />);
    await waitFor(() => expect(loadTimeline).toHaveBeenCalledTimes(2));
    await act(async () => resolvePage({ items: [stale], nextCursor: null }));

    expect(screen.queryByRole("button", { name: "Review Stale" })).not.toBeInTheDocument();
  });

  it("preserves explicitly disclosed approval pages across newest refreshes", async () => {
    const first = { id: "first", runId: "run-1", agentId: "root", provider: "codex" as const, agentPath: ["Root"], agentPathTruncated: false, operation: "First", scope: "One", status: "pending" as const, responsePending: false };
    const older = { ...first, id: "older", operation: "Older" };
    const loadTimeline = vi.fn().mockResolvedValue({ items: [], nextCursor: null, approvals: [first], approvalsTruncated: true, approvalsNextCursor: "older" });
    const api = actions({ loadTimeline, loadApprovals: vi.fn().mockResolvedValue({ items: [older], nextCursor: null }) });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
    expect(await screen.findByRole("button", { name: "Review Older" })).toBeVisible();
    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={api} />);

    expect(await screen.findByRole("button", { name: "Review Older" })).toBeVisible();
  });

  it("reconciles a resolved later approval page and restores focus", async () => {
    const first = { id: "first", runId: "run-1", agentId: "root", provider: "codex" as const, agentPath: ["Root"], agentPathTruncated: false, operation: "First", scope: "One", status: "pending" as const, responsePending: false };
    const older = { ...first, id: "older", operation: "Older" };
    const loadTimeline = vi.fn().mockResolvedValue({ items: [], nextCursor: null, approvals: [first], approvalsTruncated: true, approvalsNextCursor: "older-page" });
    const loadApprovals = vi.fn()
      .mockResolvedValueOnce({ items: [older], nextCursor: null })
      .mockResolvedValueOnce({ items: [], nextCursor: null });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={actions({ loadTimeline, loadApprovals })} />);
    fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
    const olderReview = await screen.findByRole("button", { name: "Review Older" });
    olderReview.focus();
    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={actions({ loadTimeline, loadApprovals })} />);

    await waitFor(() => expect(screen.queryByRole("button", { name: "Review Older" })).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { name: "Timeline" })).toHaveFocus();
  });

  it("prepends cursor pages and merges a live newest-page refresh by durable ID", async () => {
    const loadTimeline = vi.fn()
      .mockResolvedValueOnce({
        items: [event({ id: "e2", sequence: "2", content: "second" })],
        nextCursor: "older",
        approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
      })
      .mockResolvedValueOnce({
        items: [event({ id: "e1", sequence: "1", role: "user", content: "first" })],
        nextCursor: null,
        approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
      })
      .mockResolvedValueOnce({
        items: [
          event({ id: "e2", sequence: "2", content: "second, updated" }),
          event({ id: "e3", sequence: "3", content: "third" }),
        ],
        nextCursor: "older",
        approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
      });
    const api = actions({ loadTimeline });
    const view = render(
      <Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />,
    );
    await screen.findByText("second");
    fireEvent.click(screen.getByRole("button", { name: "Load older activity" }));
    expect(await screen.findByText("first")).toBeVisible();

    view.rerender(
      <Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={api} />,
    );
    expect(await screen.findByText("second, updated")).toBeVisible();
    expect(screen.getByText("third")).toBeVisible();
    expect(screen.getAllByRole("article", { name: "Codex assistant message" })).toHaveLength(2);
  });

  it("distinguishes progress, provider activity, lifecycle, and failures accessibly", async () => {
    const api = actions({
      loadTimeline: vi.fn().mockResolvedValue({
        items: [
          event({ id: "p", sequence: "1", kind: "progress", role: null, content: "Indexing" }),
          event({ id: "d", sequence: "2", kind: "diagnostic", role: null, content: "Provider protocol warning" }),
          event({ id: "l", sequence: "3", kind: "lifecycle", role: null, content: "Run failed: process exited" }),
        ],
        nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
      }),
    });
    render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);

    expect(await screen.findByRole("article", { name: "Codex progress" })).toHaveTextContent("Indexing");
    expect(screen.queryByRole("status", { name: "Codex progress" })).not.toBeInTheDocument();
    expect(screen.getByRole("article", { name: "Codex provider activity" })).toBeVisible();
    expect(screen.getByRole("article", { name: "Codex failure" })).toHaveTextContent("Run failed");
  });

  it("discloses bounded detail for every truncated event kind only on request", async () => {
    const items = [
      event({ id: "user", sequence: "1", role: "user", content: "Long user preview", truncated: true }),
      event({ id: "assistant", sequence: "2", content: "Long assistant preview", truncated: true }),
      event({ id: "progress", sequence: "3", kind: "progress", role: null, content: "Long progress preview", truncated: true }),
      event({ id: "diagnostic", sequence: "4", kind: "diagnostic", role: null, content: "Long diagnostic preview", truncated: true }),
      event({ id: "lifecycle", sequence: "5", kind: "lifecycle", role: null, content: "Long lifecycle preview", truncated: true }),
    ];
    const loadEventDetail = vi.fn(({ eventId }: { eventId: string }) => Promise.resolve({
      id: eventId, content: `Full ${eventId} detail`, contentBytes: "2000", truncated: false,
    }));
    const api = actions({
      loadTimeline: vi.fn().mockResolvedValue({ items, nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null }),
      loadEventDetail,
    });
    render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);

    const user = await screen.findByRole("article", { name: "Codex user message" });
    const assistant = screen.getByRole("article", { name: "Codex assistant message" });
    const progress = screen.getByRole("article", { name: "Codex progress" });
    const diagnostic = screen.getByRole("article", { name: "Codex provider activity" });
    const lifecycle = screen.getByRole("article", { name: "Codex run lifecycle" });
    expect(loadEventDetail).not.toHaveBeenCalled();
    expect(within(user).getByText("Preview truncated.")).toBeVisible();

    fireEvent.click(within(user).getByRole("button", { name: "Show full message" }));
    fireEvent.click(within(assistant).getByRole("button", { name: "Show full message" }));
    fireEvent.click(within(progress).getByRole("button", { name: "Show full progress" }));
    fireEvent.click(within(diagnostic).getByRole("button", { name: "Show full provider activity" }));
    fireEvent.click(within(lifecycle).getByRole("button", { name: "Show full run lifecycle" }));
    expect(await screen.findByText("Full user detail")).toBeVisible();
    expect(screen.getByText("Full progress detail")).toBeVisible();
    expect(screen.getByText("Full diagnostic detail")).toBeVisible();
    expect(loadEventDetail.mock.calls.map(([request]) => request.eventId)).toEqual([
      "user", "assistant", "progress", "diagnostic", "lifecycle",
    ]);
  });

  it("invalidates expanded detail when streamed content grows under the same event ID", async () => {
    const loadTimeline = vi.fn()
      .mockResolvedValueOnce(timelinePage([event({ id: "stream", sequence: "1", content: "preview one", contentBytes: "100", truncated: true })], null))
      .mockResolvedValueOnce(timelinePage([event({ id: "stream", sequence: "1", content: "preview two", contentBytes: "200", truncated: true })], null));
    const loadEventDetail = vi.fn()
      .mockResolvedValueOnce({ id: "stream", content: "full one", contentBytes: "100", truncated: false })
      .mockResolvedValueOnce({ id: "stream", content: "full two", contentBytes: "200", truncated: false });
    const api = actions({ loadTimeline, loadEventDetail });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Show full message" }));
    expect(await screen.findByText("full one")).toBeVisible();

    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={api} />);
    expect(await screen.findByText("preview two")).toBeVisible();
    expect(screen.queryByText("full one")).not.toBeInTheDocument();
    expect(loadEventDetail).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Show full message" }));
    expect(await screen.findByText("full two")).toBeVisible();
    expect(loadEventDetail).toHaveBeenCalledTimes(2);
  });

  it("decides newest auto-scroll stickiness when the refresh commits", async () => {
    let resolveRefresh!: (page: ReturnType<typeof timelinePage>) => void;
    const refresh = new Promise<ReturnType<typeof timelinePage>>((resolve) => { resolveRefresh = resolve; });
    const loadTimeline = vi.fn()
      .mockResolvedValueOnce(timelinePage([event({ id: "one", sequence: "1" })], null))
      .mockImplementationOnce(() => refresh);
    const api = actions({ loadTimeline });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);
    await screen.findByText("Done");
    const activity = screen.getByLabelText("Conversation activity");
    Object.defineProperty(activity, "scrollHeight", { configurable: true, value: 200 });
    Object.defineProperty(activity, "clientHeight", { configurable: true, value: 50 });
    activity.scrollTop = 150;

    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={api} />);
    activity.scrollTop = 20;
    await act(async () => resolveRefresh(timelinePage([event({ id: "two", sequence: "2", content: "new" })], null)));
    await screen.findByText("new");
    expect(activity.scrollTop).toBe(20);
  });

  it("bounds the live timeline window even across oversized refreshes", async () => {
    const page = (start: number) => Array.from({ length: 1_000 }, (_, offset) => event({
      id: `event-${start + offset}`,
      sequence: String(start + offset),
      content: `event ${start + offset}`,
    }));
    const loadTimeline = vi.fn()
      .mockResolvedValueOnce({ items: page(1), nextCursor: "older-1", approvals: [], approvalsTruncated: false, approvalsNextCursor: null })
      .mockResolvedValueOnce({ items: page(1_001), nextCursor: "older-2", approvals: [], approvalsTruncated: false, approvalsNextCursor: null });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={actions({ loadTimeline })} />);
    await screen.findByText("event 1000");
    expect(view.container.querySelectorAll("[data-timeline-id]").length).toBeLessThanOrEqual(80);

    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={actions({ loadTimeline })} />);
    await screen.findByText("event 2000");
    expect(view.container.querySelectorAll("[data-timeline-id]").length).toBeLessThanOrEqual(80);
    expect(screen.queryByText("event 1")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reload newest history" })).toBeVisible();
  });

  it("bounds explicitly loaded older pages and offers a newest-history reset", async () => {
    const page = (first: number, cursor: string | null) => timelinePage(
      Array.from({ length: 80 }, (_, offset) => event({
        id: `event-${first + offset}`,
        sequence: String(first + offset),
        content: `event ${first + offset}`,
      })),
      cursor,
    );
    const pages = [
      page(921, "cursor-1"), page(841, "cursor-2"), page(761, "cursor-3"),
      page(681, "cursor-4"), page(601, "cursor-5"), page(521, "cursor-6"),
    ];
    const loadTimeline = vi.fn()
      .mockImplementationOnce(() => Promise.resolve(pages[0]!))
      .mockImplementationOnce(() => Promise.resolve(pages[1]!))
      .mockImplementationOnce(() => Promise.resolve(pages[2]!))
      .mockImplementationOnce(() => Promise.resolve(pages[3]!))
      .mockImplementationOnce(() => Promise.resolve(pages[4]!))
      .mockImplementationOnce(() => Promise.resolve(pages[5]!));
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={actions({ loadTimeline })} />);
    await screen.findByText("event 1000");
    for (const oldest of [841, 761, 681, 601, 521]) {
      fireEvent.click(screen.getByRole("button", { name: "Load older activity" }));
      await screen.findByText(`event ${oldest}`);
    }

    expect(view.container.querySelectorAll("[data-timeline-id]").length).toBeLessThanOrEqual(400);
    fireEvent.click(screen.getByRole("button", { name: "Reload newest history" }));
    expect(view.container.querySelectorAll("[data-timeline-id]").length).toBeLessThanOrEqual(80);
    expect(screen.queryByText("event 521")).not.toBeInTheDocument();
  });

  it("anchors an older prepend to its own commit while a live refresh waits behind it", async () => {
    let resolveOlder!: (page: ReturnType<typeof timelinePage>) => void;
    const older = new Promise<ReturnType<typeof timelinePage>>((resolve) => { resolveOlder = resolve; });
    let newestCalls = 0;
    const loadTimeline = vi.fn(({ cursor }: { cursor: string | null }) => {
      if (cursor) return older;
      newestCalls += 1;
      return Promise.resolve(timelinePage(
        newestCalls === 1
          ? [event({ id: "e2", sequence: "2", content: "second" })]
          : [event({ id: "e2", sequence: "2", content: "second" }), event({ id: "e3", sequence: "3", content: "third" })],
        "older",
      ));
    });
    const api = actions({ loadTimeline });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);
    await screen.findByText("second");
    const activity = screen.getByLabelText("Conversation activity");
    const anchor = viewRow(activity, "e2");
    Object.defineProperty(anchor, "offsetTop", {
      configurable: true,
      get: () => screen.queryByText("first") ? 90 : 50,
    });
    Object.defineProperty(activity, "scrollHeight", { configurable: true, value: 200 });
    Object.defineProperty(activity, "clientHeight", { configurable: true, value: 50 });
    activity.scrollTop = 25;

    fireEvent.click(screen.getByRole("button", { name: "Load older activity" }));
    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={api} />);
    await waitFor(() => expect(loadTimeline).toHaveBeenCalledTimes(2));
    expect(screen.queryByText("third")).not.toBeInTheDocument();
    expect(activity.scrollTop).toBe(25);

    activity.scrollTop = 40;
    resolveOlder(timelinePage([event({ id: "e1", sequence: "1", role: "user", content: "first" })], null));
    await screen.findByText("first");
    expect(activity.scrollTop).toBe(80);
    expect(await screen.findByText("third")).toBeVisible();
  });

  it("keeps a wide and deep agent hierarchy collapsed and DOM-bounded", async () => {
    const manyAgents: AgentSnapshot[] = [
      { id: "root", parentId: null, provider: "codex", label: "Root", summary: null, status: "running" },
      ...Array.from({ length: 200 }, (_, index) => ({
        id: `child-${index}`, parentId: "root", provider: "claude" as const, label: `Child ${index}`, summary: null, status: "running" as const,
      })),
      ...Array.from({ length: 400 }, (_, index) => ({
        id: `grandchild-${index}`, parentId: `child-${Math.floor(index / 2)}`, provider: "codex" as const,
        label: `Grandchild ${index}`, summary: null, status: "queued" as const,
      })),
    ];
    const api = actions({
      loadTimeline: vi.fn().mockResolvedValue({
        items: [], nextCursor: null,
        approvals: [{ id: "approval", runId: "run-1", agentId: "root", provider: "codex", operation: "Confirm", scope: "One action", status: "pending", responsePending: false }],
        approvalsTruncated: false, approvalsNextCursor: null,
      }),
      loadApprovalDetail: vi.fn().mockResolvedValue({ id: "approval", status: "pending", responsePending: false, operation: "Confirm", scope: "One action", input: null, details: null, questionCount: 0, truncated: false }),
    });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={manyAgents} actions={api} />);
    expect(await screen.findByRole("heading", { name: "Needs your response" })).toBeVisible();
    expect(view.container.querySelectorAll(".agent-card")).toHaveLength(0);

    fireEvent.click(screen.getByRole("button", { name: "Show agent activity (600)" }));
    expect(view.container.querySelectorAll(".agent-card").length).toBeLessThanOrEqual(80);
    fireEvent.click(screen.getByRole("button", { name: "Expand Child 0" }));
    fireEvent.click(screen.getByRole("button", { name: "Show more agents" }));
    expect(view.container.querySelectorAll(".agent-card").length).toBeLessThanOrEqual(80);
    expect(screen.queryByRole("article", { name: /Child 0, Claude, Running/ })).not.toBeInTheDocument();
    expect(screen.getByRole("article", { name: /Child 20, Claude, Running/ })).toBeVisible();
    expect(screen.getByRole("heading", { name: "Needs your response" })).toBeVisible();
  });

  it("makes a descendant beyond the mounted agent-card budget reachable", async () => {
    const deepAgents: AgentSnapshot[] = [
      { id: "root", parentId: null, provider: "codex", label: "Root", summary: null, status: "running" },
      ...Array.from({ length: 100 }, (_, index) => ({
        id: `agent-${index + 1}`,
        parentId: index === 0 ? "root" : `agent-${index}`,
        provider: "claude" as const,
        label: `Agent ${index + 1}`,
        summary: null,
        status: "running" as const,
      })),
    ];
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={deepAgents} actions={actions()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Show agent activity (100)" }));
    for (let index = 1; index <= 20; index += 1) {
      fireEvent.click(screen.getByRole("button", { name: `Expand Agent ${index}` }));
    }

    fireEvent.click(screen.getByRole("button", { name: "Show more agents" }));
    expect(screen.getByRole("article", { name: /Agent 21, Claude, Running/ })).toBeVisible();
    expect(view.container.querySelectorAll(".agent-card").length).toBeLessThanOrEqual(80);
  });

  it("counts a very wide nested orchestrator without overflowing traversal", async () => {
    const wideAgents: AgentSnapshot[] = [
      { id: "root", parentId: null, provider: "codex", label: "Root", summary: null, status: "running" },
      { id: "orchestrator", parentId: "root", provider: "claude", label: "Orchestrator", summary: null, status: "running" },
      ...Array.from({ length: 200_000 }, (_, index) => ({
        id: `leaf-${index}`,
        parentId: "orchestrator",
        provider: "codex" as const,
        label: `Leaf ${index}`,
        summary: null,
        status: "queued" as const,
      })),
    ];

    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={wideAgents} actions={actions()} />);
    expect(await screen.findByRole("button", { name: "Show agent activity (200001)" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Show agent activity (200001)" }));
    fireEvent.click(screen.getByRole("button", { name: "Expand Orchestrator" }));
    expect(view.container.querySelectorAll(".agent-card")).toHaveLength(20);
    expect(screen.getByRole("button", { name: "Show more agents" })).toBeVisible();
  }, 10_000);

  it("ignores an older-page response after the user resets to newest history", async () => {
    let resolveOlder!: (page: ReturnType<typeof timelinePage>) => void;
    const older = new Promise<ReturnType<typeof timelinePage>>((resolve) => { resolveOlder = resolve; });
    let newestCalls = 0;
    const loadTimeline = vi.fn(({ cursor }: { cursor: string | null }) => {
      if (cursor) return older;
      newestCalls += 1;
      return Promise.resolve(timelinePage([
        event({ id: `new-${newestCalls}`, sequence: String(10 + newestCalls), content: `new ${newestCalls}` }),
      ], `older-${newestCalls}`));
    });
    const api = actions({ loadTimeline });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={api} />);
    await screen.findByText("new 1");
    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={[]} actions={api} />);
    await screen.findByText("new 2");

    fireEvent.click(screen.getByRole("button", { name: "Load older activity" }));
    fireEvent.click(screen.getByRole("button", { name: "Reload newest history" }));
    await act(async () => resolveOlder(timelinePage([
      event({ id: "stale-old", sequence: "1", content: "stale older response" }),
    ], null)));

    expect(loadTimeline).toHaveBeenCalledTimes(3);
    expect(screen.queryByText("stale older response")).not.toBeInTheDocument();
    expect(screen.getByText("new 2")).toBeVisible();
  });

  it("preserves the visible scroll anchor when older activity is prepended", async () => {
    const loadTimeline = vi.fn()
      .mockResolvedValueOnce({
        items: [event({ id: "new", sequence: "2", content: "newer" })],
        nextCursor: "older", approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
      })
      .mockResolvedValueOnce({
        items: [event({ id: "old", sequence: "1", content: "older" })],
        nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
      });
    render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={[]} actions={actions({ loadTimeline })} />);
    await screen.findByText("newer");
    const activity = screen.getByLabelText("Conversation activity");
    const anchor = viewRow(activity, "new");
    Object.defineProperty(anchor, "offsetTop", {
      configurable: true,
      get: () => screen.queryByText("older") ? 110 : 50,
    });
    activity.scrollTop = 25;

    fireEvent.click(screen.getByRole("button", { name: "Load older activity" }));

    await screen.findByText("older");
    expect(activity.scrollTop).toBe(85);
  });

  it("shows a canonical label path instead of the app-owned agent identifier", async () => {
    const api = actions({
      loadTimeline: vi.fn().mockResolvedValue({
        items: [], nextCursor: null,
        approvals: [{
          id: "approval", runId: "run-1", agentId: "reviewer", provider: "claude",
          agentPath: ["Root", "Reviewer"], agentPathTruncated: false,
          operation: "Edit file", scope: "One file", status: "pending", responsePending: false,
        }],
        approvalsTruncated: false, approvalsNextCursor: null,
      }),
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "approval", status: "pending", responsePending: false, operation: "Edit file", scope: "One file", input: null,
        details: { kind: "fileChange", changes: [{ path: "src/app.ts", change: { kind: "update", movePath: null } }], grantRoot: null, reason: null },
        questionCount: 0, truncated: false,
      }),
    });
    render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={agents} actions={api} />);

    expect(await screen.findByText("Root/Reviewer")).toBeVisible();
    expect(screen.queryByText("reviewer")).not.toBeInTheDocument();
  });

  it("reconciles a later-page approval by exact durable ID", async () => {
    let laterReads = 0;
    const loadApprovalDetail = vi.fn(({ approvalId }: { approvalId: string }) => {
      if (approvalId === "later") laterReads += 1;
      return Promise.resolve({
        id: approvalId,
        status: approvalId === "later" && laterReads > 1 ? "approved" as const : "pending" as const,
        responsePending: false,
        agentPath: ["Root"],
        agentPathTruncated: false,
        operation: approvalId === "later" ? "Later action" : "First action",
        scope: "One action", input: null, details: null, questionCount: 0, truncated: false,
      });
    });
    const api = actions({
      loadTimeline: vi.fn().mockResolvedValue({
        items: [], nextCursor: null,
        approvals: [{ id: "first", runId: "run-1", agentId: "root", provider: "codex", agentPath: ["Root"], agentPathTruncated: false, operation: "First action", scope: "One action", status: "pending", responsePending: false }],
        approvalsTruncated: true, approvalsNextCursor: "approval-page-2",
      }),
      loadApprovals: vi.fn().mockResolvedValue({
        items: [{ id: "later", runId: "run-1", agentId: "root", provider: "codex", agentPath: ["Root"], agentPathTruncated: false, operation: "Later action", scope: "One action", status: "pending", responsePending: false }],
        nextCursor: null,
      }),
      loadApprovalDetail,
      respondToApproval: vi.fn().mockResolvedValue(undefined),
    });
    render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={agents} actions={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
    fireEvent.click(await screen.findByRole("button", { name: "Review Later action" }));
    const allowLater = await screen.findByRole("button", { name: "Allow Later action" });
    await waitFor(() => expect(allowLater).toBeEnabled());
    fireEvent.click(allowLater);

    await waitFor(() => expect(screen.queryByRole("button", { name: "Allow Later action" })).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Review First action" })).toBeVisible();
    expect(screen.getByRole("heading", { name: "Timeline" })).toHaveFocus();
    expect(api.loadApprovals).toHaveBeenCalledTimes(1);
  });

  it("refreshes disclosed approval pages by the new cursor chain without gaps or duplicates", async () => {
    const approval = (index: number) => ({
      id: `approval-${index}`, runId: "run-1", agentId: "root", provider: "codex" as const,
      agentPath: ["Root"], agentPathTruncated: false,
      operation: `Action ${index}`, scope: "One action", status: "pending" as const, responsePending: false,
    });
    const newest = (id: string) => ({ ...approval(-1), id, operation: `New ${id}` });
    const loadTimeline = vi.fn()
      .mockResolvedValueOnce({
        items: [], nextCursor: null, approvals: Array.from({ length: 30 }, (_, index) => approval(index)),
        approvalsTruncated: true, approvalsNextCursor: "old-page-2",
      })
      .mockResolvedValueOnce({
        items: [], nextCursor: null,
        approvals: [newest("new-1"), newest("new-2"), ...Array.from({ length: 28 }, (_, index) => approval(index))],
        approvalsTruncated: true, approvalsNextCursor: "new-page-2",
      });
    const loadApprovals = vi.fn(({ cursor }: { cursor: string | null }) => {
      if (cursor === "old-page-2") return Promise.resolve({ items: Array.from({ length: 30 }, (_, index) => approval(index + 30)), nextCursor: "old-page-3" });
      if (cursor === "new-page-2") return Promise.resolve({ items: Array.from({ length: 30 }, (_, index) => approval(index + 28)), nextCursor: "new-page-3" });
      if (cursor === "new-page-3") return Promise.resolve({ items: Array.from({ length: 30 }, (_, index) => approval(index + 58)), nextCursor: null });
      return Promise.resolve({ items: [], nextCursor: null });
    });
    const api = actions({ loadTimeline, loadApprovals });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={agents} actions={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
    await screen.findByRole("button", { name: "Review Action 59" });

    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={agents} actions={api} />);
    await waitFor(() => expect(loadApprovals.mock.calls.map(([request]) => request.cursor)).toEqual([
      "old-page-2", "new-page-2",
    ]));
    expect(screen.getByRole("button", { name: "Review Action 28" })).toBeVisible();
    expect(screen.getAllByRole("button", { name: "Review Action 30" })).toHaveLength(1);

    fireEvent.click(screen.getByRole("button", { name: "Load more approvals" }));
    expect(await screen.findByRole("button", { name: "Review Action 87" })).toBeVisible();
    expect(loadApprovals.mock.calls.map(([request]) => request.cursor)).toEqual([
      "old-page-2", "new-page-2", "new-page-3",
    ]);
    expect(screen.getAllByRole("button", { name: "Review Action 58" })).toHaveLength(1);
  });

  it("re-enables approval paging when an overlapping timeline refresh fails", async () => {
    const first = { id: "first", runId: "run-1", agentId: "root", provider: "codex" as const, agentPath: ["Root"], agentPathTruncated: false, operation: "First", scope: "One", status: "pending" as const, responsePending: false };
    const later = { ...first, id: "later", operation: "Later" };
    let resolvePage!: (value: { items: typeof later[]; nextCursor: string | null }) => void;
    const pendingPage = new Promise<{ items: typeof later[]; nextCursor: string | null }>((resolve) => { resolvePage = resolve; });
    const loadTimeline = vi.fn()
      .mockResolvedValueOnce({ items: [], nextCursor: null, approvals: [first], approvalsTruncated: true, approvalsNextCursor: "page-2" })
      .mockRejectedValueOnce(new Error("Timeline refresh unavailable"));
    const loadApprovals = vi.fn()
      .mockReturnValueOnce(pendingPage)
      .mockResolvedValueOnce({ items: [later], nextCursor: null });
    const api = actions({ loadTimeline, loadApprovals });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={agents} actions={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={agents} actions={api} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Timeline refresh unavailable");
    await act(async () => resolvePage({ items: [later], nextCursor: null }));

    const retry = await screen.findByRole("button", { name: "Load more approvals" });
    expect(retry).toBeEnabled();
    fireEvent.click(retry);
    expect(await screen.findByRole("button", { name: "Review Later" })).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("re-enables approval controls when a disclosed-page refresh fails", async () => {
    const first = { id: "first", runId: "run-1", agentId: "root", provider: "codex" as const, agentPath: ["Root"], agentPathTruncated: false, operation: "First", scope: "One", status: "pending" as const, responsePending: false };
    const second = { ...first, id: "second", operation: "Second" };
    const third = { ...first, id: "third", operation: "Third" };
    let resolveThird!: (value: { items: typeof third[]; nextCursor: string | null }) => void;
    const pendingThird = new Promise<{ items: typeof third[]; nextCursor: string | null }>((resolve) => { resolveThird = resolve; });
    const loadTimeline = vi.fn().mockResolvedValue({
      items: [], nextCursor: null, approvals: [first], approvalsTruncated: true, approvalsNextCursor: "page-2",
    });
    const loadApprovals = vi.fn(({ cursor }: { cursor: string | null }) => {
      if (cursor === "page-2" && loadApprovals.mock.calls.length === 1) return Promise.resolve({ items: [second], nextCursor: "page-3" });
      if (cursor === "page-3" && loadApprovals.mock.calls.length === 2) return pendingThird;
      if (cursor === "page-2" && loadApprovals.mock.calls.length === 3) return Promise.reject(new Error("Approval refresh unavailable"));
      if (cursor === "page-3") return Promise.resolve({ items: [third], nextCursor: null });
      return Promise.resolve({ items: [], nextCursor: null });
    });
    const api = actions({ loadTimeline, loadApprovals });
    const view = render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={agents} actions={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Load more approvals" }));
    await screen.findByRole("button", { name: "Review Second" });
    fireEvent.click(screen.getByRole("button", { name: "Load more approvals" }));
    view.rerender(<Timeline conversationId="conversation-1" refreshVersion={1} agents={agents} actions={api} />);
    await act(async () => resolveThird({ items: [third], nextCursor: null }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Approval refresh unavailable");

    const retry = screen.getByRole("button", { name: "Load more approvals" });
    expect(retry).toBeEnabled();
    fireEvent.click(retry);
    expect(await screen.findByRole("button", { name: "Review Third" })).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("uses exact approval-detail ancestry when the requesting agent is not loaded", async () => {
    const api = actions({
      loadTimeline: vi.fn().mockResolvedValue({
        items: [], nextCursor: null,
        approvals: [{ id: "deep", runId: "run-1", agentId: "unloaded", provider: "claude", agentPath: ["Root", "Orchestrator", "Reviewer"], agentPathTruncated: false, operation: "Edit deep file", scope: "One file", status: "pending", responsePending: false }],
        approvalsTruncated: false, approvalsNextCursor: null,
      }),
      loadApprovalDetail: vi.fn().mockResolvedValue({
        id: "deep", status: "pending", responsePending: false,
        agentPath: ["Root", "Orchestrator", "Reviewer"], agentPathTruncated: false,
        operation: "Edit deep file", scope: "One file", input: null, details: null,
        questionCount: 0, truncated: false,
      }),
    });
    render(<Timeline conversationId="conversation-1" refreshVersion={0} agents={agents.slice(0, 1)} actions={api} />);

    expect(await screen.findByText("Root/Orchestrator/Reviewer")).toBeVisible();
    expect(screen.queryByText(/^Agent$/)).not.toBeInTheDocument();
    expect(api.loadApprovalDetail).not.toHaveBeenCalled();
  });
});

function timelinePage(items: TimelineItem[], nextCursor: string | null) {
  return { items, nextCursor, approvals: [], approvalsTruncated: false, approvalsNextCursor: null };
}

function viewRow(activity: HTMLElement, id: string) {
  const row = [...activity.querySelectorAll<HTMLElement>("[data-timeline-id]")]
    .find(({ dataset }) => dataset.timelineId === id);
  if (!row) throw new Error(`Missing timeline row ${id}`);
  return row;
}
