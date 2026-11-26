import { createMemo, createSignal, Match, onSettled, Show, Switch, untrack } from "solid-js";
import { CloseIcon, CommandPalette, ConfirmDialog, EllipsisIcon, FilterIcon, IconButton, Menu, PanelLeftIcon, PanelRightIcon, PlusIcon, SearchIcon } from "@prompting-time/ui";
import type { PluginSource, ProviderId } from "../bridge/types";
import { BackgroundAgents, type InstallKind } from "../features/agents/BackgroundAgents";
import { FailedReview } from "../features/agents/FailedReview";
import { InstallPluginDialog } from "../features/agents/InstallPluginDialog";
import { PluginInspector, PluginSettings } from "../features/agents/PluginSettings";
import { ReviewHistory } from "../features/agents/ReviewHistory";
import { pastReviews, pluginReviews, reviewKey, reviewsNeedingYou, reviewState, reviewTitle, type ReviewEntry } from "../features/agents/reviews";
import { describeAgent } from "../features/agents/status";
import { PluginFrame } from "../features/plugins/PluginFrame";
import { ConversationNavigation } from "../features/conversations/ConversationNavigation";
import { Inspector } from "../features/inspector/Inspector";
import { Composer } from "../features/timeline/Composer";
import { Timeline, type TimelineViewState } from "../features/timeline/Timeline";
import { AllWork, waitingRoots } from "../features/work/AllWork";
import { NewConversationDialog } from "./NewConversationDialog";
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

/** What the workspace shows of a plugin instead of a conversation: its settings, its past reviews, or one review in its page. */
type PluginView = { kind: "settings" | "history"; plugin: string } | { kind: "review"; plugin: string; runId: string };

/**
 * The command center: the All work list with its background agents, the selected conversation's
 * timeline and composer or the selected agent's settings, past reviews, or page, and the inspector,
 * with the dialogs that create and archive conversations and install agents.
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
  const [creating, setCreating] = createSignal(false);
  const [archiveTarget, setArchiveTarget] = createSignal<string | null>(null);
  const [archiving, setArchiving] = createSignal(false);
  const [lifecycleError, setLifecycleError] = createSignal<string | null>(null);
  const [composerModalOpen, setComposerModalOpen] = createSignal(false);
  const [messageFocusRequested, setMessageFocusRequested] = createSignal(false);
  // What the workspace shows of a background agent instead of the selected conversation.
  const [pluginView, setPluginView] = createSignal<PluginView | null>(null);
  const [installTarget, setInstallTarget] = createSignal<{ kind: InstallKind; path: string } | null>(null);
  let inspectorTrigger: HTMLButtonElement | undefined;
  let newConversationTrigger: HTMLButtonElement | undefined;
  let inspectorClose: HTMLButtonElement | undefined;

  const selected = createMemo(() => {
    const id = snapshot().selectedConversationId;
    return id ? snapshot().conversationsById[id] ?? null : null;
  });
  const selectedId = createMemo(() => selected()?.id ?? null);
  const selectedPlugin = createMemo(() => snapshot().plugins.items.find(({ name }) => name === pluginView()?.plugin) ?? null);
  const reviewsByPlugin = createMemo(() => pluginReviews(snapshot().plugins));
  const needsYouReviews = createMemo(() => reviewsNeedingYou(reviewsByPlugin()));
  const historyCounts = createMemo(() => Object.fromEntries(Object.entries(reviewsByPlugin()).map(([name, reviews]) => [name, pastReviews(reviews).length])));
  const openRun = () => {
    const view = pluginView();
    return view?.kind === "review" ? view.runId : null;
  };
  const openReviewEntry = () => {
    const view = pluginView();
    return view?.kind === "review" ? reviewsByPlugin()[view.plugin]?.find((review) => review.run_id === view.runId) ?? null : null;
  };
  /**
   * The open review when it failed, which Prompting Time shows itself, since page protocol 1 shows
   * only reviews with findings. It stays the same object while the same run stays open, so a new
   * status does not make a new screen.
   */
  const failedReview = createMemo(() => {
    const review = openReviewEntry();
    return review?.outcome === "failed" ? review : null;
  }, { equals: (previous, next) => previous?.run_id === next?.run_id });
  /** The key of what the list shows as selected: the open review, or the conversation when no plugin shows. */
  const currentKey = () => {
    const view = pluginView();
    if (view?.kind === "review") return reviewKey(view.plugin, view.runId);
    return view ? null : selectedId();
  };
  const parent = () => {
    const parentId = selected()?.parentId;
    return parentId ? snapshot().conversationsById[parentId] : undefined;
  };
  const overlay = () => narrow() && inspectorOpen();
  const lifecycleModalOpen = () => creating() || archiveTarget() !== null || installTarget() !== null;
  const paletteAvailable = () => snapshot().phase === "ready" && !overlay() && !lifecycleModalOpen() && !composerModalOpen();
  const workspaceLabel = () => {
    const view = pluginView();
    if (!view || !selectedPlugin()) return "Conversation workspace";
    return { settings: "Background agent", history: "Past reviews", review: "Review" }[view.kind];
  };
  /** Everything under "Needs you", in list order: plugin reviews, then conversations. */
  const waiting = () => [
    ...needsYouReviews().map(({ plugin, review }) => ({ key: reviewKey(plugin, review.run_id), open: () => openReview(plugin, review.run_id) })),
    ...waitingRoots(snapshot().conversationIds.flatMap((id) => snapshot().conversationsById[id] ?? []))
      .map(({ id }) => ({ key: id, open: () => selectConversation(id) })),
  ];
  const filterLabel = () => `Filter conversations: ${statusOptions.find((option) => option.value === snapshot().statusFilter)?.label}`;

  function selectConversation(id: string) {
    setPluginView(null);
    void store.selectConversation(id);
    if (window.matchMedia?.("(max-width: 46rem)").matches) setSidebarOpen(false);
  }

  function showPlugin(view: PluginView) {
    setPluginView(view);
    if (window.matchMedia?.("(max-width: 46rem)").matches) setSidebarOpen(false);
  }

  const openPlugin = (name: string) => showPlugin({ kind: "settings", plugin: name });
  const openReview = (plugin: string, runId: string) => showPlugin({ kind: "review", plugin, runId });

  /** Picks the archive or folder, then asks in the install dialog; a cancelled pick does nothing. */
  async function startInstall(kind: InstallKind) {
    setLifecycleError(null);
    try {
      const path = kind === "archive" ? await store.pickPluginArchive() : await store.pickPluginFolder();
      if (path) setInstallTarget({ kind, path });
    } catch (reason) {
      setLifecycleError(reason instanceof Error ? reason.message : "Prompting Time could not open the picker.");
    }
  }

  async function install(source: PluginSource) {
    const installed = await store.installPlugin(source);
    setInstallTarget(null);
    openPlugin(installed.name);
  }

  /** ⌘J: the next item waiting on you after the selected one, in list order, wrapping around. */
  function jumpToNextWaiting() {
    const items = waiting();
    if (items.length === 0) return;
    const index = items.findIndex(({ key }) => key === currentKey());
    const next = items[(index + 1) % items.length]!;
    if (next.key !== currentKey()) next.open();
  }

  function openNewConversation() {
    setLifecycleError(null);
    setCreating(true);
  }

  /** After creating: close a narrow inspector overlay, which would hold focus, and focus the composer. */
  function focusCreatedComposer() {
    if (narrow() && inspectorOpen()) setInspectorOpen(false);
    setMessageFocusRequested(true);
  }

  async function confirmArchive() {
    const target = archiveTarget();
    if (!target || archiving()) return;
    setArchiving(true);
    try {
      await store.archiveConversation(target);
      setArchiveTarget(null);
      queueMicrotask(() => newConversationTrigger?.focus());
    } catch (reason) {
      setLifecycleError(reason instanceof Error ? reason.message : "Prompting Time could not archive the conversation.");
    } finally {
      setArchiving(false);
    }
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

  /** The selected conversation's path, timeline, and composer. */
  const conversationWorkspace = () => (
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
          <Show when={snapshot().conversationsById[id]}>
            {(conversation) => (
              <Composer
                conversation={conversation()}
                providers={snapshot().bootstrap?.providers ?? []}
                routingProfile={conversation().routingProfile}
                actions={store.actions}
                store={store}
                onMutation={() => store.refreshConversation(id)}
                onModalChange={setComposerModalOpen}
                focusRequested={messageFocusRequested()}
                onFocusHandled={() => setMessageFocusRequested(false)}
              />
            )}
          </Show>
        </>
      )}
    </Show>
  );

  const conversationActions = () => (
    <>
      <IconButton label="Search (⌘K)" disabled={!paletteAvailable()} onClick={() => setPaletteOpen(true)}>
        <SearchIcon />
      </IconButton>
      <IconButton ref={newConversationTrigger} label="New conversation" onClick={openNewConversation}>
        <PlusIcon />
      </IconButton>
    </>
  );

  return (
    <Show when={snapshot().phase === "ready"} fallback={<StartupState store={store} snapshot={snapshot()} />}>
      <div class="app-shell" inert={composerModalOpen()}>
        <header class="app-toolbar" inert={overlay() || lifecycleModalOpen()}>
          <IconButton
            label={sidebarOpen() ? "Hide All work" : "Show All work"}
            aria-expanded={sidebarOpen() ? "true" : "false"}
            aria-controls="work-pane"
            onClick={() => setSidebarOpen((open) => !open)}
          >
            <PanelLeftIcon />
          </IconButton>
          <Show
            when={selectedPlugin()}
            fallback={
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
            }
          >
            {(plugin) => (
              <Show
                when={openReviewEntry()}
                fallback={
                  <div class="shell-title">
                    <h1>{plugin().title} {pluginView()?.kind === "history" ? "past reviews" : pluginView()?.kind === "review" ? "review" : "settings"}</h1>
                    <span class="shell-meta">Background agent · {describeAgent(plugin(), snapshot().plugins.statuses[plugin().name]).meta}</span>
                  </div>
                }
              >
                {(review) => (
                  <div class="shell-title">
                    <h1 title={reviewTitle(review())}>{reviewTitle(review())}</h1>
                    <span class="shell-meta">{plugin().title} review · {review().repo}#{review().pr} · {reviewState(review()).words}</span>
                  </div>
                )}
              </Show>
            )}
          </Show>
          <div class="toolbar-actions">
            <Show when={!sidebarOpen()}>{conversationActions()}</Show>
            <IconButton
              ref={inspectorTrigger}
              label={inspectorOpen() ? "Hide inspector" : "Show inspector"}
              aria-expanded={inspectorOpen() ? "true" : "false"}
              aria-controls="inspector-pane"
              onClick={() => setInspectorOpen((open) => !open)}
            >
              <PanelRightIcon />
            </IconButton>
            <Show when={!selectedPlugin() && selected()?.capabilities.canArchive}>
              <Menu
                label="More"
                icon={<EllipsisIcon />}
                items={[{ value: "archive", label: "Archive conversation" }]}
                onSelect={() => {
                  setLifecycleError(null);
                  setArchiveTarget(selectedId());
                }}
              />
            </Show>
          </div>
        </header>

        <div class="command-center" inert={lifecycleModalOpen()}>
          <Show when={sidebarOpen()}>
            <aside id="work-pane" class="sidebar-pane" aria-label="All work" inert={overlay()}>
              <div class="sidebar-toolbar">
                <h2>All work</h2>
                <div class="toolbar-actions">
                  {conversationActions()}
                  <Menu
                    label={filterLabel()}
                    icon={<FilterIcon />}
                    items={statusOptions.map((option) => ({ ...option, type: "radio" as const, checked: option.value === snapshot().statusFilter }))}
                    onSelect={(value) => store.setStatusFilter(value as StatusFilter)}
                  />
                </div>
              </div>
              <Show when={snapshot().queuedCount > 0}><p class="queue-count">{snapshot().queuedCount} queued</p></Show>
              <AllWork
                reviews={snapshot().statusFilter === "all" || snapshot().statusFilter === "waiting" ? needsYouReviews() : []}
                selectedReview={pluginView()?.kind === "review" ? currentKey() : null}
                onOpenReview={openReview}
                conversations={selectVisibleConversations(snapshot())}
                conversationsById={snapshot().conversationsById}
                childPagesById={snapshot().childPagesById}
                expandedById={snapshot().expandedById}
                selectedId={selectedPlugin() ? null : selectedId()}
                selectedPath={snapshot().selectedPath}
                onSelect={selectConversation}
                onToggle={(id) => store.toggleConversation(id)}
                onLoadChildPage={(id, restart) => void store.loadChildPage(id, restart)}
              />
              <BackgroundAgents
                plugins={snapshot().plugins}
                selected={pluginView()?.kind === "settings" ? selectedPlugin()?.name ?? null : null}
                history={historyCounts()}
                selectedHistory={pluginView()?.kind === "history" ? selectedPlugin()?.name ?? null : null}
                onSelect={openPlugin}
                onOpenHistory={(name) => showPlugin({ kind: "history", plugin: name })}
                onInstall={(kind) => void startInstall(kind)}
              />
            </aside>
          </Show>

          <main class="workspace-pane" aria-label={workspaceLabel()} inert={overlay()}>
            <Show when={selectedPlugin()?.name} keyed fallback={conversationWorkspace()}>
              {(name: string) => (
                <Show when={snapshot().plugins.items.find((plugin) => plugin.name === name)}>
                  {(plugin) => (
                    <Switch>
                      <Match when={pluginView()?.kind === "review"}>
                        <Show
                          when={failedReview()}
                          keyed
                          fallback={
                            <PluginFrame
                              plugin={plugin()}
                              status={snapshot().plugins.statuses[name]}
                              runId={openRun()}
                              actions={store.pluginActions}
                              onPluginFilesChange={(listener) => store.onPluginFilesChange(listener)}
                              onShowReview={(runId) => {
                                if (openRun() !== runId) setPluginView({ kind: "review", plugin: name, runId });
                              }}
                            />
                          }
                        >
                          {(review: ReviewEntry) => <FailedReview plugin={plugin()} review={review} actions={store.pluginActions} />}
                        </Show>
                      </Match>
                      <Match when={pluginView()?.kind === "history"}>
                        <ReviewHistory plugin={plugin()} status={snapshot().plugins.statuses[name]} onOpen={(runId) => openReview(name, runId)} />
                      </Match>
                      <Match when={pluginView()?.kind === "settings"}>
                        <PluginSettings plugin={plugin()} status={snapshot().plugins.statuses[name]} actions={store.pluginActions} />
                      </Match>
                    </Switch>
                  )}
                </Show>
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
              <Show
                when={selectedPlugin()}
                fallback={
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
                }
              >
                {(plugin) => <PluginInspector plugin={plugin()} />}
              </Show>
            </aside>
          </Show>
        </div>
        <Show when={snapshot().error}><p role="alert" class="inline-error lifecycle-error">{snapshot().error}</p></Show>
        <Show when={lifecycleError() && !creating() && !archiveTarget()}><p role="alert" class="inline-error lifecycle-error">{lifecycleError()}</p></Show>
        <Show when={installTarget()} keyed>
          {(target) => <InstallPluginDialog kind={target.kind} path={target.path} onInstall={install} onCancel={() => setInstallTarget(null)} />}
        </Show>
        <Show when={creating()}>
          <NewConversationDialog
            pickProjectDirectory={() => store.pickProjectDirectory()}
            inspectProject={(path) => store.inspectProject(path)}
            onCancel={() => {
              setCreating(false);
              queueMicrotask(() => newConversationTrigger?.focus());
            }}
            onCreate={(request) => store.createConversation(request)}
            onCreated={() => {
              setCreating(false);
              focusCreatedComposer();
            }}
          />
        </Show>
        <ConfirmDialog
          open={archiveTarget() !== null && archiveTarget() === selectedId()}
          busy={archiving()}
          title={`Archive ${selected()?.title ?? "conversation"}`}
          cancelLabel="Keep conversation"
          confirmLabel="Confirm archive"
          onConfirm={() => void confirmArchive()}
          onCancel={() => setArchiveTarget(null)}
        >
          <p>This removes the conversation from All work. Its durable history is retained.</p>
          <Show when={lifecycleError()}><p role="alert" class="inline-error">{lifecycleError()}</p></Show>
        </ConfirmDialog>
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
                { id: "command:new", label: "New conversation" },
                { id: "command:next-waiting", label: "Next item waiting on you (⌘J)", disabled: waiting().length === 0 },
                { id: "command:parent", label: "Go to parent conversation", disabled: !parent() },
                { id: "command:sidebar", label: sidebarOpen() ? "Hide All work" : "Show All work" },
                { id: "command:inspector", label: inspectorOpen() ? "Hide inspector" : "Show inspector" },
                {
                  id: "command:message",
                  label: "Focus message",
                  disabled: !selected()?.capabilities.canSend || snapshot().submissionsById[selectedId() ?? ""]?.pending === true,
                },
              ],
            },
          ]}
          onClose={() => setPaletteOpen(false)}
          onSelect={(id) => {
            if (id.startsWith("conversation:")) {
              const conversationId = id.slice("conversation:".length);
              selectConversation(conversationId);
              if (snapshot().conversationsById[conversationId]?.capabilities.canSend) setMessageFocusRequested(true);
            } else if (id === "command:new") openNewConversation();
            else if (id === "command:message") setMessageFocusRequested(true);
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
