import { useEffect, useRef, useState } from "react";
import type {
  CleanupBlocker,
  ConversationSummary,
  InspectorSnapshot,
  ProviderInstallation,
  RunAuditDetailSnapshot,
  RunAuditSummarySnapshot,
  RoutingBlocker,
  RoutingCriterion,
} from "../../bridge/types";
import type { AppActions } from "../../app/store";

type InspectorProps = {
  conversation: ConversationSummary;
  providers: readonly ProviderInstallation[];
  refreshVersion: number;
  actions: AppActions;
};

const providerNames = { codex: "Codex", claude: "Claude" } as const;
const RUN_AUDIT_PAGE_SIZE = 10;
const MAX_RUN_AUDIT_ITEMS = 40;

export function Inspector({ conversation, providers, refreshVersion, actions }: InspectorProps) {
  const [snapshot, setSnapshot] = useState<InspectorSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const [refreshAvailable, setRefreshAvailable] = useState(false);
  const [loading, setLoading] = useState(false);
  const inspectGeneration = useRef(0);
  const inspectInFlight = useRef(false);
  const renderedRefreshVersion = useRef(refreshVersion);
  const latestRefreshVersion = useRef(refreshVersion);
  latestRefreshVersion.current = refreshVersion;
  const [runAudits, setRunAudits] = useState<RunAuditSummarySnapshot[]>([]);
  const [runCursor, setRunCursor] = useState<string | null>(null);
  const [runHistoryEvicted, setRunHistoryEvicted] = useState(false);
  const [selectedRun, setSelectedRun] = useState<RunAuditDetailSnapshot | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const [loadingRuns, setLoadingRuns] = useState(false);
  const runListGeneration = useRef(0);
  const runDetailGeneration = useRef(0);
  const runConversation = useRef<string | null>(null);
  const runPagingStarted = useRef(false);
  const runListInFlight = useRef(false);
  const runListRefreshQueued = useRef(false);
  const runListResetQueued = useRef(false);
  const actionsRef = useRef(actions);
  actionsRef.current = actions;

  function refreshRunAudits(reset: boolean) {
    runListRefreshQueued.current = true;
    runListResetQueued.current ||= reset;
    drainRunAuditRefreshes();
  }

  function drainRunAuditRefreshes() {
    if (runListInFlight.current) return;
    runListInFlight.current = true;
    setLoadingRuns(true);
    void (async () => {
      while (runListRefreshQueued.current) {
        const reset = runListResetQueued.current;
        runListRefreshQueued.current = false;
        runListResetQueued.current = false;
        const generation = runListGeneration.current;
        const conversationId = runConversation.current;
        if (!conversationId) continue;
        setRunError(null);
        try {
          const page = await actionsRef.current.listRunAudits({ conversationId, cursor: null, limit: RUN_AUDIT_PAGE_SIZE });
          if (
            generation !== runListGeneration.current
            || conversationId !== runConversation.current
          ) continue;
          setRunAudits((current) => {
            const currentIds = new Set(current.map(({ id }) => id));
            const introduced = page.items.some(({ id }) => !currentIds.has(id));
            const merged = reset ? page.items : mergeRunAudits(page.items, current);
            if (!reset && introduced && current.length >= MAX_RUN_AUDIT_ITEMS) {
              setRunHistoryEvicted(true);
              setRunCursor(null);
            }
            return merged;
          });
          if (reset || !runPagingStarted.current) setRunCursor(page.nextCursor);
        } catch (reason) {
          if (
            generation === runListGeneration.current
            && conversationId === runConversation.current
          ) setRunError(messageFor(reason));
        }
      }
    })().finally(() => {
      runListInFlight.current = false;
      setLoadingRuns(false);
      if (runListRefreshQueued.current) drainRunAuditRefreshes();
    });
  }

  function refreshInspector(includeRunHistory = false) {
    if (inspectInFlight.current) return;
    inspectInFlight.current = true;
    const generation = inspectGeneration.current;
    const conversationId = conversation.id;
    const requestedRefreshVersion = refreshVersion;
    setLoading(true);
    setError(null);
    if (includeRunHistory) refreshRunAudits(false);
    void actions.inspectWorkspace({ conversationId })
      .then((next) => {
        if (generation === inspectGeneration.current) {
          setSnapshot(next);
          renderedRefreshVersion.current = requestedRefreshVersion;
          setRefreshAvailable(latestRefreshVersion.current !== requestedRefreshVersion);
        }
      })
      .catch((reason: unknown) => {
        if (generation === inspectGeneration.current) setError(messageFor(reason));
      })
      .finally(() => {
        if (generation === inspectGeneration.current) {
          inspectInFlight.current = false;
          setLoading(false);
        }
      });
  }

  useEffect(() => {
    inspectGeneration.current += 1;
    inspectInFlight.current = false;
    setSnapshot(null);
    setRefreshAvailable(false);
    renderedRefreshVersion.current = refreshVersion;
    refreshInspector();
    return () => { inspectGeneration.current += 1; };
  }, [actions, conversation.id]);

  useEffect(() => {
    if (refreshVersion !== renderedRefreshVersion.current) setRefreshAvailable(true);
  }, [refreshVersion]);

  useEffect(() => {
    const reset = runConversation.current !== conversation.id;
    runConversation.current = conversation.id;
    if (reset) {
      runListGeneration.current += 1;
      runDetailGeneration.current += 1;
      runPagingStarted.current = false;
      setRunAudits([]);
      setRunCursor(null);
      setRunHistoryEvicted(false);
      setSelectedRun(null);
      setRunError(null);
      setLoadingRuns(false);
    }
    refreshRunAudits(reset);
    return () => { runDetailGeneration.current += 1; };
  }, [actions, conversation.id, conversation.currentRunId]);

  async function loadMoreRuns() {
    if (!runCursor || runListInFlight.current) return;
    const generation = runListGeneration.current;
    runListInFlight.current = true;
    setLoadingRuns(true);
    setRunError(null);
    try {
      const page = await actions.listRunAudits({
        conversationId: conversation.id, cursor: runCursor, limit: RUN_AUDIT_PAGE_SIZE,
      });
      if (generation !== runListGeneration.current) return;
      runPagingStarted.current = true;
      setRunAudits((current) => {
        const combined = [...current, ...page.items];
        if (combined.length > MAX_RUN_AUDIT_ITEMS) setRunHistoryEvicted(true);
        return combined.slice(-MAX_RUN_AUDIT_ITEMS);
      });
      setRunCursor(page.nextCursor);
    } catch (reason) {
      if (generation === runListGeneration.current) setRunError(messageFor(reason));
    } finally {
      runListInFlight.current = false;
      if (generation === runListGeneration.current) setLoadingRuns(false);
      if (runListRefreshQueued.current) drainRunAuditRefreshes();
    }
  }

  async function inspectRun(runId: string) {
    const generation = ++runDetailGeneration.current;
    setRunError(null);
    try {
      const detail = await actions.loadRunAudit({ conversationId: conversation.id, runId });
      if (generation === runDetailGeneration.current) setSelectedRun(detail);
    } catch (reason) {
      if (generation === runDetailGeneration.current) setRunError(messageFor(reason));
    }
  }

  function toggle(section: string) {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(section)) next.delete(section);
      else next.add(section);
      return next;
    });
  }

  if (error && !snapshot) return <p role="alert" className="inline-error">{error}</p>;
  if (!snapshot) return <p role="status">Loading inspector…</p>;

  return (
    <div className="inspector-content">
      <div className="inspector-refresh">
        <button type="button" className="secondary-button" disabled={loading} onClick={() => refreshInspector(true)}>Refresh inspector</button>
        {refreshAvailable ? <small>New conversation activity is available. Refresh to update workspace details.</small> : null}
        {error ? <p role="alert" className="inline-error">{error}</p> : null}
      </div>
      <InspectorSection id="run-history" title="Provider run history" collapsed={collapsed.has("run-history")} onToggle={toggle}>
        {runAudits.length ? (
          <ol className="run-audit-list">
            {runAudits.map((run, index) => (
              <li key={run.id}>
                <button type="button" className="secondary-button" onClick={() => void inspectRun(run.id)}>
                  Inspect {providerNames[run.provider]} run {index + 1}
                </button>
                <small>{humanize(run.status)} · {run.reason ? humanize(run.reason) : "Routing unavailable"}{run.hasHandoff ? " · handoff" : ""}</small>
              </li>
            ))}
          </ol>
        ) : <p>{loadingRuns ? "Loading provider runs…" : "No provider runs yet."}</p>}
        {runCursor ? <button type="button" className="secondary-button" disabled={loadingRuns} onClick={() => void loadMoreRuns()}>Load older provider runs</button> : null}
        {runHistoryEvicted ? <div className="history-window-note" role="note"><span>Some provider runs are outside this bounded view.</span><button type="button" className="secondary-button" disabled={loadingRuns} onClick={() => { setRunHistoryEvicted(false); runPagingStarted.current = false; refreshRunAudits(true); }}>Reload newest provider runs</button></div> : null}
        {runError ? <p role="alert" className="inline-error">{runError}</p> : null}
        {selectedRun ? (
          <div className="run-audit-detail">
            <h3>{providerNames[selectedRun.provider]} run detail</h3>
            <p>{selectedRun.routing?.explanation ?? "No routing decision was recorded for this run."}</p>
            <p><strong>Reason:</strong> {selectedRun.reason ? humanize(selectedRun.reason) : "Unavailable"}</p>
            {selectedRun.routingTruncated ? <p>Detailed routing evaluation exceeded the audit display limit.</p> : null}
            {selectedRun.handoff ? <pre className="handoff-content">{selectedRun.handoff}</pre> : <p>No handoff was sent for this run.</p>}
            {selectedRun.handoffTruncated ? <p className="truncation-note">The stored handoff exceeds the bounded audit view.</p> : null}
          </div>
        ) : null}
      </InspectorSection>
      <InspectorSection id="routing" title="Routing" collapsed={collapsed.has("routing")} onToggle={toggle}>
        {snapshot.routing ? (
          <>
            <p><strong>{providerNames[snapshot.routing.provider]}</strong> · {humanize(snapshot.routing.profile)} · {humanize(snapshot.routing.taskKind)}</p>
            <p>{snapshot.routing.explanation}</p>
            <dl className="inspector-list">
              <div><dt>Reason</dt><dd>{humanize(snapshot.routing.reason)}</dd></div>
              <div><dt>Override</dt><dd>{snapshot.routing.overrideProvider ? providerNames[snapshot.routing.overrideProvider] : "Automatic"}</dd></div>
              <div><dt>Required</dt><dd>{snapshot.routing.requiredCapabilities.map(humanize).join(", ") || "No special capabilities"}</dd></div>
            </dl>
            <ul className="evaluation-list">
              {snapshot.routing.evaluations.map((evaluation) => (
                <li key={evaluation.provider}>
                  {providerNames[evaluation.provider]}: {evaluation.eligible
                    ? "eligible"
                    : `unavailable — ${evaluation.blockers.map(blockerLabel).join(", ")}`}
                </li>
              ))}
            </ul>
            <ol className="rationale-list">
              {snapshot.routing.rationale.map((criterion, index) => (
                <li key={`${criterion.kind}-${index}`}>{criterionLabel(criterion)}</li>
              ))}
            </ol>
          </>
        ) : <p>No route has been selected yet.</p>}
      </InspectorSection>

      <InspectorSection id="workspace" title="Workspace" collapsed={collapsed.has("workspace")} onToggle={toggle}>
        <dl className="inspector-list">
          <div><dt>Mode</dt><dd>{humanize(snapshot.workspace.mode)}</dd></div>
          <div><dt>Project</dt><dd>{conversation.projectRoot ?? "No project"}</dd></div>
          <div><dt>Execution path</dt><dd>{snapshot.executionPath}</dd></div>
          <div><dt>Worktree</dt><dd>{snapshot.ownedWorktree ? "Prompting Time owned" : "Not app-owned"}</dd></div>
          <div><dt>Cleanup</dt><dd>{snapshot.cleanup.eligible ? "Eligible" : `Cleanup blocked: ${cleanupLabel(snapshot.cleanup.blocker)}`}</dd></div>
        </dl>
        {snapshot.workspace.changes.length ? (
          <ul className="change-list">
            {snapshot.workspace.changes.map((change, index) => (
              <li key={`${change.relativePath}-${index}`}><span>{humanize(change.kind)}</span> {change.relativePath}</li>
            ))}
          </ul>
        ) : <p>No changed files.</p>}
        {snapshot.workspace.truncated ? <p className="truncation-note">Changed-file summary is truncated.</p> : null}
      </InspectorSection>

      <InspectorSection id="agents" title="Active agents" collapsed={collapsed.has("agents")} onToggle={toggle}>
        <p>{snapshot.activeDescendantCount} active descendants</p>
        {snapshot.agentsTruncated ? <p className="truncation-note">The sidebar agent preview is truncated; expand the conversation to page through all agents.</p> : null}
      </InspectorSection>

      <InspectorSection id="handoff" title="Context handoff" collapsed={collapsed.has("handoff")} onToggle={toggle}>
        {snapshot.handoff ? <pre className="handoff-content">{snapshot.handoff}</pre> : <p>No cross-provider handoff for this run.</p>}
      </InspectorSection>

      <InspectorSection id="providers" title="Provider versions" collapsed={collapsed.has("providers")} onToggle={toggle}>
        <ul className="provider-list">
          {providers.map((provider) => (
            <li key={provider.id}>
              {provider.available
                ? `${providerNames[provider.id]} ${provider.version ?? "version unknown"}`
                : `${providerNames[provider.id]} unavailable: ${provider.diagnostic ?? "Not installed"}`}
              {!provider.available && provider.version ? <small> · Version {provider.version}</small> : null}
            </li>
          ))}
        </ul>
      </InspectorSection>
    </div>
  );
}

function InspectorSection({
  id,
  title,
  collapsed,
  onToggle,
  children,
}: {
  id: string;
  title: string;
  collapsed: boolean;
  onToggle(id: string): void;
  children: React.ReactNode;
}) {
  const headingId = `inspector-${id}-heading`;
  const contentId = `inspector-${id}-content`;
  return (
    <section className="inspector-section" aria-labelledby={headingId}>
      <div className="inspector-section-heading">
        <h2 id={headingId}>{title}</h2>
        <button type="button" className="disclosure-button" aria-expanded={!collapsed} aria-controls={contentId} aria-label={`${collapsed ? "Expand" : "Collapse"} ${id}`} onClick={() => onToggle(id)}>
          {collapsed ? "+" : "−"}
        </button>
      </div>
      {!collapsed ? <div id={contentId}>{children}</div> : null}
    </section>
  );
}

function blockerLabel(blocker: RoutingBlocker) {
  switch (blocker.kind) {
    case "unavailable": return humanize(blocker.value);
    case "missingCapability": return `missing ${humanize(blocker.value)}`;
    case "notReported": return "status not reported";
  }
}

function criterionLabel(criterion: RoutingCriterion) {
  switch (criterion.kind) {
    case "manualOverride": return `Manual override: ${providerNames[criterion.provider]}`;
    case "eligibleProviders": return `Eligible: ${criterion.providers.map((provider) => providerNames[provider]).join(", ") || "none"}`;
    case "requiredCapabilities": return `Required: ${criterion.capabilities.map(humanize).join(", ") || "none"}`;
    case "continuity": return `Continuity: ${providerNames[criterion.provider]}`;
    case "rankedCandidates": return `Ranked: ${criterion.candidates.map(({ provider, recentRootRuns, stableOrder }) => (
      `${providerNames[provider]} (${recentRootRuns} recent root runs, stable order ${stableOrder})`
    )).join(", ")}`;
    case "safeFallback": return `Safe fallback: ${providerNames[criterion.from]} to ${providerNames[criterion.to]}`;
  }
}

function cleanupLabel(blocker: CleanupBlocker | null) {
  return blocker ? humanize(blocker) : "Unknown reason";
}

function humanize(value: string) {
  const words = value.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase();
  return words.replace(/^./, (letter) => letter.toUpperCase());
}

function mergeRunAudits(
  newest: readonly RunAuditSummarySnapshot[],
  disclosed: readonly RunAuditSummarySnapshot[],
) {
  const byId = new Map(disclosed.map((run) => [run.id, run]));
  newest.forEach((run) => byId.set(run.id, run));
  return [
    ...newest,
    ...disclosed.filter((run) => !newest.some(({ id }) => id === run.id)),
  ].map((run) => byId.get(run.id)!).slice(0, MAX_RUN_AUDIT_ITEMS);
}

function messageFor(reason: unknown) {
  return reason instanceof Error ? reason.message : "Prompting Time could not inspect this conversation.";
}
