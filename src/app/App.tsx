import { useEffect, useRef, useState } from "react";
import type { CreateConversationRequest, ProviderInstallation } from "../bridge/types";
import { ConversationTree } from "../features/conversations/ConversationTree";
import { Inspector } from "../features/inspector/Inspector";
import { Composer } from "../features/timeline/Composer";
import { Timeline } from "../features/timeline/Timeline";
import {
  AppStoreContext,
  selectVisibleConversations,
  useAppStore,
  type AppStore,
  type StatusFilter,
} from "./store";

type AppProps = {
  store: AppStore;
};

const statusOptions: Array<{ value: StatusFilter; label: string }> = [
  { value: "all", label: "All statuses" },
  { value: "idle", label: "Ready" },
  { value: "queued", label: "Queued" },
  { value: "running", label: "Running" },
  { value: "waiting", label: "Waiting" },
  { value: "completed", label: "Completed" },
  { value: "interrupted", label: "Interrupted" },
  { value: "failed", label: "Failed" },
];

const providerNames = {
  codex: "Codex",
  claude: "Claude",
} as const;

export function App({ store }: AppProps) {
  return (
    <AppStoreContext.Provider value={store}>
      <CommandCenter store={store} />
    </AppStoreContext.Provider>
  );
}

function CommandCenter({ store }: { store: AppStore }) {
  const snapshot = useAppStore((state) => state);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [inspectorOpen, setInspectorOpen] = useState(true);
  const [narrowInspector, setNarrowInspector] = useState(() => (
    typeof window.matchMedia === "function" && window.matchMedia("(max-width: 56rem)").matches
  ));
  const [creatingConversation, setCreatingConversation] = useState(false);
  const [archiveTarget, setArchiveTarget] = useState<string | null>(null);
  const [archiveSubmitting, setArchiveSubmitting] = useState(false);
  const [lifecycleError, setLifecycleError] = useState<string | null>(null);
  const [composerModalOpen, setComposerModalOpen] = useState(false);
  const inspectorTrigger = useRef<HTMLButtonElement>(null);
  const inspectorClose = useRef<HTMLButtonElement>(null);
  const newConversationTrigger = useRef<HTMLButtonElement>(null);
  const archiveTrigger = useRef<HTMLButtonElement>(null);
  const lifecycleModalOpen = creatingConversation || archiveTarget !== null;

  useEffect(() => {
    void store.initialize();
  }, [store]);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia("(max-width: 56rem)");
    const updateNarrow = () => setNarrowInspector(query.matches);
    query.addEventListener("change", updateNarrow);
    return () => query.removeEventListener("change", updateNarrow);
  }, []);

  useEffect(() => {
    if (inspectorOpen && narrowInspector) queueMicrotask(() => inspectorClose.current?.focus());
  }, [inspectorOpen, narrowInspector, snapshot.phase]);

  useEffect(() => {
    if (archiveTarget && !snapshot.conversationsById[archiveTarget]) setArchiveTarget(null);
  }, [archiveTarget, snapshot.conversationsById]);

  if (snapshot.phase === "idle" || snapshot.phase === "loading") {
    return (
      <main className="startup-state" aria-busy="true">
        <p className="eyebrow">Prompting Time</p>
        <h1>Opening command center…</h1>
      </main>
    );
  }

  if (snapshot.phase === "error") {
    return (
      <main className="startup-state">
        <p className="eyebrow">Prompting Time</p>
        <h1>Command center unavailable</h1>
        <p>{snapshot.error}</p>
        <button type="button" className="primary-button" onClick={() => void store.retry()}>
          Retry
        </button>
      </main>
    );
  }

  const conversations = selectVisibleConversations(snapshot);
  const selected = snapshot.selectedConversationId
    ? snapshot.conversationsById[snapshot.selectedConversationId]
    : null;
  const selectedConversation = selected ? {
    ...selected,
    agents: selected.agentIds.flatMap((id) => {
      const agent = snapshot.agentsById[id];
      return agent ? [agent] : [];
    }),
  } : null;
  const selectedVersion = selected
    ? snapshot.conversationVersions[selected.id] ?? 0
    : 0;

  function closeInspector() {
    setInspectorOpen(false);
    queueMicrotask(() => inspectorTrigger.current?.focus());
  }

  function handleInspectorKeyDown(event: React.KeyboardEvent<HTMLElement>) {
    if (!narrowInspector) return;
    if (event.key === "Escape") {
      event.preventDefault();
      closeInspector();
      return;
    }
    if (event.key !== "Tab") return;
    const controls = [...event.currentTarget.querySelectorAll<HTMLElement>(
      'button:not(:disabled), select:not(:disabled), input:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
    )];
    const first = controls[0];
    const last = controls.at(-1);
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return (
    <div className="app-shell" inert={composerModalOpen}>
      <header className="app-toolbar" inert={lifecycleModalOpen || (narrowInspector && inspectorOpen)}>
        <div className="brand-lockup">
          <span className="brand-mark" aria-hidden="true">PT</span>
          <span>Prompting Time</span>
        </div>
        <div className="toolbar-actions">
          <button ref={newConversationTrigger} type="button" className="toolbar-button" onClick={() => {
            setLifecycleError(null);
            setCreatingConversation(true);
          }}>
            New conversation
          </button>
          <button
            type="button"
            className="toolbar-button sidebar-toggle"
            aria-expanded={sidebarOpen}
            aria-controls="conversation-pane"
            onClick={() => setSidebarOpen((open) => !open)}
          >
            {sidebarOpen ? "Hide conversations" : "Show conversations"}
          </button>
          <button
            ref={inspectorTrigger}
            type="button"
            className="toolbar-button"
            aria-expanded={inspectorOpen}
            aria-controls="inspector-pane"
            onClick={() => setInspectorOpen((open) => !open)}
          >
            {inspectorOpen ? "Hide inspector" : "Show inspector"}
          </button>
        </div>
      </header>

      <div className="command-center" inert={lifecycleModalOpen}>
        {sidebarOpen ? (
          <aside id="conversation-pane" className="sidebar-pane" aria-label="Conversations" inert={narrowInspector && inspectorOpen}>
            <div className="pane-heading">
              <div>
                <p className="eyebrow">Workspace</p>
                <h1>Conversations</h1>
              </div>
              {snapshot.queuedCount > 0 ? (
                <span className="queue-count">{snapshot.queuedCount} queued</span>
              ) : null}
            </div>
            <label className="filter-control">
              <span>Filter conversations</span>
              <select
                value={snapshot.statusFilter}
                onChange={(event) => store.setStatusFilter(event.target.value as StatusFilter)}
              >
                {statusOptions.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </label>
            <ConversationTree
              conversations={conversations}
              selectedId={snapshot.selectedConversationId}
              selectedAgentId={snapshot.selectedAgentId}
              statusFilter={snapshot.statusFilter}
              agentWindow={snapshot.agentWindow}
              onLoadAgentPage={(conversationId, restart) => {
                void store.loadAgentPage(conversationId, restart);
              }}
              onSelect={(conversationId, agentId) => {
                store.selectConversation(conversationId, agentId);
                if (window.matchMedia("(max-width: 46rem)").matches) setSidebarOpen(false);
              }}
            />
          </aside>
        ) : null}

        <main className="workspace-pane" aria-label="Conversation workspace" inert={narrowInspector && inspectorOpen}>
          {selectedConversation ? (
            <>
              <div className="workspace-heading">
                <div>
                  <p className="eyebrow">Current conversation</p>
                  <h1>{selectedConversation.title}</h1>
                </div>
                {selectedConversation.provider ? (
                  <span className="provider-badge">{providerNames[selectedConversation.provider]}</span>
                ) : null}
                <button ref={archiveTrigger} type="button" className="secondary-button" onClick={() => {
                  setLifecycleError(null);
                  setArchiveTarget(selectedConversation.id);
                }}>
                  Archive conversation
                </button>
              </div>
              <Timeline
                key={`timeline-${selectedConversation.id}`}
                conversationId={selectedConversation.id}
                refreshVersion={selectedVersion}
                agents={selectedConversation.agents}
                agentsTruncated={selectedConversation.agentsTruncated}
                agentWindow={snapshot.agentWindow?.conversationId === selectedConversation.id ? snapshot.agentWindow : null}
                onLoadAgentPage={(restart) => { void store.loadAgentPage(selectedConversation.id, restart); }}
                actions={store.actions}
              />
              <Composer
                key={`composer-${selectedConversation.id}`}
                conversation={selectedConversation}
                providers={snapshot.bootstrap?.providers ?? []}
                routingProfile={selectedConversation.routingProfile}
                actions={store.actions}
                onMutation={() => store.refreshConversation(selectedConversation.id)}
                onModalChange={setComposerModalOpen}
              />
            </>
          ) : (
            <section className="empty-workspace">
              <p className="eyebrow">Prompting Time</p>
              <h1>No conversation selected</h1>
              <p>Choose a conversation from the sidebar to open its command center.</p>
            </section>
          )}
        </main>

        {inspectorOpen ? (
          <aside id="inspector-pane" className="inspector-pane" aria-label="Inspector" onKeyDown={handleInspectorKeyDown}>
            <div className="pane-heading">
              <div>
                <p className="eyebrow">System</p>
                <h1>Inspector</h1>
              </div>
              <button
                ref={inspectorClose}
                type="button"
                className="icon-button"
                aria-label="Close inspector"
                onClick={closeInspector}
              >
                ×
              </button>
            </div>
            {snapshot.bootstrap?.startupDiagnostic ? (
              <section className="diagnostic-card" aria-label="Startup diagnostic">
                <p>{snapshot.bootstrap.startupDiagnostic.message}</p>
                {snapshot.bootstrap.startupDiagnostic.action ? (
                  <p>{snapshot.bootstrap.startupDiagnostic.action}</p>
                ) : null}
              </section>
            ) : null}
            {selectedConversation ? (
              <Inspector
                key={`inspector-${selectedConversation.id}`}
                conversation={selectedConversation}
                providers={snapshot.bootstrap?.providers ?? []}
                refreshVersion={selectedVersion}
                actions={store.actions}
              />
            ) : (
              <section aria-labelledby="provider-heading">
                <h2 id="provider-heading" className="section-heading">Providers</h2>
                <ul className="provider-list">
                  {snapshot.bootstrap?.providers.map((provider) => (
                    <ProviderDiagnostic key={provider.id} provider={provider} />
                  ))}
                </ul>
              </section>
            )}
          </aside>
        ) : null}
      </div>
      {lifecycleError && !creatingConversation && !archiveTarget ? <p role="alert" className="inline-error lifecycle-error">{lifecycleError}</p> : null}
      {creatingConversation ? (
        <NewConversationDialog
          error={lifecycleError}
          inspectProject={(path) => store.inspectProject(path)}
          onCancel={() => {
            setCreatingConversation(false);
            queueMicrotask(() => newConversationTrigger.current?.focus());
          }}
          onCreate={async (request) => {
            try {
              await store.createConversation(request);
              setCreatingConversation(false);
              queueMicrotask(() => newConversationTrigger.current?.focus());
            } catch (reason) {
              setLifecycleError(messageFor(reason, "Prompting Time could not create the conversation."));
            }
          }}
        />
      ) : null}
      {archiveTarget && selectedConversation?.id === archiveTarget ? (
        <div className="dialog-backdrop">
          <div role="dialog" aria-modal="true" aria-labelledby="archive-dialog-title" className="confirm-dialog" aria-busy={archiveSubmitting} onKeyDown={(event) => trapLifecycleDialog(event, () => {
            setArchiveTarget(null);
            queueMicrotask(() => archiveTrigger.current?.focus());
          }, archiveSubmitting)}>
            <h2 id="archive-dialog-title">Archive {selectedConversation.title}</h2>
            <p>This removes the conversation from the active sidebar. Its durable history is retained.</p>
            {lifecycleError ? <p role="alert" className="inline-error">{lifecycleError}</p> : null}
            <div className="dialog-actions">
              <button type="button" className="secondary-button" disabled={archiveSubmitting} onClick={() => {
                setArchiveTarget(null);
                queueMicrotask(() => archiveTrigger.current?.focus());
              }}>Keep conversation</button>
              <button type="button" className="danger-button" autoFocus disabled={archiveSubmitting} onClick={async () => {
                if (archiveSubmitting) return;
                setArchiveSubmitting(true);
                try {
                  await store.archiveConversation(archiveTarget);
                  setArchiveTarget(null);
                  queueMicrotask(() => newConversationTrigger.current?.focus());
                } catch (reason) {
                  setLifecycleError(messageFor(reason, "Prompting Time could not archive the conversation."));
                } finally {
                  setArchiveSubmitting(false);
                }
              }}>Confirm archive</button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function NewConversationDialog({
  error,
  inspectProject,
  onCancel,
  onCreate,
}: {
  error: string | null;
  inspectProject(path: string): Promise<{ isGit: boolean }>;
  onCancel(): void;
  onCreate(request: CreateConversationRequest): Promise<void>;
}) {
  const [title, setTitle] = useState("");
  const [objective, setObjective] = useState("");
  const [workspaceKind, setWorkspaceKind] = useState<"projectless" | "project">("projectless");
  const [projectRoot, setProjectRoot] = useState("");
  const [projectCheck, setProjectCheck] = useState<{ path: string; isGit: boolean } | null>(null);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [checkingProject, setCheckingProject] = useState(false);
  const [executionMode, setExecutionMode] = useState<"isolated" | "direct">("isolated");
  const [routingProfile, setRoutingProfile] = useState<CreateConversationRequest["routingProfile"]>("balanced");
  const [submitting, setSubmitting] = useState(false);
  const valid = title.trim() !== "" && objective.trim() !== ""
    && (workspaceKind === "projectless" || projectCheck?.path === projectRoot.trim());

  return (
    <div className="dialog-backdrop">
      <form
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-conversation-title"
        className="confirm-dialog conversation-dialog"
        aria-busy={submitting}
        onKeyDown={(event) => trapLifecycleDialog(event, onCancel, submitting)}
        onSubmit={(event) => {
          event.preventDefault();
          if (!valid || submitting) return;
          setSubmitting(true);
          const workspace: CreateConversationRequest["workspace"] = workspaceKind === "projectless"
            ? { kind: "projectless" }
            : { kind: projectCheck?.isGit ? executionMode : "direct", path: projectRoot.trim() };
          void onCreate({
            title: title.trim(), objective: objective.trim(), constraints: [], workspace, routingProfile,
          }).finally(() => setSubmitting(false));
        }}
      >
        <h2 id="new-conversation-title">New conversation</h2>
        {error ? <p role="alert" className="inline-error">{error}</p> : null}
        <label><span>Title</span><input autoFocus value={title} onChange={(event) => setTitle(event.target.value)} /></label>
        <label><span>Objective</span><textarea rows={3} value={objective} onChange={(event) => setObjective(event.target.value)} /></label>
        <label>
          <span>Workspace</span>
          <select value={workspaceKind} onChange={(event) => setWorkspaceKind(event.target.value as typeof workspaceKind)}>
            <option value="projectless">No project</option>
            <option value="project">Local directory</option>
          </select>
        </label>
        {workspaceKind === "project" ? (
          <>
            <label><span>Project root</span><input value={projectRoot} onChange={(event) => { setProjectRoot(event.target.value); setProjectCheck(null); setProjectError(null); }} /></label>
            <button type="button" className="secondary-button" disabled={!projectRoot.trim() || checkingProject} onClick={async () => {
              setCheckingProject(true);
              setProjectError(null);
              try {
                const result = await inspectProject(projectRoot.trim());
                setProjectCheck({ path: projectRoot.trim(), isGit: result.isGit });
                setExecutionMode("isolated");
              } catch (reason) {
                setProjectCheck(null);
                setProjectError(messageFor(reason, "Prompting Time could not inspect this directory."));
              } finally {
                setCheckingProject(false);
              }
            }}>{checkingProject ? "Checking directory…" : "Check directory"}</button>
            {projectError ? <p role="alert" className="inline-error">{projectError}</p> : null}
            {projectCheck?.isGit ? <label><span>Execution</span><select value={executionMode} onChange={(event) => setExecutionMode(event.target.value as typeof executionMode)}><option value="isolated">Isolated worktree</option><option value="direct">Current checkout</option></select></label> : null}
            {projectCheck && !projectCheck.isGit ? <p>This non-Git directory will be used directly.</p> : null}
          </>
        ) : null}
        <label>
          <span>Routing profile</span>
          <select value={routingProfile} onChange={(event) => setRoutingProfile(event.target.value as CreateConversationRequest["routingProfile"])}>
            <option value="balanced">Balanced</option>
            <option value="bestFit">Best fit</option>
            <option value="usageBalance">Usage balance</option>
          </select>
        </label>
        <div className="dialog-actions">
          <button type="button" className="secondary-button" disabled={submitting} onClick={onCancel}>Cancel</button>
          <button type="submit" className="primary-button" disabled={!valid || submitting}>Create conversation</button>
        </div>
      </form>
    </div>
  );
}

function trapLifecycleDialog(
  event: React.KeyboardEvent<HTMLElement>,
  onCancel: () => void,
  submitting: boolean,
) {
  if (event.key === "Escape" && !submitting) {
    event.preventDefault();
    onCancel();
    return;
  }
  if (event.key !== "Tab") return;
  const controls = [...event.currentTarget.querySelectorAll<HTMLElement>(
    'button:not(:disabled), select:not(:disabled), input:not(:disabled), textarea:not(:disabled)',
  )];
  const first = controls[0];
  const last = controls.at(-1);
  if (!first || !last) return;
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

function messageFor(reason: unknown, fallback: string) {
  return reason instanceof Error ? reason.message : fallback;
}

function ProviderDiagnostic({ provider }: { provider: ProviderInstallation }) {
  const name = providerNames[provider.id];
  const detail = provider.available && provider.version
    ? `${name} ${provider.version}`
    : `${name} unavailable: ${provider.diagnostic ?? "Not installed"}`;
  return (
    <li className="provider-diagnostic">
      <span className="provider-indicator" data-available={provider.available} aria-hidden="true" />
      <span>{detail}</span>
    </li>
  );
}
