import type { AppEvent, EventDetailSnapshot, ProviderId, TimelineItem, TimelinePage } from "../../src/bridge/types";

// Invented browser-memory operations. No CLI, native bridge, files, or timers.
const query = new URLSearchParams(location.search);
export const toolScenario = query.get("chat") === "tools" || query.get("chat") === "tool-history";
export const toolProvider: ProviderId = query.get("tool-provider") === "claude" ? "claude" : "codex";
let revision = 0;
let listener: ((event: AppEvent) => void) | null = null;
let detailReads = 0;
const pageReads: Array<string | null> = [];
let rejectDetail = false;
const encoder = new TextEncoder();
type OperationDetail = NonNullable<EventDetailSnapshot["operation"]>;

export function listenToToolEvents(handler: (event: AppEvent) => void) {
  listener = handler;
  return () => { if (listener === handler) listener = null; };
}

export function advanceToolOperation() {
  revision += 1;
  listener?.({ kind: "conversationChanged", sequence: String(revision), conversationId: "conversation-0" });
  return revision;
}

export function failToolDetails(reject: boolean) { rejectDetail = reject; }
export function toolFixtureStats() { return { revision, detailReads, pageReads: [...pageReads] }; }

function message(conversationId: string, sequence: number, content: string): TimelineItem {
  const owner = conversationId.replace("conversation-", "");
  return {
    id: `${conversationId}-operation-${sequence}`, conversationId, runId: `run-${owner}`, agentId: `root-${owner}`,
    sequence: String(sequence), kind: "message", role: "assistant", provider: toolProvider,
    presentation: "normal", content, contentBytes: String(encoder.encode(content).length), truncated: false, operation: null,
  };
}

function operation(conversationId: string, sequence: number, title: string, detail: Partial<OperationDetail> = {}): { item: TimelineItem; detail: EventDetailSnapshot } {
  const payload: OperationDetail = {
    title, status: "succeeded", input: null, output: null, error: null, context: null,
    durationMs: null, exitCode: null, truncated: false, conflicted: false, detailRevision: "1",
    ...detail,
  };
  const row: TimelineItem = {
    ...message(conversationId, sequence, title), kind: "tool", role: null,
    presentation: payload.status === "failed" ? "failure" : payload.conflicted ? "notice" : "normal",
    operation: {
      status: payload.status,
      hasDetails: [payload.input, payload.output, payload.error, payload.context].some((value) => value !== null),
      detailRevision: payload.detailRevision, durationMs: payload.durationMs, exitCode: payload.exitCode,
      truncated: payload.truncated, conflicted: payload.conflicted,
    },
  };
  return {
    item: row,
    detail: { id: row.id, content: title, contentBytes: row.contentBytes, truncated: payload.truncated, operation: payload },
  };
}

function liveOperation(conversationId: string, sequence: number) {
  const current = conversationId === "conversation-0" ? revision : 0;
  return operation(conversationId, sequence, "Run cargo test", {
    status: current === 0 ? "running" : "succeeded", input: "cargo test", context: "synthetic-project",
    output: current === 0 ? null : `${current + 2} tests passed\n`,
    durationMs: current === 0 ? null : "2400", exitCode: current === 0 ? null : 0,
    detailRevision: String(current + 1),
  });
}

function shortOperations(conversationId: string) {
  return [
    operation(conversationId, 3, "Read src/parser.rs", { input: "src/parser.rs", output: "fn parse() {}\n" }),
    operation(conversationId, 4, "Search \"routing\"", { input: "routing", output: "src/router.rs:12: route\n" }),
    liveOperation(conversationId, 5),
    operation(conversationId, 6, "Run synthetic-check", {
      status: "failed", input: "synthetic-check", error: "Synthetic check failed: expected a nonempty name.", durationMs: "120", exitCode: 1,
    }),
    operation(conversationId, 7, "Run quiet-command", { input: "quiet-command", output: "", exitCode: 0 }),
    operation(conversationId, 8, "Unknown tool", { status: "unknown" }),
    operation(conversationId, 9, "Run long-output", {
      input: "long-output", output: query.get("tool-output") === "discarded" ? "" : "Invented bounded output.\n".repeat(250), truncated: true,
    }),
  ];
}

export function toolTimeline(conversationId: string, cursor: string | null, limit: number): TimelinePage {
  pageReads.push(cursor);
  if (pageReads.length > 32) pageReads.shift();
  let items: TimelineItem[];
  if (query.get("chat") === "tool-history") {
    const count = 180 + (conversationId === "conversation-0" ? revision : 0);
    items = Array.from({ length: count }, (_, index) => index === 69
      ? liveOperation(conversationId, index + 1).item
      : message(conversationId, index + 1, `Observation ${index + 1}. ${"Synthetic history for a stable reading position. ".repeat(3)}`));
  } else {
    const legacy = { ...message(conversationId, 10, "commandExecution: completed"), kind: "tool" as const, role: null };
    items = [
      { ...message(conversationId, 1, "Inspect the parser and run its tests."), role: "user" },
      message(conversationId, 2, "I’ll inspect the relevant files and check the tests."),
      ...shortOperations(conversationId).map(({ item }) => item),
      legacy,
      message(conversationId, 11, "This is invented activity for checking the interface, not real execution."),
    ];
  }
  const before = cursor === null ? null : /^tool-before-(\d+)$/.exec(cursor);
  if (cursor !== null && !before) throw new Error("Unknown synthetic history cursor.");
  const eligible = before ? items.filter(({ sequence }) => Number(sequence) < Number(before[1])) : items;
  const page = eligible.slice(-limit);
  return {
    items: page, nextCursor: eligible.length > page.length ? `tool-before-${page[0].sequence}` : null,
    approvals: [], approvalsTruncated: false, approvalsNextCursor: null, activeCompaction: null,
  };
}

export function toolEventDetail(eventId: string): EventDetailSnapshot {
  detailReads += 1;
  if (rejectDetail) throw new Error("Synthetic detail request failed. Try again.");
  const match = /^(conversation-\d+)-operation-(\d+)$/.exec(eventId);
  if (!match) throw new Error("Unknown synthetic operation.");
  const [, conversationId, sequence] = match;
  const entries = query.get("chat") === "tool-history" ? [liveOperation(conversationId, 70)] : shortOperations(conversationId);
  const entry = entries.find(({ item }) => item.sequence === sequence);
  if (!entry) throw new Error("Details weren't captured.");
  return entry.detail;
}
