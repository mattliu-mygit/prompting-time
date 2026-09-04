import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type {
  AgentSnapshot,
  ApprovalDetailSnapshot,
  ApprovalSnapshot,
  ProviderId,
  TimelineItem,
} from "../../bridge/types";
import type { AgentWindowSnapshot, ConversationActions } from "../../app/store";
import { ApprovalCard } from "../inspector/ApprovalCard";

const PAGE_SIZE = 80;
const APPROVAL_PAGE_SIZE = 30;
const MAX_APPROVALS = APPROVAL_PAGE_SIZE * 4;
const MAX_OLDER_PAGES = 4;
const AGENT_PAGE_SIZE = 20;

type TimelineProps = {
  conversationId: string;
  refreshVersion: number;
  agents: readonly AgentSnapshot[];
  agentsTruncated?: boolean;
  agentWindow?: AgentWindowSnapshot | null;
  onLoadAgentPage?(restart: boolean): void;
  actions: ConversationActions;
};

const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
const statusNames: Record<AgentSnapshot["status"], string> = {
  queued: "Queued",
  running: "Running",
  waiting: "Waiting",
  completed: "Completed",
  interrupted: "Interrupted",
  failed: "Failed",
};

export function Timeline({ conversationId, refreshVersion, agents, agentsTruncated = false, agentWindow = null, onLoadAgentPage = () => {}, actions }: TimelineProps) {
  const [newestItems, setNewestItems] = useState<TimelineItem[]>([]);
  const [olderPages, setOlderPages] = useState<TimelineItem[][]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [historyEvicted, setHistoryEvicted] = useState(false);
  const [approvals, setApprovals] = useState<ApprovalSnapshot[]>([]);
  const [approvalCursor, setApprovalCursor] = useState<string | null>(null);
  const [approvalHistoryEvicted, setApprovalHistoryEvicted] = useState(false);
  const [loadingApprovals, setLoadingApprovals] = useState(false);
  const [resolvedApprovalFocus, setResolvedApprovalFocus] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestGeneration = useRef(0);
  const historyRequestGeneration = useRef(0);
  const loadedConversation = useRef<string | null>(null);
  const requestedConversation = useRef<string | null>(null);
  const newestRefreshQueued = useRef(false);
  const newestRefreshInFlight = useRef(false);
  const approvalRequestGeneration = useRef(0);
  const approvalPagerToken = useRef(0);
  const approvalPagerInFlight = useRef(false);
  const approvalCursorRef = useRef<string | null>(null);
  const approvalPageCursors = useRef<Array<string | null>>([]);
  const approvalPages = useRef<ApprovalSnapshot[][]>([]);
  const approvalReadTail = useRef<Promise<void>>(Promise.resolve());
  const readTail = useRef<Promise<void>>(Promise.resolve());
  const actionsRef = useRef(actions);
  actionsRef.current = actions;
  const olderPagesRef = useRef<TimelineItem[][]>([]);
  const newestItemsRef = useRef<TimelineItem[]>([]);
  const newestCursor = useRef<string | null>(null);
  const scrollBox = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const prependAnchor = useRef<ScrollAnchor | null>(null);
  const scrollToEnd = useRef(false);
  const rootStatus = agents.find(({ parentId }) => parentId === null)?.status;

  useEffect(() => {
    if (rootStatus !== "completed" && rootStatus !== "interrupted" && rootStatus !== "failed") return;
    approvalRequestGeneration.current += 1;
    approvalPagerToken.current += 1;
    approvalPagerInFlight.current = false;
    approvalCursorRef.current = null;
    approvalPageCursors.current = [];
    approvalPages.current = [];
    setApprovalCursor(null);
    setApprovals([]);
    setApprovalHistoryEvicted(false);
    setLoadingApprovals(false);
  }, [rootStatus]);

  const items = useMemo(
    () => mergeTimeline(olderPages.flat(), newestItems),
    [newestItems, olderPages],
  );

  function loadTimelinePage(request: { conversationId: string; cursor: string | null; limit: number }) {
    const result = readTail.current
      .catch(() => undefined)
      .then(() => actionsRef.current.loadTimeline(request));
    readTail.current = result.then(() => undefined, () => undefined);
    return result;
  }

  function loadApprovalPage(request: { conversationId: string; cursor: string | null; limit: number; kind: "pending" }) {
    const result = approvalReadTail.current
      .catch(() => undefined)
      .then(() => actionsRef.current.loadApprovals(request));
    approvalReadTail.current = result.then(() => undefined, () => undefined);
    return result;
  }

  useEffect(() => {
    const changesConversation = requestedConversation.current !== conversationId;
    requestedConversation.current = conversationId;
    if (changesConversation) {
      requestGeneration.current += 1;
      historyRequestGeneration.current += 1;
      setNewestItems([]);
      newestItemsRef.current = [];
      setOlderPages([]);
      olderPagesRef.current = [];
      setApprovals([]);
      setResolvedApprovalFocus(null);
      setCursor(null);
      newestCursor.current = null;
      setHistoryEvicted(false);
      setApprovalCursor(null);
      approvalCursorRef.current = null;
      approvalPageCursors.current = [];
      approvalPages.current = [];
      approvalRequestGeneration.current += 1;
      approvalPagerToken.current += 1;
      approvalPagerInFlight.current = false;
      setApprovalHistoryEvicted(false);
      setLoadingApprovals(false);
      setLoading(true);
    }
    setError(null);
    newestRefreshQueued.current = true;
    const drainNewest = () => {
      if (newestRefreshInFlight.current) return;
      newestRefreshInFlight.current = true;
      void (async () => {
        while (newestRefreshQueued.current) {
          newestRefreshQueued.current = false;
          const targetConversation = requestedConversation.current;
          if (!targetConversation) continue;
          const generation = requestGeneration.current;
          const approvalGeneration = ++approvalRequestGeneration.current;
          const replacesConversation = loadedConversation.current !== targetConversation;
          try {
            const page = await loadTimelinePage({ conversationId: targetConversation, cursor: null, limit: PAGE_SIZE });
            if (
              generation !== requestGeneration.current
              || targetConversation !== requestedConversation.current
            ) continue;
            const box = scrollBox.current;
            scrollToEnd.current = replacesConversation
              || box === null
              || box.scrollHeight - box.scrollTop - box.clientHeight < 32;
            loadedConversation.current = targetConversation;
            const bounded = boundTimelinePage(page.items);
            if (!replacesConversation && newestItemsRef.current.length > 0) {
              const incomingIds = new Set(bounded.map(({ id }) => id));
              if (newestItemsRef.current.some(({ id }) => !incomingIds.has(id))) setHistoryEvicted(true);
            }
            newestItemsRef.current = bounded;
            setNewestItems(bounded);
            newestCursor.current = page.nextCursor;
            if (replacesConversation || olderPagesRef.current.length === 0) {
              setCursor(page.nextCursor);
            }
            const refreshedPages = [pendingApprovalPage(page.approvals)];
            const refreshedAnchors: Array<string | null> = [null];
            let refreshedCursor = page.approvalsNextCursor;
            const disclosedPageCount = approvalPages.current.length - 1;
            for (let pageIndex = 0; pageIndex < disclosedPageCount; pageIndex += 1) {
              const disclosedCursor = refreshedCursor;
              if (!disclosedCursor || refreshedPages.length >= MAX_OLDER_PAGES) break;
              const disclosed = await loadApprovalPage({
                conversationId: targetConversation,
                cursor: disclosedCursor,
                limit: APPROVAL_PAGE_SIZE,
                kind: "pending",
              });
              if (approvalGeneration !== approvalRequestGeneration.current) break;
              refreshedPages.push(pendingApprovalPage(disclosed.items));
              refreshedAnchors.push(disclosedCursor);
              refreshedCursor = disclosed.nextCursor;
              if (!disclosed.nextCursor) break;
            }
            if (approvalGeneration !== approvalRequestGeneration.current) continue;
            const reconciledApprovals = mergeApprovals([], refreshedPages.flat());
            const focusedApprovalId = document.activeElement
              ?.closest<HTMLElement>("[data-approval-id]")
              ?.dataset.approvalId;
            if (focusedApprovalId && !reconciledApprovals.some(({ id }) => id === focusedApprovalId)) {
              setResolvedApprovalFocus(focusedApprovalId);
            }
            setApprovals(reconciledApprovals);
            approvalPages.current = refreshedPages;
            approvalPageCursors.current = refreshedAnchors;
            setApprovalHistoryEvicted(false);
            approvalCursorRef.current = refreshedCursor;
            setApprovalCursor(refreshedCursor);
          } catch (reason) {
            if (
              generation === requestGeneration.current
              && targetConversation === requestedConversation.current
            ) setError(messageFor(reason));
          }
        }
      })().finally(() => {
        newestRefreshInFlight.current = false;
        if (loadedConversation.current === requestedConversation.current) setLoading(false);
        if (newestRefreshQueued.current) drainNewest();
      });
    };
    drainNewest();
  }, [actions, conversationId, refreshVersion]);

  useLayoutEffect(() => {
    if (!scrollBox.current) return;
    if (prependAnchor.current !== null) {
      restoreScrollAnchor(scrollBox.current, prependAnchor.current);
      prependAnchor.current = null;
      return;
    }
    if (scrollToEnd.current) {
      scrollBox.current.scrollTop = scrollBox.current.scrollHeight;
      scrollToEnd.current = false;
    }
  }, [items]);

  useLayoutEffect(() => {
    if (!resolvedApprovalFocus || approvals.some(({ id }) => id === resolvedApprovalFocus)) return;
    heading.current?.focus();
    setResolvedApprovalFocus(null);
  }, [approvals, resolvedApprovalFocus]);

  async function loadOlder() {
    if (!cursor || loadingOlder) return;
    const requestedCursor = cursor;
    const historyGeneration = ++historyRequestGeneration.current;
    setLoadingOlder(true);
    setError(null);
    try {
      const page = await loadTimelinePage({ conversationId, cursor: requestedCursor, limit: PAGE_SIZE });
      if (
        loadedConversation.current !== conversationId
        || historyGeneration !== historyRequestGeneration.current
      ) return;
      prependAnchor.current = captureScrollAnchor(scrollBox.current);
      setOlderPages((current) => {
        const next = [...current, boundTimelinePage(page.items)];
        const bounded = next.length > MAX_OLDER_PAGES ? next.slice(-MAX_OLDER_PAGES) : next;
        olderPagesRef.current = bounded;
        if (next.length > MAX_OLDER_PAGES) setHistoryEvicted(true);
        return bounded;
      });
      setCursor(page.nextCursor);
    } catch (reason) {
      if (historyGeneration !== historyRequestGeneration.current) return;
      prependAnchor.current = null;
      setError(messageFor(reason));
    } finally {
      setLoadingOlder(false);
    }
  }

  function reloadNewestHistory() {
    historyRequestGeneration.current += 1;
    prependAnchor.current = null;
    olderPagesRef.current = [];
    setOlderPages([]);
    setCursor(newestCursor.current);
    setHistoryEvicted(false);
    scrollToEnd.current = true;
  }

  async function loadMoreApprovals() {
    if (!approvalCursor || approvalPagerInFlight.current) return;
    const requestedCursor = approvalCursor;
    const generation = approvalRequestGeneration.current;
    const pagerToken = ++approvalPagerToken.current;
    approvalPagerInFlight.current = true;
    setLoadingApprovals(true);
    setError(null);
    try {
      const page = await loadApprovalPage({
        conversationId,
        cursor: requestedCursor,
        limit: APPROVAL_PAGE_SIZE,
        kind: "pending",
      });
      if (
        generation !== approvalRequestGeneration.current
        || requestedConversation.current !== conversationId
        || approvalCursorRef.current !== requestedCursor
      ) return;
      const nextPageCursors = [...approvalPageCursors.current, requestedCursor];
      const evictedNewerPages = nextPageCursors.length > MAX_OLDER_PAGES;
      approvalPageCursors.current = nextPageCursors.slice(-MAX_OLDER_PAGES);
      approvalPages.current = [...approvalPages.current, pendingApprovalPage(page.items)]
        .slice(-MAX_OLDER_PAGES);
      if (evictedNewerPages) setApprovalHistoryEvicted(true);
      setApprovals(mergeApprovals([], approvalPages.current.flat()).slice(0, MAX_APPROVALS));
      approvalCursorRef.current = page.nextCursor;
      setApprovalCursor(page.nextCursor);
    } catch (reason) {
      if (
        generation === approvalRequestGeneration.current
        && requestedConversation.current === conversationId
      ) setError(messageFor(reason));
    } finally {
      if (pagerToken === approvalPagerToken.current) {
        approvalPagerInFlight.current = false;
        setLoadingApprovals(false);
      }
    }
  }

  async function reloadNewestApprovals() {
    if (approvalPagerInFlight.current) return;
    const generation = ++approvalRequestGeneration.current;
    const pagerToken = ++approvalPagerToken.current;
    approvalPagerInFlight.current = true;
    setLoadingApprovals(true);
    setError(null);
    try {
      const page = await loadApprovalPage({
        conversationId, cursor: null, limit: APPROVAL_PAGE_SIZE, kind: "pending",
      });
      if (generation !== approvalRequestGeneration.current) return;
      approvalPageCursors.current = [null];
      approvalPages.current = [pendingApprovalPage(page.items)];
      setApprovals(approvalPages.current[0]!);
      approvalCursorRef.current = page.nextCursor;
      setApprovalCursor(page.nextCursor);
      setApprovalHistoryEvicted(false);
    } catch (reason) {
      if (
        generation === approvalRequestGeneration.current
        && requestedConversation.current === conversationId
      ) setError(messageFor(reason));
    } finally {
      if (pagerToken === approvalPagerToken.current) {
        approvalPagerInFlight.current = false;
        setLoadingApprovals(false);
      }
    }
  }

  async function reconcileApprovals(approvalId: string, known?: ApprovalDetailSnapshot) {
    const exact = known ?? await actions.loadApprovalDetail({ approvalId });
    approvalPages.current = approvalPages.current.map((page) => exact.status === "pending"
      ? page.map((approval) => approval.id === approvalId
        ? { ...approval, status: exact.status, responsePending: exact.responsePending }
        : approval)
      : page.filter(({ id }) => id !== approvalId));
    setApprovals((current) => exact.status === "pending"
      ? current.map((approval) => approval.id === approvalId
        ? { ...approval, status: exact.status, responsePending: exact.responsePending }
        : approval)
      : current.filter(({ id }) => id !== approvalId));
    if (exact.status !== "pending") setResolvedApprovalFocus(approvalId);
    return exact;
  }

  const agentActivity = useMemo(() => buildAgentActivity(agents), [agents]);

  return (
    <section className="timeline-region" aria-labelledby="timeline-heading">
      <div className="timeline-heading-row">
        <h2 ref={heading} id="timeline-heading" tabIndex={-1}>Timeline</h2>
        {loading ? <span role="status">Loading activity…</span> : null}
      </div>
      {error ? <p role="alert" className="inline-error">{error}</p> : null}
      <div ref={scrollBox} className="timeline-scroll" tabIndex={0} aria-label="Conversation activity">
        {historyEvicted ? (
          <div className="history-window-note" role="note">
            <span>Some history is outside this bounded view.</span>
            <button type="button" className="secondary-button" onClick={reloadNewestHistory}>Reload newest history</button>
          </div>
        ) : null}
        {cursor ? (
          <button type="button" className="secondary-button load-older" disabled={loadingOlder} onClick={() => void loadOlder()}>
            {loadingOlder ? "Loading older activity…" : "Load older activity"}
          </button>
        ) : null}
        {!loading && items.length === 0 ? <p className="empty-note">No activity yet. Start with a message below.</p> : null}
        <ol className="timeline-list">
          {items.map((item) => (
            <li key={item.id} data-timeline-id={item.id}>
              <TimelineEntry item={item} actions={actions} />
            </li>
          ))}
        </ol>
        {agentActivity.branches.length > 0 || agentsTruncated ? (
          <AgentActivity
            branches={agentActivity.branches}
            total={agentActivity.total}
            truncated={agentsTruncated}
            agentWindow={agentWindow}
            onLoadAgentPage={onLoadAgentPage}
          />
        ) : null}
        {approvals.length > 0 ? (
          <section className="approval-list" aria-labelledby="approvals-heading">
            <h3 id="approvals-heading">Needs your response</h3>
            {approvals.map((approval) => (
              <ApprovalCard
                key={approval.id}
                approval={approval}
                agentPath={canonicalAgentPath(approval.agentId, agents)}
                actions={actions}
                onReconcile={(detail) => reconcileApprovals(approval.id, detail)}
              />
            ))}
            {approvalCursor ? (
              <button type="button" className="secondary-button" disabled={loadingApprovals} onClick={() => void loadMoreApprovals()}>
                {loadingApprovals ? "Loading approvals…" : "Load more approvals"}
              </button>
            ) : null}
            {approvalHistoryEvicted ? (
              <div className="history-window-note" role="note">
                <span>Newer approvals are outside this bounded view.</span>
                <button type="button" className="secondary-button" disabled={loadingApprovals} onClick={() => void reloadNewestApprovals()}>
                  Reload newest approvals
                </button>
              </div>
            ) : null}
          </section>
        ) : null}
      </div>
    </section>
  );
}

function TimelineEntry({ item, actions }: { item: TimelineItem; actions: ConversationActions }) {
  const [expanded, setExpanded] = useState(false);
  const [detail, setDetail] = useState<{ content: string; truncated: boolean } | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const detailVersion = `${item.id}:${item.contentBytes}:${item.truncated}`;
  const detailVersionRef = useRef(detailVersion);
  const detailRequestGeneration = useRef(0);
  const provider = providerNames[item.provider];

  useLayoutEffect(() => {
    if (detailVersionRef.current === detailVersion) return;
    detailVersionRef.current = detailVersion;
    setExpanded(false);
    setDetail(null);
    setDetailError(null);
  }, [detailVersion]);

  async function toggleDetail() {
    const next = !expanded;
    setExpanded(next);
    if (!next) {
      detailRequestGeneration.current += 1;
      setDetail(null);
      setDetailError(null);
      return;
    }
    if (detail !== null || !item.truncated) return;
    setDetailError(null);
    const generation = ++detailRequestGeneration.current;
    try {
      const requestedVersion = detailVersion;
      const result = await actions.loadEventDetail({ eventId: item.id });
      if (detailVersionRef.current === requestedVersion && generation === detailRequestGeneration.current) {
        setDetail({ content: result.content, truncated: result.truncated });
      }
    } catch (reason) {
      if (detailVersionRef.current === detailVersion && generation === detailRequestGeneration.current) {
        setDetailError(messageFor(reason));
      }
    }
  }

  function eventContent(label: string, preformatted = false) {
    const content = expanded && detail ? detail.content : item.content;
    return (
      <>
        {preformatted && expanded ? <pre>{detail?.content ?? (item.truncated ? "Loading bounded detail…" : item.content)}</pre> : <p>{content}</p>}
        {item.truncated && !expanded ? <p className="truncation-note">Preview truncated.</p> : null}
        {detail?.truncated ? <p className="truncation-note">Bounded detail remains truncated.</p> : null}
        {item.truncated || preformatted ? (
          <button type="button" className="disclosure-link" aria-expanded={expanded} onClick={() => void toggleDetail()}>
            {expanded ? `Hide ${label}` : `Show ${label}`}
          </button>
        ) : null}
        {detailError ? <p role="alert">{detailError}</p> : null}
      </>
    );
  }

  if (item.kind === "message") {
    const role = item.role ?? "assistant";
    return (
      <article className={`timeline-message ${role}`} aria-label={`${provider} ${role} message`}>
        <header><span>{provider}</span><span>{role === "user" ? "You" : "Assistant"}</span></header>
        {eventContent("full message")}
      </article>
    );
  }

  if (item.kind === "tool") {
    return (
      <article className="timeline-activity tool-activity" aria-label={`${provider} tool activity`}>
        <header><span>{provider}</span><span>Tool</span></header>
        {eventContent("tool output", true)}
      </article>
    );
  }

  const failure = /^(?:run |provider (?:turn )?)?fail(?:ed|ure)\b/i.test(item.content);
  const label = failure
    ? "Failure"
    : item.kind === "progress"
      ? "Progress"
      : item.kind === "lifecycle" ? "Run lifecycle" : "Provider activity";
  return (
    <article
      className={`timeline-activity ${item.kind}${failure ? " failure" : ""}`}
      aria-label={`${provider} ${label.toLowerCase()}`}
    >
      <header><span>{provider}</span><span>{label}</span></header>
      {eventContent(`full ${label.toLowerCase()}`)}
    </article>
  );
}

type AgentBranch = { agent: AgentSnapshot; children: AgentBranch[] };

function buildAgentActivity(agents: readonly AgentSnapshot[]) {
  const branches = new Map(agents.map((agent) => [agent.id, { agent, children: [] as AgentBranch[] }]));
  const roots: AgentBranch[] = [];
  let total = 0;
  branches.forEach((branch) => {
    if (branch.agent.parentId !== null) total += 1;
    if (branch.agent.parentId && branches.has(branch.agent.parentId)) {
      branches.get(branch.agent.parentId)!.children.push(branch);
    } else if (branch.agent.parentId !== null) {
      roots.push(branch);
    }
  });
  branches.forEach((branch) => {
    if (branch.agent.parentId !== null) return;
    for (const child of branch.children) roots.push(child);
  });
  return { branches: roots, total };
}

function AgentActivity({ branches, total, truncated, agentWindow, onLoadAgentPage }: {
  branches: AgentBranch[];
  total: number;
  truncated: boolean;
  agentWindow: AgentWindowSnapshot | null;
  onLoadAgentPage(restart: boolean): void;
}) {
  const [open, setOpen] = useState(false);
  const [visibleOffset, setVisibleOffset] = useState(0);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const visiblePage = useMemo(
    () => getVisibleAgentPage(branches, expanded, visibleOffset, AGENT_PAGE_SIZE),
    [branches, expanded, visibleOffset],
  );

  useEffect(() => {
    if (open && visibleOffset > 0 && visiblePage.items.length === 0) setVisibleOffset(0);
  }, [open, visibleOffset, visiblePage.items.length]);

  function toggleAgent(id: string) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  return (
    <section className="agent-activity" aria-labelledby="agents-heading">
      <div className="agent-activity-heading">
        <h3 id="agents-heading">Agent activity</h3>
        <button type="button" className="secondary-button" aria-expanded={open} onClick={() => {
          const next = !open;
          setOpen(next);
          if (next && truncated && !agentWindow) onLoadAgentPage(true);
        }}>
          {open ? "Hide" : "Show"} agent activity ({total})
        </button>
      </div>
      {open ? (
        <>
          {visiblePage.items.map(({ branch, depth }) => (
            <AgentCard
              key={branch.agent.id}
              branch={branch}
              depth={depth}
              expanded={expanded.has(branch.agent.id)}
              onToggle={toggleAgent}
            />
          ))}
          <div className="agent-pagination">
            {visibleOffset > 0 ? (
              <button type="button" className="secondary-button" onClick={() => setVisibleOffset((value) => Math.max(0, value - AGENT_PAGE_SIZE))}>
                Show previous agents
              </button>
            ) : null}
            {visiblePage.hasMore ? (
              <button type="button" className="secondary-button" onClick={() => setVisibleOffset((value) => value + AGENT_PAGE_SIZE)}>
                Show more agents
              </button>
            ) : null}
          </div>
          {agentWindow?.error ? (
            <div className="inline-error">
              <p role="alert">{agentWindow.error}</p>
              <button type="button" className="secondary-button" onClick={() => onLoadAgentPage(true)}>
                Retry agent activity
              </button>
            </div>
          ) : null}
          {agentWindow?.loading ? <p role="status">Loading more agent activity…</p> : null}
          {agentWindow?.nextCursor ? <button type="button" className="secondary-button" disabled={agentWindow.loading} onClick={() => onLoadAgentPage(false)}>Load more agent activity</button> : null}
          {agentWindow?.evicted ? <button type="button" className="secondary-button" disabled={agentWindow.loading} onClick={() => onLoadAgentPage(true)}>Reload newest agent activity</button> : null}
          {visibleOffset > 0 || visiblePage.hasMore ? (
            <p className="truncation-note">Agent cards are paged to keep this view responsive.</p>
          ) : null}
        </>
      ) : null}
    </section>
  );
}

function AgentCard({
  branch,
  depth,
  expanded,
  onToggle,
}: {
  branch: AgentBranch;
  depth: number;
  expanded: boolean;
  onToggle(id: string): void;
}) {
  const provider = providerNames[branch.agent.provider];
  const status = statusNames[branch.agent.status];
  return (
    <article
      className="agent-card"
      aria-label={`${branch.agent.label}, ${provider}, ${status}`}
      data-depth={depth + 1}
      style={{ "--agent-depth": Math.min(depth, 8) } as React.CSSProperties}
    >
      <header>
        <strong>{branch.agent.label}</strong>
        <span>{provider} · {status}</span>
        {branch.children.length > 0 ? (
          <button type="button" className="disclosure-button" aria-expanded={expanded} aria-label={`${expanded ? "Collapse" : "Expand"} ${branch.agent.label}`} onClick={() => onToggle(branch.agent.id)}>
            {expanded ? "−" : "+"}
          </button>
        ) : null}
      </header>
      {branch.agent.summary ? <p>{branch.agent.summary}</p> : null}
    </article>
  );
}

function getVisibleAgentPage(
  branches: readonly AgentBranch[],
  expanded: ReadonlySet<string>,
  offset: number,
  limit: number,
) {
  const items: Array<{ branch: AgentBranch; depth: number }> = [];
  const stack: Array<{ branches: readonly AgentBranch[]; depth: number; index: number }> = [
    { branches, depth: 0, index: 0 },
  ];
  let visibleIndex = 0;
  while (stack.length > 0) {
    const frame = stack.at(-1)!;
    if (frame.index >= frame.branches.length) {
      stack.pop();
      continue;
    }
    const branch = frame.branches[frame.index++]!;
    if (visibleIndex >= offset) {
      if (items.length === limit) return { items, hasMore: true };
      items.push({ branch, depth: frame.depth });
    }
    visibleIndex += 1;
    if (expanded.has(branch.agent.id) && branch.children.length > 0) {
      stack.push({ branches: branch.children, depth: frame.depth + 1, index: 0 });
    }
  }
  return { items, hasMore: false };
}

function mergeTimeline(current: readonly TimelineItem[], incoming: readonly TimelineItem[]): TimelineItem[] {
  const byId = new Map(current.map((item) => [item.id, item]));
  incoming.forEach((item) => byId.set(item.id, item));
  return [...byId.values()].sort((left, right) => compareSequence(left.sequence, right.sequence));
}

function boundTimelinePage(items: readonly TimelineItem[]) {
  return mergeTimeline([], items).slice(-PAGE_SIZE);
}

type ScrollAnchor = {
  id: string | null;
  viewportOffset: number;
  scrollHeight: number;
};

function captureScrollAnchor(box: HTMLDivElement | null): ScrollAnchor | null {
  if (!box) return null;
  const first = [...box.querySelectorAll<HTMLElement>("[data-timeline-id]")][0] ?? null;
  return {
    id: first?.dataset.timelineId ?? null,
    viewportOffset: (first?.offsetTop ?? 0) - box.scrollTop,
    scrollHeight: box.scrollHeight,
  };
}

function restoreScrollAnchor(box: HTMLDivElement, anchor: ScrollAnchor) {
  const matched = anchor.id
    ? [...box.querySelectorAll<HTMLElement>("[data-timeline-id]")]
      .find(({ dataset }) => dataset.timelineId === anchor.id)
    : null;
  if (matched) {
    box.scrollTop = matched.offsetTop - anchor.viewportOffset;
  } else {
    box.scrollTop += box.scrollHeight - anchor.scrollHeight;
  }
}

function mergeApprovals(current: readonly ApprovalSnapshot[], incoming: readonly ApprovalSnapshot[]) {
  const byId = new Map(current.map((approval) => [approval.id, approval]));
  incoming.forEach((approval) => byId.set(approval.id, approval));
  return [...byId.values()].filter(({ status }) => status === "pending");
}

function pendingApprovalPage(items: readonly ApprovalSnapshot[]) {
  return items.filter(({ status }) => status === "pending").slice(0, APPROVAL_PAGE_SIZE);
}

function compareSequence(left: string, right: string) {
  const a = BigInt(left);
  const b = BigInt(right);
  return a < b ? -1 : a > b ? 1 : 0;
}

function canonicalAgentPath(agentId: string, agents: readonly AgentSnapshot[]) {
  const byId = new Map(agents.map((agent) => [agent.id, agent]));
  const labels: string[] = [];
  const seen = new Set<string>();
  let current = byId.get(agentId);
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    labels.unshift(current.label);
    current = current.parentId ? byId.get(current.parentId) : undefined;
  }
  return labels.join("/") || "Agent";
}

function messageFor(reason: unknown) {
  return reason instanceof Error ? reason.message : "Prompting Time could not load this activity.";
}
