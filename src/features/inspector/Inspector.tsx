import { createEffect, createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import type { JSX } from "@solidjs/web";
import type {
  CleanupBlocker,
  ConversationSummary,
  InspectorSnapshot,
  ProviderInstallation,
  RoutingBlocker,
  RoutingCriterion,
  RunAuditDetailSnapshot,
  RunAuditSummarySnapshot,
} from "../../bridge/types";
import type { AppActions } from "../../app/store";
import { Diagnostics } from "./Diagnostics";

type InspectorProps = {
  /** One Inspector shows one conversation; the caller makes a new one for another conversation. */
  conversation: ConversationSummary;
  providers: readonly ProviderInstallation[];
  refreshVersion: number;
  actions: AppActions;
};

const providerNames = { codex: "Codex", claude: "Claude" } as const;
const RUN_AUDIT_PAGE_SIZE = 10;
const MAX_RUN_AUDIT_ITEMS = 40;

/**
 * Routing, workspace, provider runs, and diagnostics for the selected conversation. The workspace
 * snapshot runs Git, so streamed activity only marks it stale; you refresh it explicitly.
 */
export function Inspector(props: InspectorProps) {
  const conversationId = untrack(() => props.conversation.id);
  const [snapshot, setSnapshot] = createSignal<InspectorSnapshot | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [collapsed, setCollapsed] = createSignal<ReadonlySet<string>>(new Set());
  const [renderedVersion, setRenderedVersion] = createSignal(untrack(() => props.refreshVersion));
  const [loading, setLoading] = createSignal(false);
  const [runAudits, setRunAudits] = createSignal<RunAuditSummarySnapshot[]>([]);
  const [runCursor, setRunCursor] = createSignal<string | null>(null);
  const [runHistoryEvicted, setRunHistoryEvicted] = createSignal(false);
  const [selectedRun, setSelectedRun] = createSignal<RunAuditDetailSnapshot | null>(null);
  const [runError, setRunError] = createSignal<string | null>(null);
  const [loadingRuns, setLoadingRuns] = createSignal(false);
  let disposed = false;
  let inspecting = false;
  let runDetailGeneration = 0;
  let runPagingStarted = false;
  let runListInFlight = false;
  let runListRefreshQueued = false;
  let runListResetQueued = false;
  onSettled(() => () => {
    disposed = true;
  });
  const refreshAvailable = () => snapshot() !== null && props.refreshVersion !== renderedVersion();

  function refreshRunAudits(reset: boolean) {
    // Also called from the effect for a new run, which must rerun only for the run.
    if (!untrack(() => props.conversation.capabilities.canSend)) return;
    runListRefreshQueued = true;
    runListResetQueued ||= reset;
    if (runListInFlight) return;
    runListInFlight = true;
    setLoadingRuns(true);
    void (async () => {
      while (runListRefreshQueued && !disposed) {
        const resetting = runListResetQueued;
        runListRefreshQueued = false;
        runListResetQueued = false;
        setRunError(null);
        try {
          const page = await props.actions.listRunAudits({ conversationId, cursor: null, limit: RUN_AUDIT_PAGE_SIZE });
          if (disposed) return;
          const current = runAudits();
          const introduced = page.items.some(({ id }) => !current.some((run) => run.id === id));
          if (!resetting && introduced && current.length >= MAX_RUN_AUDIT_ITEMS) {
            setRunHistoryEvicted(true);
            setRunCursor(null);
          }
          setRunAudits(resetting ? page.items : mergeRunAudits(page.items, current));
          if (resetting || !runPagingStarted) setRunCursor(page.nextCursor);
        } catch (reason) {
          if (!disposed) setRunError(messageFor(reason));
        }
      }
      runListInFlight = false;
      if (!disposed) setLoadingRuns(false);
    })();
  }

  function refreshInspector(includeRunHistory = false) {
    if (inspecting) return;
    inspecting = true;
    const requestedVersion = props.refreshVersion;
    setLoading(true);
    setError(null);
    if (includeRunHistory) refreshRunAudits(false);
    void props.actions.inspectWorkspace({ conversationId })
      .then((next) => {
        if (disposed) return;
        setSnapshot(next);
        setRenderedVersion(requestedVersion);
      })
      .catch((reason: unknown) => {
        if (!disposed) setError(messageFor(reason));
      })
      .finally(() => {
        inspecting = false;
        if (!disposed) setLoading(false);
      });
  }

  onSettled(() => refreshInspector());
  // A new run of this conversation adds to its run history.
  const currentRunId = createMemo(() => props.conversation.currentRunId);
  createEffect(currentRunId, () => refreshRunAudits(false));

  async function loadMoreRuns() {
    const requested = runCursor();
    if (!requested || runListInFlight) return;
    runListInFlight = true;
    setLoadingRuns(true);
    setRunError(null);
    try {
      const page = await props.actions.listRunAudits({ conversationId, cursor: requested, limit: RUN_AUDIT_PAGE_SIZE });
      if (disposed) return;
      runPagingStarted = true;
      const combined = [...runAudits(), ...page.items];
      if (combined.length > MAX_RUN_AUDIT_ITEMS) setRunHistoryEvicted(true);
      setRunAudits(combined.slice(-MAX_RUN_AUDIT_ITEMS));
      setRunCursor(page.nextCursor);
    } catch (reason) {
      if (!disposed) setRunError(messageFor(reason));
    } finally {
      runListInFlight = false;
      if (!disposed) setLoadingRuns(false);
      if (runListRefreshQueued) refreshRunAudits(false);
    }
  }

  async function inspectRun(runId: string) {
    const generation = ++runDetailGeneration;
    setRunError(null);
    try {
      const detail = await props.actions.loadRunAudit({ conversationId, runId });
      if (!disposed && generation === runDetailGeneration) setSelectedRun(detail);
    } catch (reason) {
      if (!disposed && generation === runDetailGeneration) setRunError(messageFor(reason));
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
  const section = (id: string, title: string, children: () => JSX.Element) => (
    <InspectorSection id={id} title={title} collapsed={collapsed().has(id)} onToggle={toggle}>{children()}</InspectorSection>
  );

  return (
    <div class="inspector-content">
      <Diagnostics conversationId={conversationId} actions={props.actions} />
      <Show when={snapshot()} fallback={<Show when={error()} fallback={<p role="status">Loading inspector…</p>}><p role="alert" class="inline-error">{error()}</p></Show>}>
        {(current) => (
          <>
            <div class="inspector-refresh">
              <button type="button" class="pt-button" disabled={loading()} onClick={() => refreshInspector(true)}>Refresh inspector</button>
              <Show when={refreshAvailable()}><small>New conversation activity is available. Refresh to update workspace details.</small></Show>
              <Show when={error()}><p role="alert" class="inline-error">{error()}</p></Show>
            </div>
            <Show when={props.conversation.capabilities.canSend}>
              {section("run-history", "Provider run history", () => (
                <>
                  <Show when={runAudits().length > 0} fallback={<p>{loadingRuns() ? "Loading provider runs…" : "No provider runs yet."}</p>}>
                    <ol class="run-audit-list">
                      <For each={runAudits()}>
                        {(run, index) => (
                          <li>
                            <button type="button" class="pt-button" onClick={() => void inspectRun(run.id)}>Inspect {providerNames[run.provider]} run {index() + 1}</button>
                            <small>{humanize(run.status)} · {run.reason ? humanize(run.reason) : "Routing unavailable"}{run.hasHandoff ? " · handoff" : ""}</small>
                          </li>
                        )}
                      </For>
                    </ol>
                  </Show>
                  <Show when={runCursor()}><button type="button" class="pt-button" disabled={loadingRuns()} onClick={() => void loadMoreRuns()}>Load older provider runs</button></Show>
                  <Show when={runHistoryEvicted()}>
                    <div class="history-window-note" role="note">
                      <span>Some provider runs are outside this bounded view.</span>
                      <button type="button" class="pt-button" disabled={loadingRuns()} onClick={() => { setRunHistoryEvicted(false); runPagingStarted = false; refreshRunAudits(true); }}>Reload newest provider runs</button>
                    </div>
                  </Show>
                  <Show when={runError()}><p role="alert" class="inline-error">{runError()}</p></Show>
                  <Show when={selectedRun()}>{(run) => <RunDetail run={run()} />}</Show>
                </>
              ))}
            </Show>
            <Show when={props.conversation.capabilities.canRoute}>
              {section("routing", "Routing", () => (
                <Show when={current().routing} fallback={<p>No route has been selected yet.</p>}>
                  {(routing) => (
                    <>
                      <p><strong>{providerNames[routing().provider]}</strong> · {humanize(routing().profile)} · {humanize(routing().taskKind)}</p>
                      <p>{routing().explanation}</p>
                      <dl class="inspector-list">
                        <div><dt>Reason</dt><dd>{humanize(routing().reason)}</dd></div>
                        <div><dt>Override</dt><dd>{routing().overrideProvider ? providerNames[routing().overrideProvider!] : "Automatic"}</dd></div>
                        <div><dt>Required</dt><dd>{routing().requiredCapabilities.map(humanize).join(", ") || "No special capabilities"}</dd></div>
                      </dl>
                      <ul class="evaluation-list">
                        <For each={routing().evaluations}>
                          {(evaluation) => <li>{providerNames[evaluation.provider]}: {evaluation.eligible ? "eligible" : `unavailable — ${evaluation.blockers.map(blockerLabel).join(", ")}`}</li>}
                        </For>
                      </ul>
                      <ol class="rationale-list"><For each={routing().rationale}>{(criterion) => <li>{criterionLabel(criterion)}</li>}</For></ol>
                    </>
                  )}
                </Show>
              ))}
            </Show>
            {section("workspace", "Workspace", () => (
              <>
                <dl class="inspector-list">
                  <div><dt>Mode</dt><dd>{humanize(current().workspace.mode)}</dd></div>
                  <div><dt>Project</dt><dd>{props.conversation.projectRoot ?? "No project"}</dd></div>
                  <div><dt>Execution path</dt><dd>{current().executionPath}</dd></div>
                  <div><dt>Worktree</dt><dd>{current().ownedWorktree ? "Prompting Time owned" : "Not app-owned"}</dd></div>
                  <div><dt>Cleanup</dt><dd>{current().cleanup.eligible ? "Eligible" : `Cleanup blocked: ${cleanupLabel(current().cleanup.blocker)}`}</dd></div>
                </dl>
                <Show when={current().workspace.changes.length > 0} fallback={<p>No changed files.</p>}>
                  <ul class="change-list"><For each={current().workspace.changes}>{(change) => <li><span>{humanize(change.kind)}</span> {change.relativePath}</li>}</For></ul>
                </Show>
                <Show when={current().workspace.truncated}><p class="truncation-note">Changed-file summary is truncated.</p></Show>
              </>
            ))}
            <Show when={props.conversation.capabilities.canSend}>
              {section("children", "Active child conversations", () => <p>{current().activeDescendantCount} active descendants</p>)}
            </Show>
            <Show when={props.conversation.capabilities.canRoute}>
              {section("handoff", "Context handoff", () => (
                <Show when={current().handoff} fallback={<p>No cross-provider handoff for this run.</p>}><pre class="handoff-content">{current().handoff}</pre></Show>
              ))}
            </Show>
            {section("providers", "Provider versions", () => (
              <ul class="provider-list">
                <For each={props.providers}>
                  {(provider) => (
                    <li>
                      {provider.available ? `${providerNames[provider.id]} ${provider.version ?? "version unknown"}` : `${providerNames[provider.id]} unavailable: ${provider.diagnostic ?? "Not installed"}`}
                      <Show when={!provider.available && provider.version}><small> · Version {provider.version}</small></Show>
                    </li>
                  )}
                </For>
              </ul>
            ))}
          </>
        )}
      </Show>
    </div>
  );
}

function RunDetail(props: { run: RunAuditDetailSnapshot }) {
  return (
    <div class="run-audit-detail">
      <h3>{providerNames[props.run.provider]} run detail</h3>
      <h4>Thinking</h4>
      <Show when={props.run.thinkingDecision} fallback={<p>No thinking decision was recorded for this run.</p>}>
        <p>
          Requested: {props.run.thinkingDecision.requestedEffort ?? "Provider default"} · configured: {props.run.thinkingConfiguration ? props.run.thinkingConfiguration.configuredEffort ?? "not set" : "unreported"} · model: {props.run.thinkingConfiguration?.model ?? "unreported"}
        </p>
        <p>{props.run.thinkingDecision.reason}</p>
        <p class="route-note">Configured effort reports a provider setting, not a thinking-token count.</p>
      </Show>
      <p>{props.run.routing?.explanation ?? "No routing decision was recorded for this run."}</p>
      <p><strong>Reason:</strong> {props.run.reason ? humanize(props.run.reason) : "Unavailable"}</p>
      <Show when={props.run.routingTruncated}><p>Detailed routing evaluation exceeded the audit display limit.</p></Show>
      <Show when={props.run.handoff} fallback={<p>No handoff was sent for this run.</p>}><pre class="handoff-content">{props.run.handoff}</pre></Show>
      <Show when={props.run.handoffTruncated}><p class="truncation-note">The stored handoff exceeds the bounded audit view.</p></Show>
    </div>
  );
}

function InspectorSection(props: { id: string; title: string; collapsed: boolean; onToggle(id: string): void; children: JSX.Element }) {
  return (
    <section class="inspector-section" aria-labelledby={`inspector-${props.id}-heading`}>
      <div class="inspector-section-heading">
        <h2 id={`inspector-${props.id}-heading`}>{props.title}</h2>
        <button type="button" class="disclosure-button" aria-expanded={props.collapsed ? "false" : "true"} aria-controls={`inspector-${props.id}-content`} aria-label={`${props.collapsed ? "Expand" : "Collapse"} ${props.id}`} onClick={() => props.onToggle(props.id)}>
          {props.collapsed ? "+" : "−"}
        </button>
      </div>
      <Show when={!props.collapsed}><div id={`inspector-${props.id}-content`}>{props.children}</div></Show>
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
    case "rankedCandidates": return `Ranked: ${criterion.candidates.map(({ provider, recentRootRuns, stableOrder }) => `${providerNames[provider]} (${recentRootRuns} recent root runs, stable order ${stableOrder})`).join(", ")}`;
    case "safeFallback": return `Safe fallback: ${providerNames[criterion.from]} to ${providerNames[criterion.to]}`;
  }
}

function cleanupLabel(blocker: CleanupBlocker | null) {
  return blocker ? humanize(blocker) : "Unknown reason";
}

function humanize(value: string) {
  return value.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase().replace(/^./, (letter) => letter.toUpperCase());
}

function mergeRunAudits(newest: readonly RunAuditSummarySnapshot[], disclosed: readonly RunAuditSummarySnapshot[]) {
  const byId = new Map(disclosed.map((run) => [run.id, run]));
  newest.forEach((run) => byId.set(run.id, run));
  return [...newest, ...disclosed.filter((run) => !newest.some(({ id }) => id === run.id))].map((run) => byId.get(run.id)!).slice(0, MAX_RUN_AUDIT_ITEMS);
}

function messageFor(reason: unknown) {
  return reason instanceof Error ? reason.message : "Prompting Time could not inspect this conversation.";
}
