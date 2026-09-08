import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useHotkeys } from "react-hotkeys-hook";
import type { CreateConversationRequest, ProviderInstallation } from "../bridge/types";
import { ConversationTree } from "../features/conversations/ConversationTree";
import { Inspector } from "../features/inspector/Inspector";
import { Composer } from "../features/timeline/Composer";
import { Timeline, type TimelineViewState } from "../features/timeline/Timeline";
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

const CommandPalette = lazy(() => import("../features/commands/CommandPalette"));

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
  const timelineViews = useMemo(() => new Map<string, TimelineViewState>(), [store]);
  const snapshot = useAppStore((state) => state);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [narrowInspector, setNarrowInspector] = useState(() => (
    typeof window.matchMedia === "function" && window.matchMedia("(max-width: 72rem)").matches
  ));
  const [creatingConversation, setCreatingConversation] = useState(false);
  const [archiveTarget, setArchiveTarget] = useState<string | null>(null);
  const [archiveSubmitting, setArchiveSubmitting] = useState(false);
  const [lifecycleError, setLifecycleError] = useState<string | null>(null);
  const [composerModalOpen, setComposerModalOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState<boolean | null>(null);
  const [messageFocusRequested, setMessageFocusRequested] = useState(false);
  const inspectorTrigger = useRef<HTMLButtonElement>(null);
  const inspectorClose = useRef<HTMLButtonElement>(null);
  const newConversationTrigger = useRef<HTMLButtonElement>(null);
  const archiveTrigger = useRef<HTMLButtonElement>(null);
  const composerMessage = useRef<HTMLTextAreaElement>(null);
  const focusCreatedComposer = useRef(false);
  const lifecycleModalOpen = creatingConversation || archiveTarget !== null;
  const paletteAvailable = snapshot.phase === "ready" && !lifecycleModalOpen
    && !composerModalOpen && !(narrowInspector && inspectorOpen);

  useHotkeys(["meta+k", "ctrl+k"], () => setPaletteOpen((open) => !open), {
    enabled: paletteOpen === true || paletteAvailable,
    enableOnFormTags: true,
    enableOnContentEditable: true,
    ignoreEventWhen: (event) => event.isComposing || event.keyCode === 229 || event.repeat,
    preventDefault: true,
  });

  useEffect(() => {
    void store.initialize();
  }, [store]);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia("(max-width: 72rem)");
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

  useEffect(() => {
    if (!focusCreatedComposer.current || lifecycleModalOpen) return;
    if (narrowInspector && inspectorOpen) {
      setInspectorOpen(false);
      return;
    }
    if (!composerMessage.current) return;
    composerMessage.current.focus();
    focusCreatedComposer.current = false;
  }, [lifecycleModalOpen, narrowInspector, inspectorOpen, snapshot.selectedConversationId]);

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
        {snapshot.bootstrap?.startupDiagnostic ? (
          <section className="diagnostic-card" aria-label="Startup diagnostic">
            <p>{snapshot.bootstrap.startupDiagnostic.message}</p>
            {snapshot.bootstrap.startupDiagnostic.action ? (
              <p>{snapshot.bootstrap.startupDiagnostic.action}</p>
            ) : null}
          </section>
        ) : null}
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

  function openNewConversation() {
    setLifecycleError(null);
    setCreatingConversation(true);
  }

  function toggleSidebar() {
    setSidebarOpen((open) => !open);
  }

  function toggleInspector() {
    setInspectorOpen((open) => !open);
  }

  function selectConversation(conversationId: string, agentId?: string) {
    store.selectConversation(conversationId, agentId);
    if (window.matchMedia("(max-width: 46rem)").matches) setSidebarOpen(false);
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
          <button type="button" className="toolbar-button palette-trigger" onClick={() => setPaletteOpen(true)} disabled={!paletteAvailable}>
            Search <kbd aria-hidden="true">⌘K / Ctrl K</kbd>
          </button>
          <button ref={newConversationTrigger} type="button" className="toolbar-button" onClick={openNewConversation}>
            New conversation
          </button>
          <button
            type="button"
            className="toolbar-button sidebar-toggle"
            aria-expanded={sidebarOpen}
            aria-controls="conversation-pane"
            onClick={toggleSidebar}
          >
            {sidebarOpen ? "Hide conversations" : "Show conversations"}
          </button>
          <button
            ref={inspectorTrigger}
            type="button"
            className="toolbar-button"
            aria-expanded={inspectorOpen}
            aria-controls="inspector-pane"
            onClick={toggleInspector}
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
              onSelect={selectConversation}
            />
          </aside>
        ) : null}

        <main className="workspace-pane" aria-label="Conversation workspace" inert={narrowInspector && inspectorOpen}>
          {selectedConversation ? (
            <>
              <div className="workspace-heading">
                <div>
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
                viewStates={timelineViews}
                currentRunId={selectedConversation.currentRunId}
                runStatus={selectedConversation.runStatus}
                refreshVersion={selectedVersion}
                agents={selectedConversation.agents}
                agentsTruncated={selectedConversation.agentsTruncated}
                agentWindow={snapshot.agentWindow?.conversationId === selectedConversation.id ? snapshot.agentWindow : null}
                onLoadAgentPage={(restart) => { void store.loadAgentPage(selectedConversation.id, restart); }}
                actions={store.actions}
              />
              <Composer
                key={`composer-${selectedConversation.id}`}
                store={store}
                conversation={selectedConversation}
                providers={snapshot.bootstrap?.providers ?? []}
                routingProfile={selectedConversation.routingProfile}
                actions={store.actions}
                onMutation={() => store.refreshConversation(selectedConversation.id)}
                onModalChange={setComposerModalOpen}
                messageRef={composerMessage}
                focusRequested={messageFocusRequested}
                onFocusHandled={() => setMessageFocusRequested(false)}
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
      {paletteOpen !== null ? (
        <Suspense fallback={null}>
          <CommandPalette
            open={paletteOpen}
            onOpenChange={setPaletteOpen}
            conversations={Object.values(snapshot.conversationsById).filter(({ archived }) => !archived)}
            selectedId={snapshot.selectedConversationId}
            onSelectConversation={(id) => {
              selectConversation(id);
              setMessageFocusRequested(true);
            }}
            commands={[
              { id: "new", label: "New conversation", run: openNewConversation },
              { id: "sidebar", label: sidebarOpen ? "Hide conversations" : "Show conversations", run: toggleSidebar },
              { id: "inspector", label: inspectorOpen ? "Hide inspector" : "Show inspector", run: toggleInspector },
              { id: "message", label: "Focus message", disabled: !selectedConversation || snapshot.submissionsById[selectedConversation.id]?.pending === true, run: () => setMessageFocusRequested(true) },
            ]}
          />
        </Suspense>
      ) : null}
      {lifecycleError && !creatingConversation && !archiveTarget ? <p role="alert" className="inline-error lifecycle-error">{lifecycleError}</p> : null}
      {creatingConversation ? (
        <NewConversationDialog
          pickProjectDirectory={() => store.pickProjectDirectory()}
          inspectProject={(path) => store.inspectProject(path)}
          onCancel={() => {
            setCreatingConversation(false);
            queueMicrotask(() => newConversationTrigger.current?.focus());
          }}
          onCreate={(request) => store.createConversation(request)}
          onCreated={() => {
            focusCreatedComposer.current = true;
            setCreatingConversation(false);
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
  pickProjectDirectory,
  inspectProject,
  onCancel,
  onCreate,
  onCreated,
}: {
  pickProjectDirectory(): Promise<string | null>;
  inspectProject(path: string): Promise<{ isGit: boolean }>;
  onCancel(): void;
  onCreate(request: CreateConversationRequest): Promise<void>;
  onCreated(): void;
}) {
  const [projectError, setProjectError] = useState<string | null>(null);
  const [executionMode, setExecutionMode] = useState<"isolated" | "direct">("isolated");
  const [routingProfile, setRoutingProfile] = useState<CreateConversationRequest["routingProfile"]>("bestFit");
  const [advanced, setAdvanced] = useState(false);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const lifetime = useRef({ active: false });
  const chooseButton = useRef<HTMLButtonElement>(null);
  const restoreChooserFocus = useRef(false);

  useEffect(() => {
    const current = { active: true };
    lifetime.current = current;
    return () => { current.active = false; };
  }, []);

  useEffect(() => {
    if (busy || !restoreChooserFocus.current) return;
    restoreChooserFocus.current = false;
    chooseButton.current?.focus();
  }, [busy]);

  function cancel() {
    if (!pending.current) onCancel();
  }

  async function create(intent: "folder" | "projectless") {
    if (pending.current) return;
    pending.current = true;
    const current = lifetime.current;
    const options = { executionMode, routingProfile };
    setBusy(true);
    setProjectError(null);
    try {
      let title = "New conversation";
      let workspace: CreateConversationRequest["workspace"] = { kind: "projectless" };
      if (intent === "folder") {
        const path = await pickProjectDirectory();
        if (!current.active || path === null) return;
        const { isGit } = await inspectProject(path);
        if (!current.active) return;
        title = path.replace(/\/+$/, "").split("/").pop() || title;
        workspace = { kind: isGit ? options.executionMode : "direct", path };
      }
      await onCreate({ title, objective: "", constraints: [], workspace, routingProfile: options.routingProfile });
      if (current.active) onCreated();
    } catch (reason) {
      if (current.active) setProjectError(messageFor(reason, "Prompting Time could not create the conversation."));
    } finally {
      pending.current = false;
      if (current.active) {
        restoreChooserFocus.current = true;
        setBusy(false);
      }
    }
  }

  return (
    <div className="dialog-backdrop">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-conversation-title"
        className="confirm-dialog conversation-dialog"
        aria-busy={busy}
        onKeyDown={(event) => trapLifecycleDialog(event, cancel, pending.current)}
      >
        <h2 id="new-conversation-title">New conversation</h2>
        <p>Git folders use an isolated worktree by default, without uncommitted edits. Non-Git folders are used directly.</p>
        {projectError ? <p role="alert" className="inline-error">{projectError}</p> : null}
        <button ref={chooseButton} type="button" className="primary-button" autoFocus disabled={busy} onClick={() => void create("folder")}>Choose folder</button>
        <button type="button" className="secondary-button" disabled={busy} onClick={() => void create("projectless")}>Without a folder</button>
        <button type="button" className="secondary-button" aria-expanded={advanced} aria-controls="new-conversation-advanced" disabled={busy} onClick={() => setAdvanced((open) => !open)}>Advanced</button>
        {advanced ? (
          <div id="new-conversation-advanced" className="conversation-options">
            <label>
              <span>Git execution</span>
              <select value={executionMode} disabled={busy} onChange={(event) => setExecutionMode(event.target.value as typeof executionMode)}>
                <option value="isolated">Isolated worktree</option>
                <option value="direct">Current checkout</option>
              </select>
            </label>
            <p>This execution option applies only to Git folders.</p>
            <label>
              <span>Routing profile</span>
              <select value={routingProfile} disabled={busy} onChange={(event) => setRoutingProfile(event.target.value as typeof routingProfile)}>
                <option value="bestFit">Best fit</option>
                <option value="balanced">Balanced</option>
                <option value="usageBalance">Usage balance</option>
              </select>
            </label>
          </div>
        ) : null}
        <div className="dialog-actions">
          <button type="button" className="secondary-button" disabled={busy} onClick={cancel}>Cancel</button>
        </div>
      </div>
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
  if (!first || !last) {
    event.preventDefault();
    return;
  }
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
