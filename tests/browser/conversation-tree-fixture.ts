import type { AppEvent, ConversationPage, ConversationSummary, RunStatus, TimelineItem, TimelinePage } from "../../src/bridge/types";

// Invented browser-memory history only. No native bridge, provider, file, or timer.
export const treeScenario = new URLSearchParams(location.search).get("tree") === "nested";
export const treeIds = {
  root: "conversation-0",
  child: "recorded-reviewer",
  grandchild: "recorded-tests",
  empty: "recorded-empty",
  old: "recorded-old-child",
} as const;

const encoder = new TextEncoder();
let listener: ((event: AppEvent) => void) | null = null;
let eventSequence = 0;
let parentRun = 0;
let childRevision = 0;
const newChildRuns: number[] = [];
let rejectChildren = false;
let rootArchived = false;
const timelineReads: string[] = [];
const childPageReads: string[] = [];

export function listenToTreeEvents(handler: (event: AppEvent) => void) {
  listener = handler;
  return () => { if (listener === handler) listener = null; };
}

function invalidate(runId: string) {
  eventSequence += 1;
  listener?.({ kind: "runChanged", sequence: String(eventSequence), conversationId: treeIds.root, runId });
}

export function startNextSyntheticParentRun() {
  parentRun += 1;
  invalidate(`run-${parentRun}`);
}

export function addSyntheticChild() {
  newChildRuns.push(parentRun);
  invalidate(`run-${parentRun}`);
}

export function advanceSyntheticChildActivity() {
  childRevision += 1;
  invalidate("run-0");
}

export function failSyntheticChildPages(reject: boolean) { rejectChildren = reject; }
export function archiveSyntheticRoot() {
  rootArchived = true;
  eventSequence += 1;
  listener?.({ kind: "conversationChanged", sequence: String(eventSequence), conversationId: treeIds.root });
}
export function treeFixtureStats() {
  return { parentRun, childRevision, newChildren: newChildRuns.length, rejectChildren, rootArchived, timelineReads: [...timelineReads], childPageReads: [...childPageReads] };
}

function summary(id: string, title: string, parentId: string | null, hasChildren = false) {
  const managed = parentId === null;
  const runStatus: RunStatus = id === treeIds.root && parentRun > 0 ? "running" : "completed";
  return {
    id, title, parentId, hasChildren, summary: managed ? null : "Recorded synthetic work",
    capabilities: {
      canSend: managed, canInterrupt: managed, canArchive: managed, canRoute: managed,
      unavailableReason: managed ? null : "This provider exposes recorded child activity, not an independently controllable chat.",
    },
    routingProfile: "bestFit" as const, workspaceId: null, projectRoot: null,
    thinkingPreference: { kind: "auto" as const }, thinkingDecision: null, thinkingConfiguration: null,
    archived: id === treeIds.root && rootArchived,
    currentRunId: id === treeIds.root ? `run-${parentRun}`
      : id.startsWith("new-child-") ? `run-${newChildRuns[Number(id.slice("new-child-".length)) - 1]}`
      : id === treeIds.old ? "run-old" : "run-0",
    provider: "codex" as const, runStatus,
    rollupStatus: runStatus === "running" ? "active" as const : "completed" as const,
  };
}

function summaries() {
  return [
    summary(treeIds.root, "Synthetic conversation 0", null, true),
    summary("conversation-1", "Synthetic conversation 1", null),
    ...Array.from({ length: newChildRuns.length }, (_, index) => {
      const number = newChildRuns.length - index;
      return summary(`new-child-${number}`, `New synthetic child ${number}`, treeIds.root);
    }),
    summary(treeIds.child, "Synthetic child 0", treeIds.root, true),
    summary(treeIds.grandchild, "Synthetic test conversation", treeIds.child, true),
    summary(treeIds.empty, "No captured transcript", treeIds.root),
    ...Array.from({ length: 34 }, (_, index) => summary(`sibling-${index}`, `Recorded sibling ${index}`, treeIds.root)),
    summary(treeIds.old, "Completed earlier child", treeIds.root),
    ...Array.from({ length: 8 }, (_, index) => summary(
      `deep-${index}`, `Synthetic depth ${index + 4} with a deliberately long conversation title`,
      index === 0 ? treeIds.grandchild : `deep-${index - 1}`, index < 7,
    )),
  ];
}

export function treeLoadConversation({ conversationId }: { conversationId: string }): ConversationSummary {
  const found = summaries().find(({ id }) => id === conversationId);
  if (!found || (rootArchived && found.parentId !== null)) throw new Error("Synthetic conversation is unavailable.");
  return found;
}

export function treeListConversations(): ConversationPage {
  return { items: summaries().filter(({ parentId, archived }) => parentId === null && !archived), nextCursor: null };
}

export function treeListChildren({ parentId, cursor, limit }: { parentId: string; cursor: string | null; limit: number }): ConversationPage {
  childPageReads.push(parentId);
  if (childPageReads.length > 200) childPageReads.shift();
  if (rejectChildren) throw new Error("Synthetic child page failed. Try again.");
  const parent = treeLoadConversation({ conversationId: parentId });
  if (parent.archived || limit < 1 || limit > 200) throw new Error("Synthetic child page is unavailable.");
  const children = summaries().filter((child) => child.parentId === parentId);
  const prefix = `${parentId}-before-`;
  const start = cursor === null ? 0 : children.findIndex(({ id }) => `${prefix}${id}` === cursor) + 1;
  if (cursor !== null && start === 0) throw new Error("Wrong synthetic child cursor.");
  const items = children.slice(start, start + limit);
  return { items, nextCursor: start + items.length < children.length ? `${prefix}${items.at(-1)!.id}` : null };
}

export function treeConversationPath({ conversationId }: { conversationId: string }) {
  treeLoadConversation({ conversationId });
  const all = summaries();
  const items: ConversationSummary[] = [];
  let node = all.find(({ id }) => id === conversationId);
  while (node && items.length < 64) {
    items.unshift(node);
    node = all.find(({ id }) => id === node?.parentId);
  }
  return { items, truncated: node !== undefined, ownerConversationId: conversationId === "conversation-1" ? conversationId : treeIds.root };
}

export function treeTimeline(conversationId: string, cursor: string | null, limit: number): TimelinePage {
  const conversation = treeLoadConversation({ conversationId });
  if (conversation.archived) throw new Error("Synthetic conversation is archived.");
  timelineReads.push(conversationId);
  if (timelineReads.length > 200) timelineReads.shift();
  const prefix = `${conversationId}-before-`;
  if (cursor !== null && !cursor.startsWith(prefix)) throw new Error("Wrong synthetic conversation cursor.");
  const before = cursor === null ? Infinity : Number(cursor.slice(prefix.length));
  const count = conversationId === treeIds.root ? 160
    : conversationId === treeIds.child ? 85 + childRevision
    : conversationId === treeIds.empty ? 0 : 1;
  const label = conversationId === treeIds.root ? "Parent-only observation"
    : conversationId === treeIds.child ? "Captured child activity"
    : conversationId === treeIds.grandchild ? "Captured grandchild activity" : "Older recorded activity";
  const items = Array.from({ length: count }, (_, index): TimelineItem => {
    const content = `${label} ${index + 1}.\n\nThis is invented history for checking exact conversation ownership and reading position.`;
    return {
      id: `${conversationId}-event-${index + 1}`, conversationId,
      runId: conversationId === treeIds.root ? "run-0" : conversation.currentRunId!, agentId: `execution-${conversationId}`,
      sequence: String(index + 1), kind: "message", presentation: "normal", role: "assistant",
      content, contentBytes: String(encoder.encode(content).length), truncated: false,
      provider: "codex", operation: null,
    };
  }).filter(({ sequence }) => Number(sequence) < before);
  const page = items.slice(-limit);
  return {
    items: page, nextCursor: items.length > page.length ? `${prefix}${page[0].sequence}` : null,
    approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
  };
}
