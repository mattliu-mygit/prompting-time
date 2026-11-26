import { createEffect, createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import type { ApprovalDetailSnapshot, ApprovalSnapshot, CompactionActivity, ConversationSummary, ProviderId, TimelineItem } from "../../bridge/types";
import type { ConversationActions } from "../../app/store";
import { ApprovalCard } from "../inspector/ApprovalCard";
import { TimelineEntry } from "./TimelineEntry";

const PAGE_SIZE = 80;
const APPROVAL_PAGE_SIZE = 30;
const MAX_APPROVALS = APPROVAL_PAGE_SIZE * 4;
const MAX_OLDER_PAGES = 4;
const MAX_RETAINED_VIEWS = 10;

export type TimelineViewState = {
  newestItems: TimelineItem[];
  olderPages: TimelineItem[][];
  olderPageCursors: string[];
  cursor: string | null;
  newestCursor: string | null;
  historyEvicted: boolean;
  expandedGroups: ReadonlySet<string>;
  expandedOperations: ReadonlySet<string>;
  following: boolean;
  anchor: ScrollAnchor | null;
};

type TimelineProps = {
  /** One Timeline shows one conversation; the caller makes a new one for another conversation. */
  conversation: ConversationSummary;
  refreshVersion: number;
  actions: Pick<
    ConversationActions,
    "loadTimeline" | "loadEventDetail" | "loadApprovals" | "loadApprovalDetail" | "loadApprovalQuestions" | "respondToApproval"
  >;
  /** Reading positions of recently visited conversations, kept in memory for this session. */
  viewStates?: Map<string, TimelineViewState>;
};

const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };

/**
 * A conversation's activity: the newest 80 events plus up to four older pages loaded on request.
 * Each refresh re-reads the newest page and the retained older pages by their original cursors, so
 * an older tool call can finish without loading the whole conversation. Scrolling up stops
 * following new output; returning to a recent conversation restores its place.
 */
export function Timeline(props: TimelineProps) {
  const conversationId = untrack(() => props.conversation.id);
  const saved = untrack(() => props.viewStates?.get(conversationId));
  const [newestItems, setNewestItems] = createSignal<TimelineItem[]>(saved?.newestItems ?? []);
  const [olderPages, setOlderPages] = createSignal<TimelineItem[][]>(saved?.olderPages ?? []);
  const [cursor, setCursor] = createSignal<string | null>(saved?.cursor ?? null);
  const [historyEvicted, setHistoryEvicted] = createSignal(saved?.historyEvicted ?? false);
  const [approvals, setApprovals] = createSignal<ApprovalSnapshot[]>([]);
  const [approvalCursor, setApprovalCursor] = createSignal<string | null>(null);
  const [approvalHistoryEvicted, setApprovalHistoryEvicted] = createSignal(false);
  const [loadingApprovals, setLoadingApprovals] = createSignal(false);
  const [resolvedApprovalFocus, setResolvedApprovalFocus] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [loadingOlder, setLoadingOlder] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [compaction, setCompaction] = createSignal<CompactionActivity | null>(null);
  const [showJump, setShowJump] = createSignal(saved ? !saved.following : false);
  const [expandedGroups, setExpandedGroups] = createSignal<ReadonlySet<string>>(saved?.expandedGroups ?? new Set());
  const [expandedOperations, setExpandedOperations] = createSignal<ReadonlySet<string>>(saved?.expandedOperations ?? new Set());
  // Plain variables for what drives requests and scrolling but never shows on its own.
  let disposed = false;
  let loaded = saved !== undefined;
  let historyGeneration = 0;
  let refreshQueued = false;
  let refreshing = false;
  let readTail: Promise<unknown> = Promise.resolve();
  let newestRetained = saved?.newestItems ?? [];
  let olderRetained = saved?.olderPages ?? [];
  let olderCursors = saved?.olderPageCursors ?? [];
  let newestCursor = saved?.newestCursor ?? null;
  let following = saved?.following ?? true;
  let lastScrollTop = 0;
  let prependAnchor: ScrollAnchor | null = saved && !saved.following ? saved.anchor : null;
  let scrollToEnd = saved?.following ?? false;
  let operationFocus: { id: string; control: HTMLElement } | null = null;
  let scrollBox: HTMLDivElement | undefined;
  let heading: HTMLHeadingElement | undefined;
  // Pending requests: up to four pages of 30, each read from its cursor. A refresh re-reads the
  // pages shown by following the refreshed cursor chain; paging has its own busy token, so an
  // overlapping refresh cannot leave the paging button disabled.
  let approvalGeneration = 0;
  let approvalPagerToken = 0;
  let approvalPagerInFlight = false;
  let approvalPageCursors: Array<string | null> = [];
  let approvalPages: ApprovalSnapshot[][] = [];
  let approvalReadTail: Promise<unknown> = Promise.resolve();

  const items = createMemo(() => mergeTimeline(olderPages().flat(), newestItems()).filter((item) => item.presentation !== "telemetry"));
  const groups = createMemo(() => groupActivity(items()));
  const active = () => ["queued", "running", "waiting"].includes(props.conversation.runStatus ?? "");
  const compacting = () => active() && compaction()?.runId === props.conversation.currentRunId;

  function rememberOperationFocus() {
    const control = document.activeElement;
    if (!(control instanceof HTMLElement) || !control.matches("[data-operation-disclosure]")) return;
    const id = control.closest<HTMLElement>("[data-timeline-id]")?.dataset.timelineId;
    if (id && scrollBox?.contains(control)) operationFocus = { id, control };
  }

  // Reads run one at a time, so a page never lands before the one asked for earlier.
  function loadPage(pageCursor: string | null) {
    const result = readTail.catch(() => undefined).then(() => props.actions.loadTimeline({ conversationId, cursor: pageCursor, limit: PAGE_SIZE }));
    readTail = result.catch(() => undefined);
    return result;
  }

  function loadApprovalPage(pageCursor: string | null) {
    const result = approvalReadTail.catch(() => undefined).then(() => props.actions.loadApprovals({ conversationId, cursor: pageCursor, limit: APPROVAL_PAGE_SIZE, kind: "pending" }));
    approvalReadTail = result.catch(() => undefined);
    return result;
  }

  function resetApprovals() {
    approvalGeneration += 1;
    approvalPagerToken += 1;
    approvalPagerInFlight = false;
    approvalPageCursors = [];
    approvalPages = [];
    setApprovalCursor(null);
    setApprovals([]);
    setApprovalHistoryEvicted(false);
    setLoadingApprovals(false);
  }

  function requestRefresh() {
    refreshQueued = true;
    if (refreshing) return;
    refreshing = true;
    void (async () => {
      while (refreshQueued && !disposed) {
        refreshQueued = false;
        const replacing = !loaded;
        const approvalRead = ++approvalGeneration;
        try {
          const page = await loadPage(null);
          if (disposed) return;
          setCompaction(page.activeCompaction);
          const box = scrollBox ?? null;
          scrollToEnd = replacing || box === null || (following && box.scrollHeight - box.scrollTop - box.clientHeight < 32);
          if (!scrollToEnd && !prependAnchor) prependAnchor = captureScrollAnchor(box);
          loaded = true;
          const bounded = boundTimelinePage(page.items);
          if (!replacing && newestRetained.length > 0) {
            const incoming = new Set(bounded.map(({ id }) => id));
            if (newestRetained.some(({ id }) => !incoming.has(id))) setHistoryEvicted(true);
          }
          newestRetained = bounded;
          rememberOperationFocus();
          setNewestItems(bounded);
          newestCursor = page.nextCursor;
          if (replacing || olderRetained.length === 0) setCursor(page.nextCursor);
          // A tool can finish outside the newest page: re-read the retained older pages by their
          // original cursors, keeping the reader's position.
          const generation = historyGeneration;
          for (const retainedCursor of [...olderCursors]) {
            const retained = await loadPage(retainedCursor).catch((reason) => {
              if (!disposed && generation === historyGeneration) setError(messageFor(reason));
              return null;
            });
            if (!retained || disposed || generation !== historyGeneration) break;
            const index = olderCursors.indexOf(retainedCursor);
            if (index === -1) continue;
            if (!following && !prependAnchor) prependAnchor = captureScrollAnchor(scrollBox ?? null);
            olderRetained = olderRetained.map((pageItems, pageIndex) => (pageIndex === index ? boundTimelinePage(retained.items) : pageItems));
            rememberOperationFocus();
            setOlderPages(olderRetained);
          }
          // A failed history refresh above must not hold back fresh requests for a response.
          if (disposed) return;
          const refreshedPages = [pendingApprovalPage(page.approvals)];
          const refreshedCursors: Array<string | null> = [null];
          let refreshedCursor = page.approvalsNextCursor;
          for (let pageIndex = 0; pageIndex < approvalPages.length - 1; pageIndex += 1) {
            const disclosedCursor = refreshedCursor;
            if (!disclosedCursor || refreshedPages.length >= MAX_OLDER_PAGES) break;
            const disclosed = await loadApprovalPage(disclosedCursor);
            if (disposed || approvalRead !== approvalGeneration) break;
            refreshedPages.push(pendingApprovalPage(disclosed.items));
            refreshedCursors.push(disclosedCursor);
            refreshedCursor = disclosed.nextCursor;
            if (!disclosed.nextCursor) break;
          }
          if (disposed || approvalRead !== approvalGeneration) continue;
          const reconciled = mergeApprovals(refreshedPages.flat());
          const focusedApprovalId = document.activeElement?.closest<HTMLElement>("[data-approval-id]")?.dataset.approvalId;
          if (focusedApprovalId && !reconciled.some(({ id }) => id === focusedApprovalId)) setResolvedApprovalFocus(focusedApprovalId);
          setApprovals(reconciled);
          approvalPages = refreshedPages;
          approvalPageCursors = refreshedCursors;
          setApprovalHistoryEvicted(false);
          setApprovalCursor(refreshedCursor);
        } catch (reason) {
          if (!disposed) {
            setCompaction(null);
            setError(messageFor(reason));
          }
        }
      }
      refreshing = false;
      if (!disposed) setLoading(false);
    })();
  }

  // Effects run on every change of what they read, so read the version through a memo, which
  // only changes when the number does.
  const refreshVersion = createMemo(() => props.refreshVersion);
  createEffect(refreshVersion, () => {
    setError(null);
    requestRefresh();
  });

  // A run that ended has no pending requests to answer; drop the pages until the next read.
  const runStatus = createMemo(() => props.conversation.runStatus);
  createEffect(runStatus, (status) => {
    if (status === "completed" || status === "interrupted" || status === "failed") resetApprovals();
  });

  // A request that was answered while focus was inside it hands focus to the timeline heading.
  createEffect(
    () => ({ resolved: resolvedApprovalFocus(), approvals: approvals() }),
    ({ resolved, approvals: current }) => {
      if (!resolved || current.some(({ id }) => id === resolved)) return;
      heading?.focus();
      setResolvedApprovalFocus(null);
    },
  );

  // After rows change: keep focus on a disclosure that was redrawn, then hold the reading position.
  createEffect(items, () => {
    const box = scrollBox;
    if (!box) return;
    const focused = operationFocus;
    operationFocus = null;
    if (focused && !focused.control.isConnected && document.activeElement === document.body) {
      const replacement = [...box.querySelectorAll<HTMLButtonElement>("[data-operation-disclosure]")]
        .find((button) => button.closest<HTMLElement>("[data-timeline-id]")?.dataset.timelineId === focused.id);
      replacement?.focus({ preventScroll: true });
    }
    if (prependAnchor !== null) {
      restoreScrollAnchor(box, prependAnchor);
      lastScrollTop = box.scrollTop;
      prependAnchor = null;
    } else if (scrollToEnd) {
      box.scrollTop = box.scrollHeight;
      lastScrollTop = box.scrollTop;
      scrollToEnd = false;
    }
  });

  onSettled(() => () => {
    disposed = true;
    if (!props.viewStates || !loaded) return;
    const retainedIds = new Set([...newestRetained, ...olderRetained.flat()].map(({ id }) => id));
    props.viewStates.delete(conversationId);
    props.viewStates.set(conversationId, {
      newestItems: newestRetained,
      olderPages: olderRetained,
      olderPageCursors: [...olderCursors],
      cursor: cursor(),
      newestCursor,
      historyEvicted: historyEvicted(),
      expandedGroups: new Set([...expandedGroups()].filter((id) => retainedIds.has(id))),
      expandedOperations: new Set([...expandedOperations()].filter((id) => retainedIds.has(id))),
      following,
      anchor: captureScrollAnchor(scrollBox ?? null),
    });
    while (props.viewStates.size > MAX_RETAINED_VIEWS) props.viewStates.delete(props.viewStates.keys().next().value!);
  });

  function onScroll() {
    const box = scrollBox;
    if (!box) return;
    const movedUp = box.scrollTop < lastScrollTop;
    const atBottom = box.scrollHeight - box.scrollTop - box.clientHeight <= 2;
    // A larger viewport can clamp scrollTop downward while already at the end.
    if (atBottom) following = true;
    else if (movedUp) following = false;
    lastScrollTop = box.scrollTop;
    setShowJump(!following);
  }

  function jumpToLatest() {
    following = true;
    setShowJump(false);
    if (scrollBox) {
      scrollBox.scrollTop = scrollBox.scrollHeight;
      lastScrollTop = scrollBox.scrollTop;
    }
  }

  function toggleGroup(group: TimelineItem[]) {
    const expanded = group.some(({ id }) => expandedGroups().has(id) || expandedOperations().has(id));
    if (expanded) setExpandedOperations((current) => new Set([...current].filter((id) => !group.some((item) => item.id === id))));
    setExpandedGroups((current) => {
      const next = new Set(current);
      if (expanded) group.forEach(({ id }) => next.delete(id));
      else next.add(group[0]!.id);
      return next;
    });
  }

  function toggleOperation(id: string) {
    if (expandedOperations().has(id) && groups().some((group) => isGroupActivity(group[0]!) && group.some((item) => item.id === id))) {
      setExpandedGroups((current) => new Set([...current, id]));
    }
    setExpandedOperations((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function loadOlder() {
    const requested = cursor();
    if (!requested || loadingOlder()) return;
    const generation = ++historyGeneration;
    // Paging invalidates a running pass over the retained pages; run it again afterwards.
    if (refreshing) refreshQueued = true;
    setLoadingOlder(true);
    setError(null);
    try {
      const page = await loadPage(requested);
      if (disposed || generation !== historyGeneration) return;
      prependAnchor = captureScrollAnchor(scrollBox ?? null);
      olderCursors = [...olderCursors, requested].slice(-MAX_OLDER_PAGES);
      const next = [...olderRetained, boundTimelinePage(page.items)];
      olderRetained = next.slice(-MAX_OLDER_PAGES);
      if (next.length > MAX_OLDER_PAGES) setHistoryEvicted(true);
      rememberOperationFocus();
      setOlderPages(olderRetained);
      setCursor(page.nextCursor);
    } catch (reason) {
      if (generation !== historyGeneration) return;
      prependAnchor = null;
      setError(messageFor(reason));
    } finally {
      if (!disposed) setLoadingOlder(false);
    }
  }

  async function loadMoreApprovals() {
    const requested = approvalCursor();
    if (!requested || approvalPagerInFlight) return;
    const generation = approvalGeneration;
    const pagerToken = ++approvalPagerToken;
    approvalPagerInFlight = true;
    setLoadingApprovals(true);
    setError(null);
    try {
      const page = await loadApprovalPage(requested);
      if (disposed || generation !== approvalGeneration || approvalCursor() !== requested) return;
      const cursors = [...approvalPageCursors, requested];
      approvalPageCursors = cursors.slice(-MAX_OLDER_PAGES);
      approvalPages = [...approvalPages, pendingApprovalPage(page.items)].slice(-MAX_OLDER_PAGES);
      if (cursors.length > MAX_OLDER_PAGES) setApprovalHistoryEvicted(true);
      setApprovals(mergeApprovals(approvalPages.flat()).slice(0, MAX_APPROVALS));
      setApprovalCursor(page.nextCursor);
    } catch (reason) {
      if (!disposed && generation === approvalGeneration) setError(messageFor(reason));
    } finally {
      if (!disposed && pagerToken === approvalPagerToken) {
        approvalPagerInFlight = false;
        setLoadingApprovals(false);
      }
    }
  }

  async function reloadNewestApprovals() {
    if (approvalPagerInFlight) return;
    const generation = ++approvalGeneration;
    const pagerToken = ++approvalPagerToken;
    approvalPagerInFlight = true;
    setLoadingApprovals(true);
    setError(null);
    try {
      const page = await loadApprovalPage(null);
      if (disposed || generation !== approvalGeneration) return;
      approvalPageCursors = [null];
      approvalPages = [pendingApprovalPage(page.items)];
      setApprovals(approvalPages[0]!);
      setApprovalCursor(page.nextCursor);
      setApprovalHistoryEvicted(false);
    } catch (reason) {
      if (!disposed && generation === approvalGeneration) setError(messageFor(reason));
    } finally {
      if (!disposed && pagerToken === approvalPagerToken) {
        approvalPagerInFlight = false;
        setLoadingApprovals(false);
      }
    }
  }

  /** Applies a request's exact durable state: still pending updates it; anything else removes it. */
  async function reconcileApproval(approvalId: string, known?: ApprovalDetailSnapshot) {
    const exact = known ?? (await props.actions.loadApprovalDetail({ approvalId }));
    const apply = (list: ApprovalSnapshot[]) => (exact.status === "pending"
      ? list.map((approval) => (approval.id === approvalId ? { ...approval, status: exact.status, responsePending: exact.responsePending } : approval))
      : list.filter(({ id }) => id !== approvalId));
    approvalPages = approvalPages.map(apply);
    setApprovals(apply);
    if (exact.status !== "pending") setResolvedApprovalFocus(approvalId);
    return exact;
  }

  function reloadNewestHistory() {
    historyGeneration += 1;
    prependAnchor = null;
    olderRetained = [];
    olderCursors = [];
    setOlderPages([]);
    setCursor(newestCursor);
    setHistoryEvicted(false);
    scrollToEnd = true;
    following = true;
    setShowJump(false);
  }

  return (
    <section class="timeline-region" aria-labelledby="timeline-heading">
      <h2 ref={heading} class="pt-visually-hidden" id="timeline-heading" tabindex={-1}>Timeline</h2>
      <Show when={loading()}><span role="status" class="timeline-status">Loading activity…</span></Show>
      <Show when={compacting()}><span role="status" class="timeline-status compaction-status">Compacting…</span></Show>
      <Show when={error()}><p role="alert" class="inline-error">{error()}</p></Show>
      <div ref={scrollBox} class="timeline-scroll" role="region" tabindex={0} aria-label="Conversation activity" onScroll={onScroll}>
        <div class="timeline-column">
          <Show when={historyEvicted()}>
            <div class="history-window-note" role="note">
              <span>Some history is outside this bounded view.</span>
              <button type="button" class="pt-button" onClick={reloadNewestHistory}>Reload newest history</button>
            </div>
          </Show>
          <Show when={cursor()}>
            <button type="button" class="pt-button load-older" disabled={loadingOlder()} onClick={() => void loadOlder()}>
              {loadingOlder() ? "Loading older activity…" : "Load older activity"}
            </button>
          </Show>
          <Show when={!loading() && items().length === 0}>
            <p class="empty-note">{props.conversation.capabilities.canSend ? "No activity yet." : "No recorded activity is available."}</p>
          </Show>
          <ol class="timeline-list">
            <For each={groups()} keyed={(group) => (isGroupActivity(group[0]!) ? `group:${group.find(({ id }) => expandedGroups().has(id) || expandedOperations().has(id))?.id ?? group[0]!.id}` : group[0]!.id)}>
              {(group) => (
                <Show
                  when={isGroupActivity(group()[0]!)}
                  fallback={
                    <li data-timeline-id={group()[0]!.id}>
                      <TimelineEntry item={group()[0]!} actions={props.actions} expanded={expandedOperations().has(group()[0]!.id)} onToggle={() => toggleOperation(group()[0]!.id)} agentPath={props.conversation.title} />
                    </li>
                  }
                >
                  <ActivityGroup
                    group={group()}
                    conversationTitle={props.conversation.title}
                    expanded={group().some(({ id }) => expandedGroups().has(id) || expandedOperations().has(id))}
                    expandedOperations={expandedOperations()}
                    actions={props.actions}
                    onToggle={() => toggleGroup(group())}
                    onToggleOperation={toggleOperation}
                  />
                </Show>
              )}
            </For>
          </ol>
          <Show when={approvals().length > 0}>
            <section class="approval-list" aria-labelledby="approvals-heading">
              <h3 id="approvals-heading">Needs your response</h3>
              <For each={approvals()} keyed={(approval) => approval.id}>
                {(approval) => (
                  <ApprovalCard approval={approval()} actions={props.actions} onReconcile={(detail) => reconcileApproval(approval().id, detail)} />
                )}
              </For>
              <Show when={approvalCursor()}>
                <button type="button" class="pt-button" disabled={loadingApprovals()} onClick={() => void loadMoreApprovals()}>
                  {loadingApprovals() ? "Loading approvals…" : "Load more approvals"}
                </button>
              </Show>
              <Show when={approvalHistoryEvicted()}>
                <div class="history-window-note" role="note">
                  <span>Newer approvals are outside this bounded view.</span>
                  <button type="button" class="pt-button" disabled={loadingApprovals()} onClick={() => void reloadNewestApprovals()}>
                    Reload newest approvals
                  </button>
                </div>
              </Show>
            </section>
          </Show>
        </div>
      </div>
      <Show when={showJump()}>
        <button type="button" class="pt-button jump-to-latest" onClick={jumpToLatest}>Jump to latest</button>
      </Show>
    </section>
  );
}

function ActivityGroup(props: {
  group: TimelineItem[];
  conversationTitle: string;
  expanded: boolean;
  expandedOperations: ReadonlySet<string>;
  actions: Pick<ConversationActions, "loadEventDetail">;
  onToggle(): void;
  onToggleOperation(id: string): void;
}) {
  const first = () => props.group[0]!;
  const completed = () => first().operation !== null;
  const label = () => (completed()
    ? `${props.group.length} completed ${props.group.length === 1 ? "operation" : "operations"}`
    : props.group.some(({ kind }) => kind === "tool") ? "tool activity" : "progress");
  const attribution = () => `${providerNames[first().provider]} · ${props.conversationTitle}`;
  return (
    <li class="activity-group">
      <button type="button" class="activity-toggle" data-timeline-id={props.expanded ? undefined : first().id} aria-expanded={props.expanded ? "true" : "false"} aria-label={`${props.expanded ? "Hide" : "Show"} ${label()} · ${attribution()}`} onClick={() => props.onToggle()}>
        <span aria-hidden="true">{props.expanded ? "▾" : "▸"}</span>
        <span>{completed() ? label() : label() === "tool activity" ? "Tool activity" : "Progress"}</span>
        <small>{attribution()}</small>
      </button>
      <Show when={props.expanded}>
        <p class="activity-fragment-note">Activity in the loaded history.</p>
        <ol class="activity-entries">
          <For each={props.group} keyed={(item) => item.id}>
            {(item) => (
              <li data-timeline-id={item().id}>
                <TimelineEntry item={item()} actions={props.actions} grouped expanded={props.expandedOperations.has(item().id)} onToggle={() => props.onToggleOperation(item().id)} agentPath={props.conversationTitle} />
              </li>
            )}
          </For>
        </ol>
      </Show>
    </li>
  );
}

function mergeApprovals(incoming: readonly ApprovalSnapshot[]) {
  const byId = new Map<string, ApprovalSnapshot>();
  incoming.forEach((approval) => byId.set(approval.id, approval));
  return [...byId.values()].filter(({ status }) => status === "pending");
}

function pendingApprovalPage(items: readonly ApprovalSnapshot[]) {
  return items.filter(({ status }) => status === "pending").slice(0, APPROVAL_PAGE_SIZE);
}

function isGroupActivity(item: TimelineItem) {
  if (item.operation) return item.presentation === "normal" && item.operation.status === "succeeded" && !item.operation.conflicted;
  return item.presentation === "normal" && (item.kind === "tool" || item.kind === "progress");
}

function groupActivity(items: readonly TimelineItem[]) {
  const groups: TimelineItem[][] = [];
  for (const item of items) {
    const previous = groups.at(-1);
    const first = previous?.[0];
    if (first && isGroupActivity(first) && isGroupActivity(item)
      && Boolean(first.operation) === Boolean(item.operation)
      && first.runId === item.runId && first.agentId === item.agentId && first.provider === item.provider) previous!.push(item);
    else groups.push([item]);
  }
  return groups;
}

function mergeTimeline(current: readonly TimelineItem[], incoming: readonly TimelineItem[]): TimelineItem[] {
  const byId = new Map(current.map((item) => [item.id, item]));
  incoming.forEach((item) => byId.set(item.id, item));
  return [...byId.values()].sort((left, right) => compareSequence(left.sequence, right.sequence));
}

function boundTimelinePage(items: readonly TimelineItem[]) {
  return mergeTimeline([], items).slice(-PAGE_SIZE);
}

type ScrollAnchor = { id: string | null; viewportOffset: number; scrollHeight: number };

function captureScrollAnchor(box: HTMLDivElement | null): ScrollAnchor | null {
  if (!box) return null;
  const rows = [...box.querySelectorAll<HTMLElement>("[data-timeline-id]")];
  const first = rows.find((row) => rowTop(box, row) + row.offsetHeight > box.scrollTop) ?? rows.at(-1) ?? null;
  return { id: first?.dataset.timelineId ?? null, viewportOffset: (first ? rowTop(box, first) : 0) - box.scrollTop, scrollHeight: box.scrollHeight };
}

function restoreScrollAnchor(box: HTMLDivElement, anchor: ScrollAnchor) {
  const matched = anchor.id ? [...box.querySelectorAll<HTMLElement>("[data-timeline-id]")].find(({ dataset }) => dataset.timelineId === anchor.id) : null;
  if (matched) box.scrollTop = rowTop(box, matched) - anchor.viewportOffset;
  else box.scrollTop += box.scrollHeight - anchor.scrollHeight;
}

function rowTop(box: HTMLDivElement, row: HTMLElement) {
  // Rectangles handle nested activity lists and offset parents outside the scroller.
  if (box.getClientRects().length) return row.getBoundingClientRect().top - box.getBoundingClientRect().top - box.clientTop + box.scrollTop;
  return row.offsetTop;
}

function compareSequence(left: string, right: string) {
  const a = BigInt(left);
  const b = BigInt(right);
  return a < b ? -1 : a > b ? 1 : 0;
}

function messageFor(reason: unknown) {
  return reason instanceof Error ? reason.message : "Prompting Time could not load this activity.";
}
