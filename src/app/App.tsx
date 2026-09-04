import { useEffect, useState } from "react";
import type { ProviderInstallation } from "../bridge/types";
import { ConversationTree } from "../features/conversations/ConversationTree";
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

  useEffect(() => {
    void store.initialize();
  }, [store]);

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

  return (
    <div className="app-shell">
      <header className="app-toolbar">
        <div className="brand-lockup">
          <span className="brand-mark" aria-hidden="true">PT</span>
          <span>Prompting Time</span>
        </div>
        <div className="toolbar-actions">
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

      <div className="command-center">
        {sidebarOpen ? (
          <aside id="conversation-pane" className="sidebar-pane" aria-label="Conversations">
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
              onSelect={(conversationId, agentId) => {
                store.selectConversation(conversationId, agentId);
                if (window.matchMedia("(max-width: 46rem)").matches) setSidebarOpen(false);
              }}
            />
          </aside>
        ) : null}

        <main className="workspace-pane" aria-label="Conversation workspace">
          {selected ? (
            <>
              <div className="workspace-heading">
                <div>
                  <p className="eyebrow">Current conversation</p>
                  <h1>{selected.title}</h1>
                </div>
                {selected.provider ? (
                  <span className="provider-badge">{providerNames[selected.provider]}</span>
                ) : null}
              </div>
              <section className="workspace-placeholder" aria-label="Timeline placeholder">
                <p>The shared timeline and composer arrive in the next implementation slice.</p>
              </section>
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
          <aside id="inspector-pane" className="inspector-pane" aria-label="Inspector">
            <div className="pane-heading">
              <div>
                <p className="eyebrow">System</p>
                <h1>Inspector</h1>
              </div>
              <button
                type="button"
                className="icon-button"
                aria-label="Close inspector"
                onClick={() => setInspectorOpen(false)}
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
            <section aria-labelledby="provider-heading">
              <h2 id="provider-heading" className="section-heading">Providers</h2>
              <ul className="provider-list">
                {snapshot.bootstrap?.providers.map((provider) => (
                  <ProviderDiagnostic key={provider.id} provider={provider} />
                ))}
              </ul>
            </section>
          </aside>
        ) : null}
      </div>
    </div>
  );
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
