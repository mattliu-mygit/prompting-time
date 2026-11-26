import { createMemo, createSignal, onSettled, Show, untrack } from "solid-js";
import { CloseIcon, CommandPalette, FilterIcon, IconButton, Menu, PanelLeftIcon, PanelRightIcon, SearchIcon } from "@prompting-time/ui";
import type { ProviderId } from "../bridge/types";
import { ConversationNavigation } from "../features/conversations/ConversationNavigation";
import { Inspector } from "../features/inspector/Inspector";
import { Timeline, type TimelineViewState } from "../features/timeline/Timeline";
import { AllWork, waitingRoots } from "../features/work/AllWork";
import { createSnapshot, effectiveConversationStatus, selectVisibleConversations, type AppSnapshot, type AppStore, type StatusFilter } from "./store";

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
const statusWords = { idle: "ready", queued: "queued", running: "working", waiting: "waiting for you", completed: "completed", interrupted: "interrupted", failed: "failed" } as const;
const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
const NARROW_INSPECTOR = "(max-width: 72rem)";

/**
 * The command center: the All work list, the selected conversation's timeline, and the inspector.
 * This first Solid version only reads: it shows conversations and their activity but cannot send,
 * answer requests, create, or archive.
 */
export function App(props: { store: AppStore }) {
  // One store for the app's lifetime. Read once: a prop written as a call, such as
  // store={createAppStore(api)}, would make a new store on every read.
  const store = untrack(() => props.store);
  const snapshot = createSnapshot(store);
  const timelineViews = new Map<string, TimelineViewState>();
  const [sidebarOpen, setSidebarOpen] = createSignal(true);
  const [inspectorOpen, setInspectorOpen] = createSignal(false);
  const [narrow, setNarrow] = createSignal(typeof window.matchMedia === "function" && window.matchMedia(NARROW_INSPECTOR).matches);
  const [paletteOpen, setPaletteOpen] = createSignal(false);
  let inspectorTrigger: HTMLButtonElement | undefined;
  let inspectorClose: HTMLButtonElement | undefined;

  const selected = createMemo(() => {
    const id = snapshot().selectedConversationId;
    return id ? snapshot().conversationsById[id] ?? null : null;
  });
  const selectedId = createMemo(() => selected()?.id ?? null);
  const parent = () => {
    const parentId = selected()?.parentId;
    return parentId ? snapshot().conversationsById[parentId] : undefined;
  };
  const overlay = () => narrow() && inspectorOpen();
  const paletteAvailable = () => snapshot().phase === "ready" && !overlay();
  const waiting = () => waitingRoots(snapshot().conversationIds.flatMap((id) => snapshot().conversationsById[id] ?? []));
  const filterLabel = () => `Filter conversations: ${statusOptions.find((option) => option.value === snapshot().statusFilter)?.label}`;

  function selectConversation(id: string) {
    void store.selectConversation(id);
    if (window.matchMedia?.("(max-width: 46rem)").matches) setSidebarOpen(false);
  }

  /** ⌘J: the next root waiting on you after the selected one, in list order, wrapping around. */
  function jumpToNextWaiting() {
    const roots = waiting();
    if (roots.length === 0) return;
    const index = roots.findIndex(({ id }) => id === selectedId());
    const next = roots[(index + 1) % roots.length]!;
    if (next.id !== selectedId()) selectConversation(next.id);
  }

  function closeInspector() {
    setInspectorOpen(false);
    queueMicrotask(() => inspectorTrigger?.focus());
  }

  onSettled(() => {
    void store.initialize();
    const query = typeof window.matchMedia === "function" ? window.matchMedia(NARROW_INSPECTOR) : null;
    const updateNarrow = () => setNarrow(query?.matches ?? false);
    query?.addEventListener("change", updateNarrow);
    // ⌘K (or Ctrl+K) toggles the palette and ⌘J (or Ctrl+J) jumps to the next item waiting on you,
    // also while typing. Composition input and held keys do nothing.
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey || event.isComposing || event.keyCode === 229 || event.repeat) return;
      const key = event.key.toLowerCase();
      if (key === "k" && (paletteOpen() || paletteAvailable())) {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      } else if (key === "j" && snapshot().phase === "ready" && !paletteOpen() && !overlay()) {
        event.preventDefault();
        jumpToNextWaiting();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      query?.removeEventListener("change", updateNarrow);
      window.removeEventListener("keydown", onKeyDown);
    };
  });

  function onInspectorKeyDown(event: KeyboardEvent) {
    if (!narrow()) return;
    if (event.key === "Escape") {
      event.preventDefault();
      closeInspector();
      return;
    }
    if (event.key !== "Tab") return;
    const controls = [...(event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>('button:not(:disabled), select:not(:disabled), input:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])')];
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

  const searchButton = () => (
    <IconButton label="Search (⌘K)" disabled={!paletteAvailable()} onClick={() => setPaletteOpen(true)}>
      <SearchIcon />
    </IconButton>
  );

  return (
    <Show when={snapshot().phase === "ready"} fallback={<StartupState store={store} snapshot={snapshot()} />}>
      <div class="app-shell">
        <header class="app-toolbar" inert={overlay()}>
          <IconButton
            label={sidebarOpen() ? "Hide All work" : "Show All work"}
            aria-expanded={sidebarOpen() ? "true" : "false"}
            aria-controls="work-pane"
            onClick={() => setSidebarOpen((open) => !open)}
          >
            <PanelLeftIcon />
          </IconButton>
          <div class="shell-title">
            <h1 title={selected()?.title ?? "Prompting Time"}>{selected()?.title ?? "Prompting Time"}</h1>
            <Show when={selected()}>
              {(conversation) => (
                <span class="shell-meta">
                  {conversation().provider ? providerNames[conversation().provider!] : "No provider"} · {statusWords[effectiveConversationStatus(conversation())]}
                  {conversation().projectRoot ? ` · ${conversation().projectRoot!.split(/[\\/]/).filter(Boolean).at(-1)}` : ""}
                </span>
              )}
            </Show>
          </div>
          <div class="toolbar-actions">
            <Show when={!sidebarOpen()}>{searchButton()}</Show>
            <IconButton
              ref={inspectorTrigger}
              label={inspectorOpen() ? "Hide inspector" : "Show inspector"}
              aria-expanded={inspectorOpen() ? "true" : "false"}
              aria-controls="inspector-pane"
              onClick={() => setInspectorOpen((open) => !open)}
            >
              <PanelRightIcon />
            </IconButton>
          </div>
        </header>

        <div class="command-center">
          <Show when={sidebarOpen()}>
            <aside id="work-pane" class="sidebar-pane" aria-label="All work" inert={overlay()}>
              <div class="sidebar-toolbar">
                <h2>All work</h2>
                <div class="toolbar-actions">
                  {searchButton()}
                  <Menu
                    label={filterLabel()}
                    icon={<FilterIcon />}
                    items={statusOptions.map((option) => ({ value: option.value, label: option.value === snapshot().statusFilter ? `${option.label} (shown)` : option.label }))}
                    onSelect={(value) => store.setStatusFilter(value as StatusFilter)}
                  />
                </div>
              </div>
              <Show when={snapshot().queuedCount > 0}><p class="queue-count">{snapshot().queuedCount} queued</p></Show>
              <AllWork
                conversations={selectVisibleConversations(snapshot())}
                conversationsById={snapshot().conversationsById}
                childPagesById={snapshot().childPagesById}
                expandedById={snapshot().expandedById}
                selectedId={selectedId()}
                selectedPath={snapshot().selectedPath}
                onSelect={selectConversation}
                onToggle={(id) => store.toggleConversation(id)}
                onLoadChildPage={(id, restart) => void store.loadChildPage(id, restart)}
              />
            </aside>
          </Show>

          <main class="workspace-pane" aria-label="Conversation workspace" inert={overlay()}>
            <Show
              when={selectedId()}
              keyed
              fallback={
                <section class="empty-workspace">
                  <h2>No conversation selected</h2>
                  <p>Choose a conversation from All work.</p>
                </section>
              }
            >
              {(id: string) => (
                <>
                  <ConversationNavigation
                    conversation={snapshot().conversationsById[id]!}
                    conversationsById={snapshot().conversationsById}
                    path={snapshot().selectedPath}
                    onSelect={(id) => {
                      selectConversation(id);
                      queueMicrotask(() => document.getElementById("timeline-heading")?.focus({ preventScroll: true }));
                    }}
                  />
                  <SelectedTimeline store={store} snapshot={snapshot} conversationId={id} views={timelineViews} />
                </>
              )}
            </Show>
          </main>

          <Show when={inspectorOpen()}>
            <aside id="inspector-pane" class={["inspector-pane", { overlay: narrow() }]} aria-label="Inspector" onKeyDown={onInspectorKeyDown}>
              <div class="pane-heading">
                <h2>Inspector</h2>
                <IconButton
                  ref={(element: HTMLButtonElement) => {
                    inspectorClose = element;
                    if (narrow()) queueMicrotask(() => inspectorClose?.focus());
                  }}
                  label="Close inspector"
                  onClick={closeInspector}
                >
                  <CloseIcon />
                </IconButton>
              </div>
              <StartupDiagnostic snapshot={snapshot()} />
              <Show when={selectedId()} keyed fallback={<ProviderList snapshot={snapshot()} />}>
                {(id) => (
                  <Inspector
                    conversation={snapshot().conversationsById[id]!}
                    providers={snapshot().bootstrap?.providers ?? []}
                    refreshVersion={snapshot().conversationVersions[id] ?? 0}
                    actions={store.actions}
                  />
                )}
              </Show>
            </aside>
          </Show>
        </div>
        <Show when={snapshot().error}><p role="alert" class="inline-error lifecycle-error">{snapshot().error}</p></Show>
        <CommandPalette
          open={paletteOpen()}
          label="Search conversations and commands"
          placeholder="Search conversations and commands…"
          emptyText="No conversations or commands found."
          groups={[
            {
              heading: "Conversations",
              items: Object.values(snapshot().conversationsById).filter(({ archived }) => !archived).map((conversation) => {
                const parentTitle = conversation.parentId ? snapshot().conversationsById[conversation.parentId]?.title : undefined;
                return {
                  id: `conversation:${conversation.id}`,
                  label: conversation.title,
                  detail: conversation.parentId ? parentTitle ?? "Earlier ancestry" : conversation.projectRoot ?? undefined,
                  keywords: [conversation.projectRoot ?? "", parentTitle ?? ""],
                  current: conversation.id === selectedId(),
                };
              }),
            },
            {
              heading: "Commands",
              items: [
                { id: "command:next-waiting", label: "Next item waiting on you (⌘J)", disabled: waiting().length === 0 },
                { id: "command:parent", label: "Go to parent conversation", disabled: !parent() },
                { id: "command:sidebar", label: sidebarOpen() ? "Hide All work" : "Show All work" },
                { id: "command:inspector", label: inspectorOpen() ? "Hide inspector" : "Show inspector" },
              ],
            },
          ]}
          onClose={() => setPaletteOpen(false)}
          onSelect={(id) => {
            if (id.startsWith("conversation:")) selectConversation(id.slice("conversation:".length));
            else if (id === "command:next-waiting") jumpToNextWaiting();
            else if (id === "command:parent") {
              const target = parent();
              if (target) selectConversation(target.id);
            } else if (id === "command:sidebar") setSidebarOpen((open) => !open);
            else if (id === "command:inspector") setInspectorOpen((open) => !open);
          }}
        />
      </div>
    </Show>
  );
}

/** The selected conversation's timeline. App makes one per conversation, so each starts clean. */
function SelectedTimeline(props: { store: AppStore; snapshot: () => AppSnapshot; conversationId: string; views: Map<string, TimelineViewState> }) {
  return (
    <Show when={props.snapshot().conversationsById[props.conversationId]}>
      {(conversation) => (
        <Timeline
          conversation={conversation()}
          refreshVersion={props.snapshot().conversationVersions[props.conversationId] ?? 0}
          actions={props.store.actions}
          viewStates={props.views}
        />
      )}
    </Show>
  );
}

function StartupState(props: { store: AppStore; snapshot: AppSnapshot }) {
  return (
    <Show
      when={props.snapshot.phase === "error"}
      fallback={
        <main class="startup-state" aria-busy="true">
          <p class="eyebrow">Prompting Time</p>
          <h1>Opening command center…</h1>
        </main>
      }
    >
      <main class="startup-state">
        <p class="eyebrow">Prompting Time</p>
        <h1>Command center unavailable</h1>
        <p>{props.snapshot.error}</p>
        <StartupDiagnostic snapshot={props.snapshot} />
        <button type="button" class="pt-button" data-variant="primary" onClick={() => void props.store.retry()}>Retry</button>
      </main>
    </Show>
  );
}

function StartupDiagnostic(props: { snapshot: AppSnapshot }) {
  return (
    <Show when={props.snapshot.bootstrap?.startupDiagnostic}>
      {(diagnostic) => (
        <section class="diagnostic-card" aria-label="Startup diagnostic">
          <p>{diagnostic().message}</p>
          <Show when={diagnostic().action}><p>{diagnostic().action}</p></Show>
        </section>
      )}
    </Show>
  );
}

function ProviderList(props: { snapshot: AppSnapshot }) {
  return (
    <section aria-labelledby="provider-heading">
      <h2 id="provider-heading" class="section-heading">Providers</h2>
      <ul class="provider-list">
        {(props.snapshot.bootstrap?.providers ?? []).map((provider) => (
          <li class="provider-diagnostic">
            <span class="provider-indicator" data-available={provider.available ? "true" : "false"} aria-hidden="true" />
            <span>{provider.available && provider.version ? `${providerNames[provider.id]} ${provider.version}` : `${providerNames[provider.id]} unavailable: ${provider.diagnostic ?? "Not installed"}`}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
