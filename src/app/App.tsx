import { createMemo, createSignal, Match, onSettled, Show, Switch, untrack } from "solid-js";
import { Button, CloseIcon, CommandPalette, ConfirmDialog, EllipsisIcon, FilterIcon, IconButton, Menu, PanelLeftIcon, PanelRightIcon, PlusIcon, SearchIcon } from "@prompting-time/ui";
import type { PluginSource, ProviderId } from "../bridge/types";
import { ACTIONS, hasAction, messageOf, runPluginAction } from "../features/agents/actions";
import { AgentScreen } from "../features/agents/AgentScreen";
import { reviewsInProgress, type AgentAction, type AgentTab } from "../features/agents/agentMenu";
import { BackgroundAgents, INSTALL_ITEMS, type InstallKind } from "../features/agents/BackgroundAgents";
import { CauseScreen } from "../features/agents/CauseScreen";
import { InstallPluginDialog } from "../features/agents/InstallPluginDialog";
import { PluginInspector } from "../features/agents/PluginSettings";
import { blockingCauses, causeKey, needsYou, newestRuns, pluginReviews, reviewKey, reviewState, reviewTitle, shortPullRequest, versionsOf, waitingText, type PluginReview, type ReviewEntry } from "../features/agents/reviews";
import { ReviewScreen } from "../features/agents/ReviewScreen";
import { describeAgent, readAgentStatus, readBlocked } from "../features/agents/status";
import { ConversationNavigation } from "../features/conversations/ConversationNavigation";
import { Inspector } from "../features/inspector/Inspector";
import { Composer } from "../features/timeline/Composer";
import { Timeline, type TimelineViewState } from "../features/timeline/Timeline";
import { AllWork, searchWork, waitingRoots, type PluginCause } from "../features/work/AllWork";
import { NewConversationDialog } from "./NewConversationDialog";
import { readSidebarWidth, SidebarResizer } from "./SidebarResizer";
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

/** A short note at the bottom of the window about what an action did, with one follow-up, such as Undo; an alert when it failed. */
type Notice = { text: string; alert?: boolean; action?: { label: string; run(): void } };
/** How long a notice stays: Undo is meant for right after, and Restore on a removed review's page covers later. */
const NOTICE_MS = 8000;

/** What the workspace shows of a plugin instead of a conversation: its screen on a tab, one review, or one cause that blocks every review. */
type PluginView = { kind: "agent"; plugin: string; tab: AgentTab } | { kind: "review"; plugin: string; runId: string } | { kind: "cause"; plugin: string; cause: string };

/**
 * The command center: the All work list with its reviews and background agents, the selected
 * conversation's timeline and composer, or an agent's screen, a review, or a cause that blocks every
 * review, and the inspector, with the dialogs that create and archive conversations, install
 * agents, and stop an agent.
 */
export function App(props: { store: AppStore }) {
  // One store for the app's lifetime. Read once: a prop written as a call, such as
  // store={createAppStore(api)}, would make a new store on every read.
  const store = untrack(() => props.store);
  const snapshot = createSnapshot(store);
  const timelineViews = new Map<string, TimelineViewState>();
  const [sidebarOpen, setSidebarOpen] = createSignal(true);
  const [sidebarWidth, setSidebarWidth] = createSignal(readSidebarWidth());
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
  // The control that runs for each agent, the last one's failure, and the agent Stop asks about.
  const [agentPending, setAgentPending] = createSignal<Readonly<Record<string, AgentAction>>>({});
  const [agentError, setAgentError] = createSignal<{ plugin: string; text: string } | null>(null);
  const [stopTarget, setStopTarget] = createSignal<string | null>(null);
  // What a Submit, Skip, or Remove did and what opened after it, with Undo, until it times out or is dismissed.
  const [notice, setNotice] = createSignal<Notice | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  // What the sidebar's search holds; its results replace the sections while it has words.
  const [query, setQuery] = createSignal("");
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
  /** The newest run of each pull request of every plugin: the reviews All work lists. */
  const newestReviews = createMemo(() => Object.entries(reviewsByPlugin()).flatMap(([plugin, reviews]): PluginReview[] => newestRuns(reviews).map((review) => ({ plugin, review }))));
  const causes = createMemo(() => snapshot().plugins.items.flatMap((plugin): PluginCause[] => blockingCauses(reviewsByPlugin()[plugin.name] ?? [], readBlocked(readAgentStatus(snapshot().plugins.statuses[plugin.name]))).map((cause) => ({ plugin: plugin.name, cause }))));
  /** The reviews under "Needs you", in list order: ready to approve, needing a look, then failed for a reason of their own. */
  const needsYouReviews = createMemo(() => {
    const { ready, look, failed } = needsYou(newestReviews());
    return [...ready, ...look, ...failed];
  });
  const openReviewEntry = () => {
    const view = pluginView();
    return view?.kind === "review" ? reviewsByPlugin()[view.plugin]?.find((review) => review.run_id === view.runId) ?? null : null;
  };
  const openCause = () => {
    const view = pluginView();
    return view?.kind === "cause" ? causes().find((item) => item.plugin === view.plugin && item.cause.id === view.cause) ?? null : null;
  };
  /** The key of what the list shows as selected: the open review's pull request, the open cause, or the conversation when no plugin shows. */
  const currentKey = () => {
    const view = pluginView();
    if (view?.kind === "review") {
      const review = openReviewEntry();
      const newest = review ? versionsOf(reviewsByPlugin()[view.plugin] ?? [], review).at(-1) : null;
      return reviewKey(view.plugin, newest?.run_id ?? view.runId);
    }
    if (view?.kind === "cause") return causeKey(view.plugin, view.cause);
    return view ? null : selectedId();
  };
  /** The folders of the root chats, in list order (most recent first), each once, at most five: New chat's first places. */
  const recentFolders = () => [...new Set(snapshot().conversationIds.flatMap((id) => snapshot().conversationsById[id]?.projectRoot ?? []))].slice(0, 5);
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
    return { agent: "Background agent", review: "Review", cause: "Blocked reviews" }[view.kind];
  };
  /** Everything under "Needs you", in list order: its reviews, then the conversations waiting on you, each with what a notice names it. */
  const waiting = () => [
    ...needsYouReviews().map(({ plugin, review }) => ({ key: reviewKey(plugin, review.run_id), label: shortPullRequest(review), open: () => openReview(plugin, review.run_id) })),
    ...waitingRoots(snapshot().conversationIds.flatMap((id) => snapshot().conversationsById[id] ?? []))
      .map(({ id, title }) => ({ key: id, label: title, open: () => selectConversation(id) })),
  ];
  /** The reviews the sidebar lists under the status filter: every one for all statuses, those under Needs you for waiting, none for the rest. */
  const sidebarReviews = () => (snapshot().statusFilter === "all" ? newestReviews() : snapshot().statusFilter === "waiting" ? needsYouReviews() : []);
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

  const openPlugin = (name: string, tab: AgentTab = "runs") => showPlugin({ kind: "agent", plugin: name, tab });
  const openReview = (plugin: string, runId: string) => showPlugin({ kind: "review", plugin, runId });

  /**
   * Runs one of an agent's controls from its row menu or its screen: Poll now, Pause, and Resume
   * run its round, pause, and resume actions; Stop asks first; Start and a confirmed Stop go to
   * the plugin host. A failure shows the plugin's own words.
   */
  async function runAgentAction(name: string, action: AgentAction) {
    const plugin = snapshot().plugins.items.find((item) => item.name === name);
    if (!plugin) return;
    if (action === "settings" || action === "logs") return openPlugin(name, action);
    if (action === "stop" && stopTarget() !== name) return setStopTarget(name);
    setAgentError(null);
    setAgentPending({ ...agentPending(), [name]: action });
    try {
      if (action === "stop") await store.stopPlugin(name);
      else if (action === "start") await store.startPlugin(name);
      else {
        await runPluginAction(store.pluginActions, plugin, action === "poll" ? ACTIONS.round : action === "pause" ? ACTIONS.pause : ACTIONS.resume);
        void store.refreshPlugins();
      }
    } catch (reason) {
      setAgentError({ plugin: name, text: messageOf(reason) });
    } finally {
      setAgentPending(Object.fromEntries(Object.entries(agentPending()).filter(([key]) => key !== name)));
      if (action === "stop") setStopTarget(null);
    }
  }

  function notify(next: Notice | null) {
    clearTimeout(noticeTimer);
    setNotice(next);
    if (next) noticeTimer = setTimeout(() => setNotice(null), NOTICE_MS);
  }

  const pluginNamed = (name: string) => snapshot().plugins.items.find((plugin) => plugin.name === name) ?? null;
  const reviewOf = (plugin: string, runId: string) => reviewsByPlugin()[plugin]?.find((review) => review.run_id === runId) ?? null;
  /** Runs one of a plugin's actions on its reviews; a refusal throws the plugin's words. */
  async function reviewAction(plugin: string, name: string, args: string[]) {
    const found = pluginNamed(plugin);
    if (!found) throw new Error("The plugin is no longer installed.");
    await runPluginAction(store.pluginActions, found, name, args);
  }

  /**
   * After the user submitted, skipped, or removed a review: when it was open, the item after it in
   * Needs you opens, as the list stood before (the first item when the review was not in it), and
   * the notice names both; otherwise the notice says what happened. Undo runs `undo` and opens the
   * review again when it was open. A refusal of `run` throws to the caller, which shows it.
   */
  async function decide(plugin: string, review: ReviewEntry, verb: "Submitted" | "Skipped" | "Removed", run: () => Promise<void>, undo?: () => Promise<void>) {
    const key = reviewKey(plugin, review.run_id);
    const items = waiting();
    const wasOpen = currentKey() === key;
    await run();
    void store.refreshPlugins();
    const name = shortPullRequest(review);
    const action = undo && {
      label: "Undo",
      run: () => void undo().then(() => {
        void store.refreshPlugins();
        if (wasOpen) openReview(plugin, review.run_id);
      }, (reason: unknown) => notify({ text: messageOf(reason), alert: true })),
    };
    if (!wasOpen) return notify({ text: `${verb} ${name}`, action });
    const index = items.findIndex((item) => item.key === key);
    const next = index < 0 ? items[0] : items[index + 1] ?? items.find((item) => item.key !== key);
    next?.open();
    notify({ text: next ? `${verb} ${name} · now showing ${next.label}` : `${verb} ${name} · nothing else needs you`, action });
  }

  /** The review actions a plugin's screens and rows ask for, for plugin `plugin`. */
  const triage = (plugin: string) => ({
    async skip(runId: string) {
      const review = reviewOf(plugin, runId);
      if (review) await decide(plugin, review, "Skipped", () => reviewAction(plugin, ACTIONS.skip, [runId]), () => reviewAction(plugin, ACTIONS.skip, [runId, "--undo"]));
    },
    async remove(runId: string) {
      const review = reviewOf(plugin, runId);
      if (review) await decide(plugin, review, "Removed", () => reviewAction(plugin, ACTIONS.remove, [runId]), () => reviewAction(plugin, ACTIONS.remove, [runId, "--undo"]));
    },
    async restore(runId: string) {
      const review = reviewOf(plugin, runId);
      await reviewAction(plugin, ACTIONS.remove, [runId, "--undo"]);
      void store.refreshPlugins();
      notify({ text: `Restored ${review ? shortPullRequest(review) : "the review"} to your lists.` });
    },
  });

  /**
   * Remove all on a cause's screen: removes each review one after another, then names how many with
   * Undo, and what the plugin refused. It throws only when the plugin refused every one.
   */
  async function removeAll(plugin: string, runIds: readonly string[]) {
    if (runIds.length === 1) return triage(plugin).remove(runIds[0]!);
    const removed: string[] = [];
    const refused: string[] = [];
    for (const runId of runIds) {
      try {
        await reviewAction(plugin, ACTIONS.remove, [runId]);
        removed.push(runId);
      } catch (reason) {
        const review = reviewOf(plugin, runId);
        refused.push(`${review ? shortPullRequest(review) : runId}: ${messageOf(reason)}`);
      }
    }
    void store.refreshPlugins();
    if (removed.length === 0) throw new Error(refused.join("\n"));
    const title = pluginNamed(plugin)?.title ?? plugin;
    notify({
      text: `Removed ${removed.length} reviews. Their files stay on disk.${refused.length > 0 ? ` ${title} refused ${refused.length}: ${refused.join("; ")}` : ""}`,
      action: {
        label: "Undo",
        run: () => void Promise.all(removed.map((runId) => reviewAction(plugin, ACTIONS.remove, [runId, "--undo"])))
          .then(() => void store.refreshPlugins(), (reason: unknown) => notify({ text: messageOf(reason), alert: true })),
      },
    });
  }

  /** Remove on a review's row in the sidebar; a refusal shows in the notice. */
  function removeFromRow(plugin: string, runId: string) {
    triage(plugin).remove(runId).catch((reason: unknown) => notify({ text: messageOf(reason), alert: true }));
  }

  /** The plugin's page posted the user's review: the next item in Needs you opens. */
  function submitted(plugin: string, runId: string) {
    const review = reviewOf(plugin, runId);
    if (review) void decide(plugin, review, "Submitted", async () => {});
  }

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
      clearTimeout(noticeTimer);
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

  const newConversationButton = () => (
    <IconButton ref={newConversationTrigger} label="New chat" onClick={openNewConversation}>
      <PlusIcon />
    </IconButton>
  );

  /** Escape clears the search; Enter opens its first result, a review before a chat. */
  function onSearchKeyDown(event: KeyboardEvent) {
    if (event.key === "Escape" && query() !== "") {
      event.preventDefault();
      setQuery("");
    } else if (event.key === "Enter" && query().trim() !== "") {
      event.preventDefault();
      const { groups, chats } = searchWork(sidebarReviews(), selectVisibleConversations(snapshot()), query());
      const first = groups[0]?.reviews[0];
      if (first) openReview(first.plugin, first.review.run_id);
      else if (chats[0]) selectConversation(chats[0].id);
    }
  }

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
                  <Show
                    when={pluginView()?.kind === "cause"}
                    fallback={
                      <div class="shell-title">
                        <h1>{plugin().title}</h1>
                        <span class="shell-meta">Background agent · {describeAgent(plugin(), snapshot().plugins.statuses[plugin().name]).meta}</span>
                      </div>
                    }
                  >
                    <div class="shell-title">
                      <h1>{openCause()?.cause.summary ?? "Blocked reviews"}</h1>
                      <span class="shell-meta">{plugin().title} · {waitingText(openCause()?.cause.reviews.length ?? 0)}</span>
                    </div>
                  </Show>
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
            <IconButton label="Search (⌘K)" disabled={!paletteAvailable()} onClick={() => setPaletteOpen(true)}>
              <SearchIcon />
            </IconButton>
            <Show when={!sidebarOpen()}>{newConversationButton()}</Show>
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
            <aside id="work-pane" class="sidebar-pane" aria-label="All work" inert={overlay()} style={{ "--sidebar-width": `${sidebarWidth()}px` }}>
              <BackgroundAgents
                plugins={snapshot().plugins}
                selected={pluginView()?.kind === "agent" ? selectedPlugin()?.name ?? null : null}
                selectedCause={pluginView()?.kind === "cause" ? currentKey() : null}
                pending={agentPending()}
                onSelect={(name) => openPlugin(name)}
                onOpenCause={(plugin, cause) => showPlugin({ kind: "cause", plugin, cause })}
                onAction={(name, action) => void runAgentAction(name, action)}
                onInstall={(kind) => void startInstall(kind)}
              />
              <div class="sidebar-find">
                <label class="sidebar-search">
                  <SearchIcon />
                  <input
                    type="search"
                    class="pt-input"
                    placeholder="Search PRs and chats"
                    aria-label="Search PRs and chats"
                    value={query()}
                    onInput={(event) => setQuery(event.currentTarget.value)}
                    onKeyDown={onSearchKeyDown}
                  />
                </label>
                {newConversationButton()}
                <Menu
                  label={filterLabel()}
                  icon={<FilterIcon />}
                  items={statusOptions.map((option) => ({ ...option, type: "radio" as const, checked: option.value === snapshot().statusFilter }))}
                  onSelect={(value) => store.setStatusFilter(value as StatusFilter)}
                />
              </div>
              <Show when={snapshot().queuedCount > 0}><p class="queue-count">{snapshot().queuedCount} queued</p></Show>
              <AllWork
                query={query()}
                reviews={sidebarReviews()}
                causes={snapshot().statusFilter === "all" ? causes() : []}
                selectedReview={pluginView()?.kind === "review" || pluginView()?.kind === "cause" ? currentKey() : null}
                onOpenReview={openReview}
                onOpenCause={(plugin, cause) => showPlugin({ kind: "cause", plugin, cause })}
                canRemove={(plugin) => {
                  const found = pluginNamed(plugin);
                  return found !== null && hasAction(found, ACTIONS.remove);
                }}
                onRemoveReview={removeFromRow}
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
              <SidebarResizer width={sidebarWidth()} onResize={setSidebarWidth} />
            </aside>
          </Show>

          <main class="workspace-pane" aria-label={workspaceLabel()} inert={overlay()}>
            <Show when={selectedPlugin()?.name} keyed fallback={conversationWorkspace()}>
              {(name: string) => (
                <Show when={snapshot().plugins.items.find((plugin) => plugin.name === name)}>
                  {(plugin) => (
                    <Switch>
                      <Match when={pluginView()?.kind === "review" && (pluginView() as { runId: string }).runId}>
                        {(runId) => (
                          <ReviewScreen
                            plugin={plugin()}
                            status={snapshot().plugins.statuses[name]}
                            runId={runId()}
                            actions={store.pluginActions}
                            controls={store}
                            onPluginFilesChange={(listener) => store.onPluginFilesChange(listener)}
                            onOpenReview={(next) => setPluginView({ kind: "review", plugin: name, runId: next })}
                            onOpenCause={(cause) => showPlugin({ kind: "cause", plugin: name, cause })}
                            triage={triage(name)}
                            onSubmitted={(runId) => submitted(name, runId)}
                          />
                        )}
                      </Match>
                      <Match when={pluginView()?.kind === "cause" && (pluginView() as { cause: string }).cause}>
                        {(cause) => (
                          <Show when={cause()} keyed>
                            {(id: string) => (
                              <CauseScreen
                                plugin={plugin()}
                                status={snapshot().plugins.statuses[name]}
                                cause={id}
                                actions={store.pluginActions}
                                onOpenReview={(runId) => openReview(name, runId)}
                                onOpenLogs={() => openPlugin(name, "logs")}
                                onRemove={(runIds) => removeAll(name, runIds)}
                              />
                            )}
                          </Show>
                        )}
                      </Match>
                      <Match when={pluginView()?.kind === "agent" && (pluginView() as { tab: AgentTab }).tab}>
                        {(tab) => (
                          <AgentScreen
                            plugin={plugin()}
                            status={snapshot().plugins.statuses[name]}
                            tab={tab()}
                            onTab={(next) => setPluginView({ kind: "agent", plugin: name, tab: next })}
                            pending={agentPending()[name] ?? null}
                            error={agentError()?.plugin === name ? agentError()!.text : null}
                            onAction={(action) => void runAgentAction(name, action)}
                            actions={store.pluginActions}
                            controls={store}
                            onPluginFilesChange={(listener) => store.onPluginFilesChange(listener)}
                            onOpenReview={(runId) => openReview(name, runId)}
                          />
                        )}
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
        <Show when={notice()} keyed>
          {(current: Notice) => (
            <div class={["app-notice", { "app-notice-alert": current.alert }]} role={current.alert ? "alert" : "status"}>
              <span>{current.text}</span>
              <Show when={current.action}>
                {(action) => (
                  <Button
                    onClick={() => {
                      notify(null);
                      action().run();
                    }}
                  >
                    {action().label}
                  </Button>
                )}
              </Show>
              <IconButton label="Dismiss" onClick={() => notify(null)}><CloseIcon /></IconButton>
            </div>
          )}
        </Show>
        <Show when={snapshot().error}><p role="alert" class="inline-error lifecycle-error">{snapshot().error}</p></Show>
        <Show when={lifecycleError() && !creating() && !archiveTarget()}><p role="alert" class="inline-error lifecycle-error">{lifecycleError()}</p></Show>
        {/* A control run from an agent's row menu fails here; its screen shows the failure itself. */}
        <Show when={agentError() && !(pluginView()?.kind === "agent" && pluginView()?.plugin === agentError()!.plugin)}>
          <p role="alert" class="inline-error lifecycle-error">{agentError()!.text}</p>
        </Show>
        <Show when={snapshot().plugins.items.find((plugin) => plugin.name === stopTarget())}>
          {(plugin) => (
            <ConfirmDialog
              open
              busy={agentPending()[plugin().name] === "stop"}
              title={`Stop ${plugin().title}?`}
              cancelLabel="Keep running"
              confirmLabel={`Stop ${plugin().title}`}
              onConfirm={() => void runAgentAction(plugin().name, "stop")}
              onCancel={() => setStopTarget(null)}
            >
              <Show
                when={reviewsInProgress(snapshot().plugins.statuses[plugin().name]).length > 0}
                fallback={<p>Stopping ends {plugin().title}'s process now. No review is in progress.</p>}
              >
                <p>Stopping ends {plugin().title}'s process now, and with it the {reviewsInProgress(snapshot().plugins.statuses[plugin().name]).length === 1 ? "review" : "reviews"} in progress:</p>
                <ul class="stop-reviews">
                  {reviewsInProgress(snapshot().plugins.statuses[plugin().name]).map((review) => <li>{shortPullRequest(review)} · {reviewTitle(review)}</li>)}
                </ul>
              </Show>
              <p>Pause instead lets reviews in progress finish and skips polls until you resume. Prompting Time starts {plugin().title} again the next time it opens.</p>
            </ConfirmDialog>
          )}
        </Show>
        <Show when={installTarget()} keyed>
          {(target) => <InstallPluginDialog kind={target.kind} path={target.path} onInstall={install} onCancel={() => setInstallTarget(null)} />}
        </Show>
        <Show when={creating()}>
          <NewConversationDialog
            recentFolders={recentFolders()}
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
          label="Search conversations, reviews, and commands"
          placeholder="Search conversations, reviews, and commands…"
          emptyText="No conversations, reviews, or commands found."
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
              heading: "Reviews",
              items: newestReviews().map(({ plugin, review }) => ({
                id: `review:${plugin}:${review.run_id}`,
                label: reviewTitle(review),
                detail: `${shortPullRequest(review)} · ${reviewState(review).words}`,
                keywords: [review.repo, `#${review.pr}`, shortPullRequest(review), ...(review.author ? [`@${review.author}`] : [])],
                current: currentKey() === reviewKey(plugin, review.run_id),
              })),
            },
            {
              heading: "Commands",
              items: [
                { id: "command:new", label: "New chat" },
                { id: "command:next-waiting", label: "Next item waiting on you (⌘J)", disabled: waiting().length === 0 },
                { id: "command:parent", label: "Go to parent conversation", disabled: !parent() },
                { id: "command:sidebar", label: sidebarOpen() ? "Hide All work" : "Show All work" },
                ...INSTALL_ITEMS.map((item) => ({ id: `command:install-${item.value}`, label: `Add background agent: ${item.label}` })),
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
            if (id.startsWith("review:")) {
              const [, plugin = "", ...run] = id.split(":");
              openReview(plugin, run.join(":"));
            } else if (id.startsWith("conversation:")) {
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
            else if (id.startsWith("command:install-")) void startInstall(id.slice("command:install-".length) as InstallKind);
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
