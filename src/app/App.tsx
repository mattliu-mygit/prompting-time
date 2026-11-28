import { createEffect, createMemo, createSignal, Match, onSettled, Show, Switch, untrack } from "solid-js";
import { CloseIcon, CommandPalette, ConfirmDialog, EllipsisIcon, FilterIcon, IconButton, Menu, PanelRightIcon, PlusIcon, SearchIcon, StatusDot, type Status } from "@prompting-time/ui";
import type { MenuBarCommand, PluginSource, ProviderId } from "../bridge/types";
import { ACTIONS, messageOf, runPluginAction } from "../features/agents/actions";
import { AgentScreen } from "../features/agents/AgentScreen";
import { agentMenuItems, itemsInProgress, type AgentAction, type AgentTab } from "../features/agents/agentMenu";
import { INSTALL_ITEMS, PluginsSection, type InstallKind } from "../features/agents/PluginsSection";
import { InstallPluginDialog } from "../features/agents/InstallPluginDialog";
import { menuBarRequest, readMenuBarEnabled, saveMenuBarEnabled } from "../features/agents/menuBar";
import { PluginInspector } from "../features/agents/PluginSettings";
import { describeAgent } from "../features/agents/status";
import { blockerKey, itemKey, NO_STATUS_V2_WORK, readStatusV2, statusV2Work, type StatusV2Work } from "../features/agents/statusV2";
import { ConfirmActionDialog } from "../features/agents/ConfirmActionDialog";
import { BulkConfirmDialog, type BulkOutcome } from "../features/agents/BulkConfirmDialog";
import { ConversationNavigation } from "../features/conversations/ConversationNavigation";
import { PluginPage } from "../features/plugins/PluginPage";
import { Inspector } from "../features/inspector/Inspector";
import { Composer } from "../features/timeline/Composer";
import { Timeline, type TimelineViewState } from "../features/timeline/Timeline";
import { AllWork, conversationState, searchWork, waitingRoots } from "../features/work/AllWork";
import { bulkSummary, inFilter, isHidden, keepHidden, narrowWork, needsYouEntries, readFilter, readHidden, saveFilter, saveHidden, sharedActions, type HiddenEntry, type NeedsYouEntry, type NeedsYouFilter } from "../features/work/needsYou";
import { BulkBar } from "../features/work/BulkBar";
import { NewConversationDialog } from "./NewConversationDialog";
import { readSidebarWidth, SidebarResizer } from "./SidebarResizer";
import { createSnapshot, selectVisibleConversations, type AppSnapshot, type AppStore, type StatusFilter } from "./store";

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
const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
const NARROW_INSPECTOR = "(max-width: 72rem)";

/** A short note at the bottom of the window about what a row action did, an alert when it failed, with Undo when it can be undone. */
type Notice = { text: string; alert?: boolean; undo?: () => void };
const NOTICE_MS = 8000;

/**
 * What the workspace shows of a plugin instead of a conversation: its screen on a tab, or its page
 * at the route of the status format 2 item or blocker the user opened, or at `/` from the plugin's
 * name. `entry` is that item's or blocker's key, which the list shows as selected, and null at `/`.
 */
type PluginView =
  | { kind: "agent"; plugin: string; tab: AgentTab }
  | { kind: "page"; plugin: string; entry: string | null; target: { route: string } };
/** A row action of a format-2 plugin that waits for the user's confirmation. */
type EntryConfirmation = { plugin: string; action: string; args: string[] };
/** A bulk row action marked confirm, while its one confirmation for all the checked items is open; `done` hears how it ended. */
type BulkConfirmation = { plugin: string; action: string; items: { title: string; args: string[] }[]; done: (outcome: BulkOutcome) => void };

/**
 * The command center: the All work sidebar (Needs you, the chats, and the Plugins section), the
 * selected conversation's timeline and composer, or a plugin's page or screen with the sidebar
 * folded to a rail, and the inspector, with the dialogs that create and archive conversations,
 * install plugins, and stop one.
 */
export function App(props: { store: AppStore }) {
  // One store for the app's lifetime. Read once: a prop written as a call, such as
  // store={createAppStore(api)}, would make a new store on every read.
  const store = untrack(() => props.store);
  const snapshot = createSnapshot(store);
  const timelineViews = new Map<string, TimelineViewState>();
  // The sidebar folds to its rail in two ways: the user folds it, which lasts until they unfold it,
  // or a plugin shows, which lasts until the user unfolds it or chooses a chat.
  const [sidebarOpen, setSidebarOpen] = createSignal(true);
  const [railed, setRailed] = createSignal(false);
  const folded = () => !sidebarOpen() || railed();
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
  // A format-2 row action marked confirm, while its confirmation is open.
  const [entryConfirmation, setEntryConfirmation] = createSignal<EntryConfirmation | null>(null);
  const [bulkConfirmation, setBulkConfirmation] = createSignal<BulkConfirmation | null>(null);
  // What a row action did, until it times out or is dismissed.
  const [notice, setNotice] = createSignal<Notice | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  // What the sidebar's search holds; its results replace the sections while it has words.
  const [query, setQuery] = createSignal("");
  // Whether the background agents show in the macOS menu bar, which the palette turns off and on.
  const [menuBarEnabled, setMenuBarEnabled] = createSignal(readMenuBarEnabled());
  let inspectorTrigger: HTMLButtonElement | undefined;
  let newConversationTrigger: HTMLButtonElement | undefined;
  let inspectorClose: HTMLButtonElement | undefined;
  let rail: HTMLButtonElement | undefined;
  let foldTrigger: HTMLButtonElement | undefined;

  const selected = createMemo(() => {
    const id = snapshot().selectedConversationId;
    return id ? snapshot().conversationsById[id] ?? null : null;
  });
  const selectedId = createMemo(() => selected()?.id ?? null);
  const selectedPlugin = createMemo(() => snapshot().plugins.items.find(({ name }) => name === pluginView()?.plugin) ?? null);
  /** The words and dot of the selected agent's row, which the header repeats on its screen. */
  const selectedAgentState = () => {
    const plugin = selectedPlugin();
    return plugin ? describeAgent(plugin, snapshot().plugins.statuses[plugin.name]) : null;
  };
  /** What plugins that print status format 2 add to All work: blockers, and items in their declared groups. */
  const pluginWork = createMemo(() => statusV2Work(snapshot().plugins));
  /** The Needs you entries this device hides, until each changes or leaves Needs you. */
  const [hidden, setHidden] = createSignal<readonly HiddenEntry[]>(readHidden());
  /** Which entries the sidebar's Needs you lists, as this device chose. */
  const [chosenFilter, setChosenFilter] = createSignal<NeedsYouFilter>(readFilter());
  /** The chosen filter, or all when it names a plugin that no longer prints status format 2. */
  const filter = (): NeedsYouFilter => {
    const chosen = chosenFilter();
    return chosen.startsWith("plugin:") && !snapshot().plugins.items.some(({ name, status }) => `plugin:${name}` === chosen && status?.format === 2) ? "all" : chosen;
  };
  /** Every root waiting on you, whatever the sidebar's status filter shows. */
  const waitingChats = () => waitingRoots(snapshot().conversationIds.flatMap((id) => snapshot().conversationsById[id] ?? []));
  /** Everything under Needs you in Command-J's order, hidden entries included. */
  const needsYou = createMemo(() => needsYouEntries(pluginWork(), waitingChats()));
  /** The keys of the Needs you entries checked for a bulk action; All work drops those it no longer lists. */
  const [checked, setChecked] = createSignal<readonly string[]>([]);
  /** The checked entries, in Command-J's order. */
  const checkedEntries = createMemo(() => needsYou().filter(({ key }) => checked().includes(key)));
  /** The row actions every checked entry offers, with the manifest's titles, for the bulk bar. */
  const bulkActions = () => {
    const shared = sharedActions(checkedEntries());
    const plugin = shared && pluginNamed(shared.plugin);
    return plugin ? shared.actions.flatMap((name) => plugin.actions.filter((item) => item.name === name).map(({ title }) => ({ name, title }))) : [];
  };
  /** What the bulk action running now says, such as "Rerun · 2 of 3", or null. */
  const [bulkProgress, setBulkProgress] = createSignal<string | null>(null);
  /** What needs you less what the user hid: what the menu bar counts and auto-advance watches. */
  const unhidden = createMemo(() => needsYou().filter(({ key, version }) => !isHidden(hidden(), key, version)));
  const openNeedsYou = (entry: NeedsYouEntry) => (entry.plugin === null ? selectConversation(entry.key) : openEntry(entry.plugin, entry.key, entry.route!));
  /** The format-2 items under Needs you that the user didn't hide, in list order: the menu bar counts them and opens the first. */
  const needsYouItems = () => unhidden().filter(({ kind }) => kind === "item");
  // The menu bar extra shows what the Plugins section and Needs you show, and changes with them.
  createEffect(() => menuBarRequest(snapshot().plugins, agentPending(), needsYouItems().length, menuBarEnabled()), (request) => store.updateMenuBar(request));
  /** The key of what the list shows as selected: the item or blocker the page shows, or the conversation when no plugin shows. */
  const currentKey = () => {
    const view = pluginView();
    if (view?.kind === "page") return view.entry;
    return view ? null : selectedId();
  };
  /** The folders of the root chats, in list order (most recent first), each once, at most five: New chat's first places. */
  const recentFolders = () => [...new Set(snapshot().conversationIds.flatMap((id) => snapshot().conversationsById[id]?.projectRoot ?? []))].slice(0, 5);
  const parent = () => {
    const parentId = selected()?.parentId;
    return parentId ? snapshot().conversationsById[parentId] : undefined;
  };
  const overlay = () => narrow() && inspectorOpen();
  const lifecycleModalOpen = () => creating() || archiveTarget() !== null || installTarget() !== null || entryConfirmation() !== null || bulkConfirmation() !== null;
  const paletteAvailable = () => snapshot().phase === "ready" && !overlay() && !lifecycleModalOpen() && !composerModalOpen();
  const workspaceLabel = () => (pluginView() && selectedPlugin() ? "Plugin" : "Conversation workspace");
  /** What Needs you lists, in list order: format-2 blockers and items, then the conversations waiting on you; none hidden, only what the filter lists. Command-J steps through it. */
  const waiting = () => unhidden().filter(({ plugin }) => inFilter(filter(), plugin));
  /** Every format-2 blocker and item, in list order, as the palette's Work group finds and opens them. */
  const paletteWork = () => [
    ...pluginWork().blockers.map(({ plugin, pluginTitle, blocker }) => ({
      key: blockerKey(plugin, blocker.id), label: blocker.title, detail: `${pluginTitle} · blocks every job`, keywords: [pluginTitle],
      open: () => openEntry(plugin, blockerKey(plugin, blocker.id), blocker.route),
    })),
    ...[...pluginWork().needsYou, ...pluginWork().groups].flatMap(({ plugin, label, items }) => items.map((item) => ({
      key: itemKey(plugin, item.id), label: item.title, detail: [label, item.detail].filter(Boolean).join(" · "), keywords: item.project ? [item.project] : [],
      open: () => openEntry(plugin, itemKey(plugin, item.id), item.route),
    }))),
  ];
  /** The format-2 work the sidebar lists under Needs you: for all statuses and for waiting, none for the rest. */
  const sidebarPluginWork = (): StatusV2Work => (snapshot().statusFilter === "all" || snapshot().statusFilter === "waiting" ? pluginWork() : NO_STATUS_V2_WORK);
  /** The rail's words: "All work", and how many chats wait on you when any do. */
  const railLabel = () => {
    const count = unhidden().filter(({ kind }) => kind === "chat").length;
    return count > 0 ? `All work · ${count} waiting on you` : "All work";
  };
  const filterLabel = () => `Filter conversations: ${statusOptions.find((option) => option.value === snapshot().statusFilter)?.label}`;

  /** « and ⌘K fold the sidebar to its rail: while a plugin shows, until a chat is chosen, as opening the plugin did; otherwise until the user unfolds it. */
  const fold = () => (pluginView() ? setRailed(true) : setSidebarOpen(false));
  /** The rail and ⌘K unfold the sidebar. */
  function unfold() {
    setRailed(false);
    setSidebarOpen(true);
  }

  function selectConversation(id: string) {
    setPluginView(null);
    setRailed(false);
    void store.selectConversation(id);
    if (window.matchMedia?.("(max-width: 46rem)").matches) setSidebarOpen(false);
  }

  /** Shows a plugin in the workspace and folds the sidebar to its rail. */
  function showPlugin(view: PluginView) {
    setPluginView(view);
    setRailed(true);
    if (window.matchMedia?.("(max-width: 46rem)").matches) setSidebarOpen(false);
  }

  const openPlugin = (name: string, tab: AgentTab = "logs") => showPlugin({ kind: "agent", plugin: name, tab });
  /** A format-2 item or blocker: its plugin's page, which shows its route. */
  const openEntry = (plugin: string, key: string, route: string) => showPlugin({ kind: "page", plugin, entry: key, target: { route } });
  /** The plugin's row in the sidebar, its name in the menu bar, or Open page in the header's ⋯ menu: its page at its home route. */
  const openHome = (plugin: string) => showPlugin({ kind: "page", plugin, entry: null, target: { route: "/" } });

  // When the item or blocker the page shows leaves Needs you, as after the user acts on it there or
  // hides it, open the next thing Needs you lists: the first after it in Command-J's order that is
  // still there, else the first there is. With nothing left, stay. A plugin whose status can't be
  // read lists nothing for the moment, which doesn't count as leaving, and choosing a filter isn't
  // leaving either.
  createEffect(unhidden, (now, before) => untrack(() => {
    const view = pluginView();
    if (view?.kind !== "page" || view.entry === null || !before || now.some(({ key }) => key === view.entry)) return;
    const index = before.findIndex(({ key }) => key === view.entry);
    const plugin = pluginNamed(view.plugin);
    if (index === -1 || !plugin || !readStatusV2(plugin, snapshot().plugins.statuses[plugin.name])) return;
    const listed = now.filter(({ plugin: source }) => inFilter(filter(), source));
    const nextKey = [...before.slice(index + 1), ...before.slice(0, index)].find(({ key }) => listed.some((entry) => entry.key === key))?.key;
    const next = listed.find(({ key }) => key === nextKey) ?? listed[0];
    if (next) openNeedsYou(next);
  }));

  // An entry the user hid stays hidden while it is in Needs you unchanged; once it changes or leaves,
  // this device forgets it, so it shows the next time it needs you. A chat Prompting Time hasn't
  // loaded, or a plugin whose status it can't read now, can't be checked, so its entries stay hidden.
  createEffect(() => keepHidden(hidden(), needsYou(), (entry) => {
    if (entry.plugin === null) return snapshot().conversationsById[entry.key] !== undefined;
    const plugin = pluginNamed(entry.plugin);
    return plugin !== null && readStatusV2(plugin, snapshot().plugins.statuses[plugin.name]) !== null;
  }), (kept) => {
    if (kept.length !== untrack(hidden).length) changeHidden(kept);
  });

  function changeHidden(next: readonly HiddenEntry[]) {
    setHidden(next);
    saveHidden(next);
  }

  /** Hide in a Needs you row's menu: the entry leaves Needs you, and a notice offers Undo. */
  function hide(entry: HiddenEntry, title: string) {
    const next = [...hidden().filter(({ key }) => key !== entry.key), entry];
    changeHidden(next);
    notify({ text: `Hidden ${title}`, undo: () => showAgain(entry.key) });
  }

  /** Hide in the bulk bar: the checked entries leave Needs you, and a notice offers Undo for all of them. */
  function hideChecked() {
    const entries = checkedEntries();
    const keys = new Set(entries.map(({ key }) => key));
    changeHidden([...hidden().filter(({ key }) => !keys.has(key)), ...entries.map(({ key, plugin, version }) => ({ key, plugin, version }))]);
    setChecked([]);
    notify({ text: `Hid ${entries.length}`, undo: () => changeHidden(hidden().filter(({ key }) => !keys.has(key))) });
  }

  /**
   * A row action in the bulk bar: it runs once for each checked item, one after another, with
   * `--item <id>`; an action marked confirm runs only after one confirmation that shows what each
   * item will send. A notice then says how many it ran on, failed on, and the user cancelled.
   */
  async function runChecked(action: string) {
    const entries = checkedEntries();
    const plugin = pluginNamed(sharedActions(entries)?.plugin ?? "");
    const declared = plugin?.actions.find((item) => item.name === action);
    if (!plugin || !declared || bulkProgress() !== null) return;
    let outcome: BulkOutcome = { ran: 0, failed: 0, cancelled: 0, failure: null };
    if (declared.confirm) {
      setBulkProgress(`${declared.title} · confirming ${entries.length}`);
      const items = entries.map(({ title, itemId }) => ({ title, args: ["--item", itemId!] }));
      outcome = await new Promise<BulkOutcome>((done) => setBulkConfirmation({ plugin: plugin.name, action, items, done }));
    } else {
      for (const [index, entry] of entries.entries()) {
        setBulkProgress(`${declared.title} · ${index + 1} of ${entries.length}`);
        try {
          await runPluginAction(store.pluginActions, plugin, action, ["--item", entry.itemId!]);
          outcome.ran += 1;
        } catch (reason) {
          outcome.failed += 1;
          outcome.failure ??= messageOf(reason);
        }
      }
    }
    setBulkProgress(null);
    setChecked([]);
    void store.refreshPlugins();
    notify({ text: `${bulkSummary(declared.title, outcome)}${outcome.failure ? `: ${outcome.failure}` : ""}`, alert: outcome.failed > 0 });
  }

  function showAgain(key: string) {
    changeHidden(hidden().filter((entry) => entry.key !== key));
  }

  function chooseFilter(next: NeedsYouFilter) {
    setChosenFilter(next);
    saveFilter(next);
  }

  /**
   * Runs one of a plugin's controls from its row's ⋯ menu, the header's, or the menu bar: Poll now, Pause, and Resume
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

  /** A choice in the menu bar extra: an agent's button runs as on its row, and the rest open what their sidebar rows open. */
  function runMenuBarCommand(command: MenuBarCommand) {
    if (command.kind === "agent") void runAgentAction(command.plugin, command.action);
    else if (command.kind === "openAgent") openHome(command.plugin);
    else {
      const first = needsYouItems()[0];
      if (first) openNeedsYou(first);
    }
  }

  /**
   * A choice in a format-2 row's ⋯ menu: an action marked confirm opens its confirmation, and the
   * rest run at once, with the plugin's words in a notice when it refuses.
   */
  async function runEntryAction(pluginName: string, action: string, args: string[], label: string) {
    const plugin = pluginNamed(pluginName);
    const declared = plugin?.actions.find((item) => item.name === action);
    if (!plugin || !declared) return;
    if (declared.confirm) {
      setEntryConfirmation({ plugin: pluginName, action, args });
      return;
    }
    try {
      await runPluginAction(store.pluginActions, plugin, action, args);
      void store.refreshPlugins();
      notify({ text: `${declared.title} · ${label}` });
    } catch (reason) {
      notify({ text: messageOf(reason), alert: true });
    }
  }

  function notify(next: Notice | null) {
    clearTimeout(noticeTimer);
    setNotice(next);
    if (next) noticeTimer = setTimeout(() => setNotice(null), NOTICE_MS);
  }

  const pluginNamed = (name: string) => snapshot().plugins.items.find((plugin) => plugin.name === name) ?? null;
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
    if (next.key !== currentKey()) openNeedsYou(next);
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
    const stopMenuBar = store.onMenuBarCommand(runMenuBarCommand);
    return () => {
      clearTimeout(noticeTimer);
      query?.removeEventListener("change", updateNarrow);
      window.removeEventListener("keydown", onKeyDown);
      stopMenuBar();
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

  /** Escape clears the search; Enter opens its first result: a blocker, then an item, then a chat. */
  function onSearchKeyDown(event: KeyboardEvent) {
    if (event.key === "Escape" && query() !== "") {
      event.preventDefault();
      setQuery("");
    } else if (event.key === "Enter" && query().trim() !== "") {
      event.preventDefault();
      const { blockers, groups, chats } = searchWork(narrowWork(sidebarPluginWork(), hidden(), filter()), inFilter(filter(), null) ? selectVisibleConversations(snapshot()) : [], query());
      const blocker = blockers[0];
      const group = groups[0];
      const item = group?.items[0];
      if (blocker) openEntry(blocker.plugin, blockerKey(blocker.plugin, blocker.blocker.id), blocker.blocker.route);
      else if (group && item) openEntry(group.plugin, itemKey(group.plugin, item.id), item.route);
      else if (chats[0]) selectConversation(chats[0].id);
    }
  }

  return (
    <Show when={snapshot().phase === "ready"} fallback={<StartupState store={store} snapshot={snapshot()} />}>
      <div class="app-shell" inert={composerModalOpen()}>
        <header class="app-toolbar" inert={overlay() || lifecycleModalOpen()}>
          <Show
            when={selectedPlugin()}
            fallback={
              <div class="shell-title">
                <h1 title={selected()?.title ?? "Prompting Time"}>{selected()?.title ?? "Prompting Time"}</h1>
                <Show when={selected()}>
                  {(conversation) => (
                    <span class="shell-meta">
                      {conversation().provider ? providerNames[conversation().provider!] : "No provider"} · <StatusWords words={conversationState(conversation()).words} status={conversationState(conversation()).status} />
                      {conversation().projectRoot ? ` · ${conversation().projectRoot!.split(/[\\/]/).filter(Boolean).at(-1)}` : ""}
                    </span>
                  )}
                </Show>
              </div>
            }
          >
            {(plugin) => (
              <div class="shell-title">
                <h1>{plugin().title}</h1>
                <span class="shell-meta"><Show when={selectedAgentState()}>{(state) => <StatusWords words={state().meta} status={state().status} />}</Show></span>
              </div>
            )}
          </Show>
          <div class="toolbar-actions">
            <IconButton label="Search (⌘K)" disabled={!paletteAvailable()} onClick={() => setPaletteOpen(true)}>
              <SearchIcon />
            </IconButton>
            <Show when={folded()}>{newConversationButton()}</Show>
            <IconButton
              ref={inspectorTrigger}
              label={inspectorOpen() ? "Hide inspector" : "Show inspector"}
              aria-expanded={inspectorOpen() ? "true" : "false"}
              aria-controls="inspector-pane"
              onClick={() => setInspectorOpen((open) => !open)}
            >
              <PanelRightIcon />
            </IconButton>
            <Show when={selectedPlugin()}>
              {(plugin) => (
                <Menu
                  label="More"
                  icon={<EllipsisIcon />}
                  items={[
                    ...(pluginView()?.kind === "agent" ? [{ value: "page", label: "Open page" }] : []),
                    ...agentMenuItems(plugin(), snapshot().plugins.statuses[plugin().name], agentPending()[plugin().name] ?? null),
                  ]}
                  onSelect={(value) => (value === "page" ? openHome(plugin().name) : void runAgentAction(plugin().name, value as AgentAction))}
                />
              )}
            </Show>
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
          {/* The sidebar folds to this rail; it stays mounted, hidden, so it unfolds as it was. */}
          <Show when={folded()}>
            <button
              type="button"
              ref={rail}
              class="sidebar-rail"
              aria-controls="work-pane"
              aria-expanded="false"
              inert={overlay()}
              onClick={() => {
                unfold();
                queueMicrotask(() => foldTrigger?.focus());
              }}
            >
              <span aria-hidden="true">»</span>
              <span class="sidebar-rail-label">{railLabel()}</span>
            </button>
          </Show>
          <aside id="work-pane" class="sidebar-pane" aria-label="All work" hidden={folded()} inert={overlay()} style={{ "--sidebar-width": `${sidebarWidth()}px` }}>
            <div class="sidebar-find">
              <IconButton
                ref={foldTrigger}
                class="sidebar-fold"
                label="Fold All work"
                aria-controls="work-pane"
                aria-expanded="true"
                onClick={() => {
                  fold();
                  queueMicrotask(() => rail?.focus());
                }}
              >
                <span aria-hidden="true">«</span>
              </IconButton>
              <label class="sidebar-search">
                <SearchIcon />
                <input
                  type="search"
                  class="pt-input"
                  placeholder="Search work and chats"
                  aria-label="Search work and chats"
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
              selectedEntry={pluginView() ? currentKey() : null}
              pluginWork={sidebarPluginWork()}
              actionTitle={(plugin, action) => pluginNamed(plugin)?.actions.find((item) => item.name === action)?.title ?? action}
              onOpenEntry={openEntry}
              onEntryAction={(plugin, action, args, label) => void runEntryAction(plugin, action, args, label)}
              hidden={hidden()}
              onHide={hide}
              onShowAgain={showAgain}
              checked={checked()}
              onCheck={setChecked}
              filter={filter()}
              onFilter={chooseFilter}
              pluginTitle={(plugin) => pluginNamed(plugin)?.title ?? plugin}
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
            <PluginsSection
              plugins={snapshot().plugins}
              selected={currentKey() === null ? selectedPlugin()?.name ?? null : null}
              pending={agentPending()}
              onSelect={openHome}
              onAction={(name, action) => void runAgentAction(name, action)}
              onInstall={(kind) => void startInstall(kind)}
            />
            <Show when={checkedEntries().length > 0 || bulkProgress() !== null}>
              <BulkBar
                count={checkedEntries().length}
                actions={bulkActions()}
                progress={bulkProgress()}
                onHide={hideChecked}
                onAction={(action) => void runChecked(action)}
                onClear={() => setChecked([])}
              />
            </Show>
            <SidebarResizer width={sidebarWidth()} onResize={setSidebarWidth} />
          </aside>

          <main class="workspace-pane" aria-label={workspaceLabel()} inert={overlay()}>
            <Show when={selectedPlugin()?.name} keyed fallback={conversationWorkspace()}>
              {(name: string) => (
                <Show when={snapshot().plugins.items.find((plugin) => plugin.name === name)}>
                  {(plugin) => (
                    <Switch>
                      <Match when={pluginView()?.kind === "page" && (pluginView() as { target: { route: string } }).target}>
                        {(target) => (
                          <PluginPage
                            plugin={plugin()}
                            target={target()}
                            actions={store.pluginActions}
                            onPluginFilesChange={(listener) => store.onPluginFilesChange(listener)}
                          />
                        )}
                      </Match>
                      <Match when={pluginView()?.kind === "agent" && (pluginView() as { tab: AgentTab }).tab}>
                        {(tab) => (
                          <AgentScreen
                            plugin={plugin()}
                            status={snapshot().plugins.statuses[name]}
                            tab={tab()}
                            onTab={(next) => setPluginView({ kind: "agent", plugin: name, tab: next })}
                            error={agentError()?.plugin === name ? agentError()!.text : null}
                            actions={store.pluginActions}
                            controls={store}
                            onPluginFilesChange={(listener) => store.onPluginFilesChange(listener)}
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
                    // An inspector that opens as an overlay takes the focus as it opens.
                    if (untrack(narrow)) queueMicrotask(() => inspectorClose?.focus());
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
              <Show when={current.undo}>
                {(undo) => (
                  <button type="button" class="pt-button" onClick={() => {
                    undo()();
                    notify(null);
                  }}>Undo</button>
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
                when={itemsInProgress(plugin(), snapshot().plugins.statuses[plugin().name]).length > 0}
                fallback={<p>Stopping ends {plugin().title}'s process now. Nothing it lists is in progress.</p>}
              >
                <p>Stopping ends {plugin().title}'s process now, and with it what it lists as in progress:</p>
                <ul class="stop-reviews">
                  {itemsInProgress(plugin(), snapshot().plugins.statuses[plugin().name]).map((item) => <li>{item.title}</li>)}
                </ul>
              </Show>
              <p>Prompting Time starts {plugin().title} again the next time it opens.</p>
            </ConfirmDialog>
          )}
        </Show>
        <Show when={installTarget()} keyed>
          {(target) => <InstallPluginDialog kind={target.kind} path={target.path} onInstall={install} onCancel={() => setInstallTarget(null)} />}
        </Show>
        <Show when={entryConfirmation()} keyed>
          {(confirmation: EntryConfirmation) => (
            <Show when={pluginNamed(confirmation.plugin)}>
              {(plugin) => (
                <ConfirmActionDialog
                  plugin={plugin()}
                  action={confirmation.action}
                  args={confirmation.args}
                  actions={store.pluginActions}
                  onClose={(outcome) => {
                    setEntryConfirmation(null);
                    if ("result" in outcome && outcome.stage === "run" && outcome.result.exitCode === 0) void store.refreshPlugins();
                  }}
                />
              )}
            </Show>
          )}
        </Show>
        <Show when={bulkConfirmation()} keyed>
          {(confirmation: BulkConfirmation) => (
            <Show when={pluginNamed(confirmation.plugin)}>
              {(plugin) => (
                <BulkConfirmDialog
                  plugin={plugin()}
                  action={confirmation.action}
                  items={confirmation.items}
                  actions={store.pluginActions}
                  onClose={(outcome) => {
                    setBulkConfirmation(null);
                    confirmation.done(outcome);
                  }}
                />
              )}
            </Show>
          )}
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
              // The new chat shows in place of a plugin, with the sidebar unfolded.
              setPluginView(null);
              setRailed(false);
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
          label="Search conversations, work, and commands"
          placeholder="Search conversations, work, and commands…"
          emptyText="No conversations, work, or commands found."
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
              heading: "Work",
              items: paletteWork().map((entry) => ({ id: entry.key, label: entry.label, detail: entry.detail, keywords: entry.keywords, current: currentKey() === entry.key })),
            },
            {
              heading: "Commands",
              items: [
                { id: "command:new", label: "New chat" },
                { id: "command:next-waiting", label: "Next item waiting on you (⌘J)", disabled: waiting().length === 0 },
                { id: "command:parent", label: "Go to parent conversation", disabled: !parent() },
                { id: "command:sidebar", label: folded() ? "Unfold All work" : "Fold All work" },
                ...INSTALL_ITEMS.map((item) => ({ id: `command:install-${item.value}`, label: `Add plugin: ${item.label}` })),
                { id: "command:menu-bar", label: menuBarEnabled() ? "Hide plugins from the menu bar" : "Show plugins in the menu bar" },
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
            const entry = paletteWork().find(({ key }) => key === id);
            if (entry) entry.open();
            else if (id.startsWith("conversation:")) {
              const conversationId = id.slice("conversation:".length);
              selectConversation(conversationId);
              if (snapshot().conversationsById[conversationId]?.capabilities.canSend) setMessageFocusRequested(true);
            } else if (id === "command:new") openNewConversation();
            else if (id === "command:message") setMessageFocusRequested(true);
            else if (id === "command:next-waiting") jumpToNextWaiting();
            else if (id === "command:parent") {
              const target = parent();
              if (target) selectConversation(target.id);
            } else if (id === "command:sidebar") (folded() ? unfold : fold)();
            else if (id === "command:inspector") setInspectorOpen((open) => !open);
            else if (id === "command:menu-bar") {
              // Solid applies the write on its next flush, so read the new value first.
              const enabled = !menuBarEnabled();
              setMenuBarEnabled(enabled);
              saveMenuBarEnabled(enabled);
            }
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

/** A status in the window's header: its words, after its dot when the status has one, as its row shows it. */
function StatusWords(props: { words: string; status: Status }) {
  return (
    <>
      <Show when={props.status !== "idle"}>
        <StatusDot status={props.status} />
      </Show>
      {props.words}
    </>
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
            <StatusDot status={provider.available ? "idle" : "error"} />
            <span>{provider.available && provider.version ? `${providerNames[provider.id]} ${provider.version}` : `${providerNames[provider.id]} unavailable: ${provider.diagnostic ?? "Not installed"}`}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
