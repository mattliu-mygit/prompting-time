import { createEffect, createMemo, createSignal, createUniqueId, For, Show, untrack } from "solid-js";
import type { JSX } from "@solidjs/web";
import { AlertIcon, BotIcon, ChatIcon, ChevronDownIcon, ChevronRightIcon, EllipsisIcon, ListRow, Menu, type Status } from "@prompting-time/ui";
import type { ConversationSummary, ProviderId } from "../../bridge/types";
import { effectiveConversationStatus, type ChildPageSnapshot, type EffectiveStatus, type SelectedPath } from "../../app/store";
import { readPreference, savePreference } from "../../app/preferences";
import { blockerKey, itemKey, type PluginBlocker, type PluginGroup, type StatusV2Blocker, type StatusV2Item, type StatusV2Work } from "../agents/statusV2";
import { blockerVersion, inFilter, isHidden, itemVersion, narrowWork, needsYouEntries, type HiddenEntry, type NeedsYouFilter } from "./needsYou";

type Props = {
  /** Active roots in list order, already filtered by status. */
  conversations: readonly ConversationSummary[];
  conversationsById: Readonly<Record<string, ConversationSummary>>;
  childPagesById: Readonly<Record<string, ChildPageSnapshot>>;
  expandedById: Readonly<Record<string, boolean>>;
  selectedId: string | null;
  selectedPath: SelectedPath | null;
  /** The key (itemKey or blockerKey) of the item or blocker the workspace shows, if any. */
  selectedEntry: string | null;
  /** What the sidebar's search holds; while it has words, searchWork's results replace the sections. */
  query: string;
  onSelect(id: string): void;
  /**
   * What plugins that print status format 2 add: their blockers and the items of their Needs you
   * groups, under Needs you. Their other groups live in their pages, so the sidebar leaves them out.
   */
  pluginWork: StatusV2Work;
  /** The title the plugin's manifest gives an action, for a row's ⋯ menu. */
  actionTitle(plugin: string, action: string): string;
  /** A click on a format-2 item or blocker, by its key (itemKey or blockerKey), with the route its plugin's page shows it at. */
  onOpenEntry(plugin: string, key: string, route: string): void;
  /** A choice in a format-2 row's ⋯ menu: an item's action with `--item <id>`, a blocker's with no arguments. */
  onEntryAction(plugin: string, action: string, args: string[], label: string): void;
  /** The Needs you entries the user hid; each stays out of Needs you and in its "Hidden" fold while it is there unchanged. */
  hidden: readonly HiddenEntry[];
  /** Hide in a Needs you row's ⋯ menu, with the row's title. */
  onHide(entry: HiddenEntry, title: string): void;
  /** Show again in the "Hidden" fold, by the entry's key. */
  onShowAgain(key: string): void;
  /** The keys of the Needs you entries checked for a bulk action. */
  checked: readonly string[];
  /**
   * The keys checked now, in list order: after a checkbox's click, Shift+click's range, x, Command-A,
   * or Escape, and when checked entries leave what Needs you lists.
   */
  onCheck(keys: string[]): void;
  /** Which entries Needs you lists; search finds only those too. */
  filter: NeedsYouFilter;
  onFilter(filter: NeedsYouFilter): void;
  /** A plugin's title, for its filter chip while it has nothing in Needs you. */
  pluginTitle(plugin: string): string;
  onToggle(id: string): void;
  onLoadChildPage(id: string, restart: boolean): void;
};
export type WorkGroup = { key: string; label: string; path: string | null; conversations: ConversationSummary[] };
/**
 * A conversation; a search result shows its folder and no children (`found`), and a root waiting on
 * you under Needs you has a ⋯ menu with Hide (`hideable`). Every Needs you row but a chat's children
 * has a checkbox for bulk actions (`checkable`).
 */
type ConversationRow = { kind: "conversation"; key: string; conversation: ConversationSummary; depth: number; found?: boolean; hideable?: boolean; checkable?: boolean };
/**
 * An item or a blocker of a plugin that prints status format 2. `reason` is why it needs you: its
 * Needs you group's title, such as "Run failed", or "blocks every job" for a blocker.
 */
type EntryRow = { kind: "entry"; key: string; plugin: string; pluginTitle: string; reason: string; entry: { item: StatusV2Item } | { blocker: StatusV2Blocker }; depth: 0; checkable?: boolean };
type Row = ConversationRow | EntryRow;
/** One item of a part: a plugin's item or blocker, or a root with its loaded children. */
type Entry = Row[];
/** A labeled part of the search results, or a section's only, unlabeled part. */
type Part = { key: string; label: string | null; entries: Entry[] };
/** A filter chip above Needs you: its words, such as "Chats", and how many entries it lists. */
type FilterChip = { value: NeedsYouFilter; label: string; count: number };
/** A hidden entry in the "Hidden" fold: its title and the gray line its row had. */
type HiddenRow = { key: string; title: string; meta: string };
/** What Needs you adds to a section: its filter chips (none while one kind of entry is there), what it says when its filter lists nothing, and the hidden entries. */
type NeedsYouParts = { chips: FilterChip[]; empty: string | null; hidden: HiddenRow[] };
type Section = { key: string; label: string; title?: string; count: number; parts: Part[]; needsYou?: NeedsYouParts };

/** The sections the user opened or folded, by key, which this device keeps across launches; the rest start folded except Needs you. */
const SECTIONS_PREFERENCE = "prompting-time.sidebar-sections";

const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
/** Hide in a Needs you row's ⋯ menu, after the row's actions; theirs are `action:<name>`. */
const HIDE_ITEM = { value: "hide", label: "Hide" };
const statusText: Record<EffectiveStatus, string> = {
  idle: "ready", queued: "queued", running: "working", waiting: "waiting for you",
  completed: "completed", interrupted: "interrupted", failed: "failed",
};
const statusDots: Record<EffectiveStatus, Status> = {
  idle: "idle", queued: "running", running: "running", waiting: "attention",
  completed: "idle", interrupted: "idle", failed: "error",
};

/**
 * A conversation's status in the words and dot its row and the window's header show: "waiting for
 * you" in orange, "failed" in red, "working" and "queued" with a dot that has no color, and the
 * rest with no dot.
 */
export function conversationState(conversation: Pick<ConversationSummary, "runStatus" | "rollupStatus">): { words: string; status: Status } {
  const status = effectiveConversationStatus(conversation);
  return { words: statusText[status], status: statusDots[status] };
}

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;

/** The words of a search, lowercase, as searchWork takes them. */
export function searchTokens(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

/** Whether every word, without a leading "#" or "@", is in `text`, such as "#101" in "widgets#101 · @sam". */
const matches = (text: string, tokens: readonly string[]) => {
  const words = text.toLowerCase();
  return tokens.every((token) => words.includes(token.replace(/^[#@]/, "")));
};

/**
 * What the sidebar's search finds: the blockers whose title holds every word of `query`, the items
 * of the Needs you groups whose title, detail, or project folder holds every word, in Needs you's
 * order, then the roots whose title or folder holds every word ("no folder" for one without). Each
 * word may start with "#" or "@", which it ignores.
 */
export function searchWork(work: StatusV2Work, conversations: readonly ConversationSummary[], query: string) {
  const tokens = searchTokens(query);
  return {
    blockers: work.blockers.filter(({ blocker }) => matches(blocker.title, tokens)),
    groups: work.needsYou
      .map((group) => ({ ...group, items: group.items.filter((item) => matches(`${item.title} ${item.detail ?? ""} ${item.project ? folderName(item.project) : ""}`, tokens)) }))
      .filter((group) => group.items.length > 0),
    chats: conversations.filter((conversation) => !conversation.archived
      && matches(`${conversation.title} ${conversation.projectRoot ? folderName(conversation.projectRoot) : "no folder"}`, tokens)),
  };
}

const blockerEntries = (blockers: readonly PluginBlocker[]): Entry[] => blockers.map(({ plugin, pluginTitle, blocker }) => [{ kind: "entry", key: blockerKey(plugin, blocker.id), plugin, pluginTitle, reason: "blocks every job", entry: { blocker }, depth: 0 }]);
const groupEntries = ({ plugin, pluginTitle, group, items }: PluginGroup): Entry[] => items.map((item) => [{ kind: "entry", key: itemKey(plugin, item.id), plugin, pluginTitle, reason: group.title, entry: { item }, depth: 0 }]);
const entryTitle = (row: EntryRow) => ("item" in row.entry ? row.entry.item.title : row.entry.blocker.title);
/** A chat row's gray line: its provider and status, after its folder in a search result (`found`), such as "Codex · waiting for you". */
const chatMeta = (chat: ConversationSummary, found: boolean) => `${found ? `${chat.projectRoot ? folderName(chat.projectRoot) : "no folder"} · ` : ""}${chat.provider ? providerNames[chat.provider] : "No provider"} · ${conversationState(chat).words}`;

/** What a filter chip says: "All", "Chats", or the plugin's title. */
function filterLabel(filter: NeedsYouFilter, pluginTitle: (plugin: string) => string): string {
  return filter === "all" ? "All" : filter === "chats" ? "Chats" : pluginTitle(filter.slice("plugin:".length));
}

/**
 * The chips above Needs you: All, Chats, and one per plugin by title, each with how many entries it
 * lists, none hidden. A source shows while it has entries, and the chosen one always. With one source and
 * All chosen, a chip would only repeat the list, so there are none.
 */
function filterChips(entries: readonly { plugin: string | null; pluginTitle: string | null }[], filter: NeedsYouFilter, pluginTitle: (plugin: string) => string): FilterChip[] {
  const counts = new Map<NeedsYouFilter, FilterChip>();
  for (const { plugin, pluginTitle: title } of entries) {
    const value: NeedsYouFilter = plugin === null ? "chats" : `plugin:${plugin}`;
    const chip = counts.get(value) ?? { value, label: plugin === null ? "Chats" : title ?? plugin, count: 0 };
    counts.set(value, { ...chip, count: chip.count + 1 });
  }
  if (filter !== "all" && !counts.has(filter)) counts.set(filter, { value: filter, label: filterLabel(filter, pluginTitle), count: 0 });
  const sources = [...counts.values()].sort((left, right) => Number(right.value === "chats") - Number(left.value === "chats") || left.label.localeCompare(right.label));
  if (sources.length < 2 && filter === "all") return [];
  return [{ value: "all", label: "All", count: entries.length }, ...sources];
}

/** What Needs you says when its filter lists nothing, as when the user hid all of it. */
function emptyText(filter: NeedsYouFilter, pluginTitle: (plugin: string) => string): string {
  if (filter === "chats") return "No chat waits on you";
  return filter === "all" ? "Nothing else needs you" : `Nothing from ${filterLabel(filter, pluginTitle)} needs you`;
}

function readOpenSections(): Record<string, boolean> {
  try {
    const value: unknown = JSON.parse(readPreference(SECTIONS_PREFERENCE) ?? "{}");
    return typeof value === "object" && value !== null && !Array.isArray(value)
      ? Object.fromEntries(Object.entries(value).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"))
      : {};
  } catch {
    return {};
  }
}

/**
 * The All work list, in sections that work like VS Code's sidebar panes: every section's heading,
 * with its name and count, stays on screen. The open sections share the space above the folded
 * ones, each scrolling on its own, and the folded ones stack at the bottom; a heading opens its
 * section into that space or folds it back into the stack. "Needs you", shown only while something
 * needs you, is one list in Command-J's order: the blockers of the plugins that print status format
 * 2, the items of their groups marked `needs_you`, each saying why it needs you and its plugin's
 * name, and the roots waiting on you. Chips above it choose all, the chats, or one plugin's, and
 * every row's ⋯ menu has Hide, after an item's or blocker's actions; the hidden entries wait in a
 * folded "Hidden · N" line at its end, each with Show again. Each Needs you row's checkbox, on the
 * open or focused row and on all of them while any is checked, chooses entries for the bulk bar App
 * draws: a click, Shift+click for a range, x, Command-A, and Escape. Then the chats of each project and
 * those without a folder, each root followed by its loaded children, folded until opened; a waiting
 * root that is hidden or filtered out of Needs you shows with its project's.
 * The sections the user opens or folds stay so across launches. The rows are a tree for keyboard
 * users: arrow keys move and open rows across the open sections, Home and End jump, and Enter or
 * Space opens a conversation, an item, or a blocker.
 */
export function AllWork(props: Props) {
  const [focusedId, setFocusedId] = createSignal<string | null>(null);
  // Toggles read and write `opened`, which holds every toggle at once; the signal shows it, and
  // Solid applies its writes only on the next flush.
  let opened = readOpenSections();
  const [openSections, setOpenSections] = createSignal(opened);
  const items = new Map<string, HTMLElement>();
  const openIn = (sections: Record<string, boolean>, key: string) => sections[key] ?? key === "needs-you";
  const isOpen = (key: string) => openIn(openSections(), key);
  // The section whose heading had the focus when it opened or folded. Its heading moves between the
  // open sections and the stack, which draws a new one, so the new heading takes the focus back.
  let refocus: string | null = null;
  let resultsBox: HTMLElement | undefined;
  // Whether the "Hidden" fold at the end of Needs you is open; it starts folded.
  const [hiddenOpen, setHiddenOpen] = createSignal(false);

  function toggleSection(key: string, heading: HTMLElement) {
    if (document.activeElement === heading) refocus = key;
    opened = { ...opened, [key]: !openIn(opened, key) };
    setOpenSections(opened);
    savePreference(SECTIONS_PREFERENCE, JSON.stringify(opened));
  }

  // A new search shows its results from the top, as the sections do after one.
  createEffect(() => props.query, () => {
    if (resultsBox) resultsBox.scrollTop = 0;
  });

  // When the selection moves to another row, as with Command-J or the next item after a decision,
  // a row outside the visible part of its section scrolls into view, as VS Code reveals the file it
  // opens. A row in a folded section isn't drawn, so its section stays folded.
  createEffect(() => props.selectedEntry ?? props.selectedId, (key) => {
    if (key) items.get(key)?.scrollIntoView({ block: "nearest" });
  });

  const sections = createMemo((): Section[] => {
    const chatEntries = (roots: readonly ConversationSummary[]): Entry[] => {
      const rows = visibleRows(roots, props.conversationsById, props.childPagesById, props.expandedById, props.selectedPath) as Row[];
      return rows.reduce<Entry[]>((entries, row) => {
        if (row.depth === 0) entries.push([row]);
        else entries.at(-1)?.push(row);
        return entries;
      }, []);
    };
    const filter = props.filter;
    const hidden = props.hidden;
    const chatHidden = (chat: ConversationSummary) => isHidden(hidden, chat.id, "");
    // A waiting root the filter leaves out or the user hid shows with its project's chats.
    const groups = groupWork(props.conversations, (chat) => inFilter(filter, null) && !chatHidden(chat));
    const waitingChats = waitingRoots(props.conversations);
    const work = props.pluginWork;
    const entries = needsYouEntries(work, waitingChats);
    const unhidden = entries.filter(({ key, version }) => !isHidden(hidden, key, version));
    const shownWork = narrowWork(work, hidden, filter);
    const shownChats = groups.find((group) => group.key === "needs-you")?.conversations ?? [];
    const needsYou = [...blockerEntries(shownWork.blockers), ...shownWork.needsYou.flatMap(groupEntries),
      ...chatEntries(shownChats).map((entry) => entry.map((row) => (row.kind === "conversation" && row.depth === 0 ? { ...row, hideable: true } : row)))]
      .map(([first, ...children]) => [{ ...first!, checkable: true }, ...children]);
    const single = (key: string, entries: Entry[]): Part[] => (entries.length > 0 ? [{ key, label: null, entries }] : []);
    const hiddenRows: HiddenRow[] = [
      ...blockerEntries(work.blockers.filter(({ plugin, blocker }) => isHidden(hidden, blockerKey(plugin, blocker.id), blockerVersion(blocker)))),
      ...work.needsYou.flatMap((group) => groupEntries({ ...group, items: group.items.filter((item) => isHidden(hidden, itemKey(group.plugin, item.id), itemVersion(item))) })),
    ].flatMap(([row]) => (row?.kind === "entry" && inFilter(filter, row.plugin) ? [{ key: row.key, title: entryTitle(row), meta: `${row.reason} · ${row.pluginTitle}` }] : []))
      .concat(inFilter(filter, null) ? waitingChats.filter(chatHidden).map((chat) => ({ key: chat.id, title: chat.title, meta: chatMeta(chat, false) })) : []);
    const anyHidden = entries.length > unhidden.length;
    return [
      ...(unhidden.length > 0 || anyHidden ? [{
        key: "needs-you", label: "Needs you", count: unhidden.length, parts: single("needs-you", needsYou),
        needsYou: { chips: filterChips(unhidden, filter, props.pluginTitle), empty: needsYou.length > 0 ? null : emptyText(filter, props.pluginTitle), hidden: hiddenRows },
      }] : []),
      ...groups.filter((group) => group.key !== "needs-you").map((group): Section => {
        const key = `chats:${group.path ?? "no-folder"}`;
        return { key, label: `Chats · ${group.label}`, title: group.path ?? undefined, count: group.conversations.length, parts: single(key, chatEntries(group.conversations)) };
      }),
    ];
  });

  const searching = () => searchTokens(props.query).length > 0;
  /** While searching, the results as parts: the blockers and items under Needs you, then the chats; only those the filter lists, and none the user hid. */
  const results = createMemo((): Part[] => {
    if (!searching()) return [];
    const { blockers, groups, chats } = searchWork(narrowWork(props.pluginWork, props.hidden, props.filter), inFilter(props.filter, null) ? props.conversations : [], props.query);
    const work = [...blockerEntries(blockers), ...groups.flatMap(groupEntries)];
    return [
      ...(work.length > 0 ? [{ key: "found:needs-you", label: "Needs you", entries: work }] : []),
      ...(chats.length > 0 ? [{ key: "found:chats", label: "Chats", entries: chats.map((conversation): Entry => [{ kind: "conversation", key: conversation.id, conversation, depth: 0, found: true }]) }] : []),
    ];
  });

  /** The open sections, which share the space above the stack, and the folded ones in the stack; each in list order. */
  const openList = createMemo(() => sections().filter((section) => isOpen(section.key)));
  const foldedList = createMemo(() => sections().filter((section) => !isOpen(section.key)));
  const rows = createMemo(() => (searching()
    ? results().flatMap((part) => part.entries.flat())
    : openList().flatMap((section) => section.parts.flatMap((part) => part.entries.flat()))));
  const tabStop = () => (rows().some((row) => row.key === focusedId()) ? focusedId() : rows()[0]?.key);

  /** The keys of the rows with a checkbox, in list order: what Needs you lists now. */
  const checkable = createMemo(() => sections().find((section) => section.key === "needs-you")?.parts.flatMap((part) => part.entries.map((entry) => entry[0]!.key)) ?? []);
  const checkedKeys = createMemo(() => new Set(props.checked));
  /** While any row is checked, every Needs you row shows its checkbox. */
  const selecting = () => props.checked.length > 0;
  // The row Shift+click's range starts from: the last one checked or unchecked.
  let anchor: string | null = null;

  // A checked entry that leaves what Needs you lists, as when the user hides it, acts on it, or
  // chooses another chip, is no longer checked.
  createEffect(() => props.checked.filter((key) => checkable().includes(key)), (kept) => {
    if (kept.length !== untrack(() => props.checked.length)) props.onCheck(kept);
  });

  /** Checks or unchecks a row, with Shift (`range`) every row from the anchor to it too, all as the row becomes. */
  function toggleCheck(key: string, range: boolean) {
    const order = checkable();
    const next = new Set(props.checked);
    const on = !next.has(key);
    const from = range && anchor !== null ? order.indexOf(anchor) : -1;
    const to = order.indexOf(key);
    const keys = from === -1 ? [key] : order.slice(Math.min(from, to), Math.max(from, to) + 1);
    keys.forEach((each) => (on ? next.add(each) : next.delete(each)));
    anchor = key;
    props.onCheck(order.filter((each) => next.has(each)));
  }

  /** x checks or unchecks the focused row, Command-A checks every row Needs you lists, and Escape unchecks them all; true when the key did one. */
  function checkKey(event: KeyboardEvent, row: Row): boolean {
    if (event.key === "Escape" && selecting()) {
      props.onCheck([]);
      anchor = null;
    } else if (!row.checkable || event.altKey) {
      return false;
    } else if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
      props.onCheck(checkable());
    } else if (event.key === "x" && !event.metaKey && !event.ctrlKey) {
      toggleCheck(row.key, false);
    } else {
      return false;
    }
    event.preventDefault();
    event.stopPropagation();
    return true;
  }

  /** A Needs you row's checkbox. It takes no tab stop, as x checks the focused row; a click leaves the focus on the row. */
  function checkbox(key: string, title: () => string) {
    return (
      <input
        type="checkbox"
        class="work-check"
        tabindex={-1}
        aria-label={`Select ${title()}`}
        checked={checkedKeys().has(key)}
        onClick={(event) => {
          event.stopPropagation();
          toggleCheck(key, event.shiftKey);
          items.get(key)?.focus();
        }}
      />
    );
  }
  /** A checkable row's aria-checked while any row is checked. */
  const ariaChecked = (row: Row) => (row.checkable && selecting() ? (checkedKeys().has(row.key) ? "true" : "false") : undefined);

  function focus(id: string | null | undefined) {
    if (!id) return;
    setFocusedId(id);
    items.get(id)?.focus();
  }

  function open(row: EntryRow) {
    props.onOpenEntry(row.plugin, row.key, "item" in row.entry ? row.entry.item.route : row.entry.blocker.route);
  }

  function navigate(event: KeyboardEvent, row: Row) {
    if (checkKey(event, row)) return;
    const all = rows();
    const index = all.findIndex((item) => item.key === row.key);
    switch (event.key) {
      case "ArrowDown": event.preventDefault(); focus(all[index + 1]?.key); return;
      case "ArrowUp": event.preventDefault(); focus(all[index - 1]?.key); return;
      case "Home": event.preventDefault(); focus(all[0]?.key); return;
      case "End": event.preventDefault(); focus(all.at(-1)?.key); return;
    }
    if (row.kind !== "conversation") {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        open(row);
      }
      return;
    }
    const id = row.conversation.id;
    const expanded = props.expandedById[id] === true;
    switch (event.key) {
      case "ArrowRight":
        event.preventDefault();
        if (row.conversation.hasChildren && !expanded) props.onToggle(id);
        else if (expanded && all[index + 1]?.depth === row.depth + 1) focus(all[index + 1]?.key);
        break;
      case "ArrowLeft":
        event.preventDefault();
        if (expanded) props.onToggle(id);
        else focus(row.conversation.parentId);
        break;
      case "Enter": case " ": event.preventDefault(); props.onSelect(id); break;
    }
  }

  /** The tree attributes and handlers every row shares, the tab stop as a getter so it stays live; `key` never changes for a row. */
  const treeItem = (key: string, row: () => Row) => ({
    element: "li" as const,
    role: "treeitem" as const,
    ref: (element: HTMLElement) => items.set(key, element),
    get tabindex() {
      return tabStop() === key ? 0 : -1;
    },
    onFocus: (event: FocusEvent) => {
      if (event.target === event.currentTarget) setFocusedId(key);
    },
    onKeyDown: (event: KeyboardEvent) => {
      if (event.target === event.currentTarget) navigate(event, row());
    },
  });

  /** One row. Rows keep the place of a dot they don't draw, so the gray lines of a list start in one column. */
  function renderRow(row: () => Row): JSX.Element {
    const first = untrack(row);
    if (first.kind === "entry") {
      // An entry's key holds its plugin and ID, so neither changes; nor does whether it is a blocker.
      const current = () => row() as EntryRow;
      const item = () => {
        const entry = current().entry;
        return "item" in entry ? entry.item : null;
      };
      const blocker = () => {
        const entry = current().entry;
        return "blocker" in entry ? entry.blocker : null;
      };
      const title = () => entryTitle(current());
      // Why it needs you, beside its dot, then its plugin's name.
      const meta = () => `${current().reason} · ${current().pluginTitle}`;
      const menuItems = () => {
        const names = item()?.actions ?? (blocker()?.action ? [blocker()!.action!] : []);
        return [...names.map((name) => ({ value: `action:${name}`, label: props.actionTitle(current().plugin, name) })), HIDE_ITEM];
      };
      const version = () => {
        const shown = item();
        return shown ? itemVersion(shown) : blockerVersion(blocker()!);
      };
      const isBlocker = "blocker" in first.entry;
      return (
        <ListRow
          {...treeItem(first.key, row)}
          aria-label={`${title()}, ${meta()}`}
          aria-level={1}
          aria-selected={props.selectedEntry === first.key ? "true" : "false"}
          aria-checked={ariaChecked(current())}
          class="work-row work-entry"
          style={{ "--work-depth": "0" }}
          icon={isBlocker ? <AlertIcon /> : <BotIcon />}
          leading={first.checkable ? checkbox(first.key, title) : undefined}
          title={title()}
          status={item()?.dot ?? blocker()?.dot ?? "idle"}
          meta={meta()}
          trailing={
            // The menu opens on a click and runs the choice; neither opens the row.
            <span class="work-entry-menu" onClick={(event) => event.stopPropagation()}>
              <Menu
                label={`Actions for ${title()}`}
                icon={<EllipsisIcon />}
                items={menuItems()}
                onSelect={(value) => {
                  if (value === HIDE_ITEM.value) return props.onHide({ key: first.key, plugin: current().plugin, version: version() }, title());
                  const shown = item();
                  props.onEntryAction(current().plugin, value.slice("action:".length), shown ? ["--item", shown.id] : [], title());
                }}
              />
            </span>
          }
          onClick={() => open(current())}
        />
      );
    }
    // The row's key is its conversation id, so the id never changes.
    const id = first.key;
    const conversation = () => (row() as ConversationRow).conversation;
    const state = () => conversationState(conversation());
    const found = first.kind === "conversation" && first.found === true;
    const hideable = first.kind === "conversation" && first.hideable === true;
    const expanded = () => !found && props.expandedById[id] === true;
    const page = () => props.childPagesById[id];
    const expandable = () => !found && (conversation().hasChildren || page() !== undefined);
    const meta = () => chatMeta(conversation(), found);
    return (
      <ListRow
        {...treeItem(id, row)}
        aria-label={`${conversation().title}, ${meta().replaceAll(" · ", ", ")}`}
        aria-level={row().depth + 1}
        aria-selected={props.selectedId === id ? "true" : "false"}
        aria-expanded={expandable() ? (expanded() ? "true" : "false") : undefined}
        aria-checked={ariaChecked(row())}
        class="work-row"
        style={{ "--work-depth": String(Math.min(row().depth, 8)) }}
        icon={<ChatIcon />}
        leading={first.checkable ? checkbox(id, () => conversation().title) : undefined}
        title={conversation().title}
        status={state().status}
        meta={meta()}
        trailing={
          <>
          <Show when={hideable}>
            <span class="work-entry-menu" onClick={(event) => event.stopPropagation()}>
              <Menu label={`Actions for ${conversation().title}`} icon={<EllipsisIcon />} items={[HIDE_ITEM]} onSelect={() => props.onHide({ key: id, plugin: null, version: "" }, conversation().title)} />
            </span>
          </Show>
          <Show when={expandable()}>
            <button
              type="button"
              class="work-disclosure"
              tabindex={-1}
              aria-label={`${expanded() ? "Collapse" : "Expand"} ${conversation().title}`}
              onClick={(event) => {
                event.stopPropagation();
                props.onToggle(id);
              }}
            >
              {expanded() ? <ChevronDownIcon /> : <ChevronRightIcon />}
            </button>
          </Show>
          </>
        }
        onClick={() => props.onSelect(id)}
      >
        <Show when={expanded() && page()}>
          {(current) => (
            <div class="child-page-controls" onClick={(event) => event.stopPropagation()}>
              <Show when={current().loading}><span role="status">Loading conversations…</span></Show>
              <Show when={current().error}>
                <p role="alert">{current().error}</p>
                <button type="button" class="pt-button" onClick={() => props.onLoadChildPage(id, true)}>Retry conversations</button>
              </Show>
              <Show when={current().nextCursor}>
                <button type="button" class="pt-button" disabled={current().loading} onClick={() => props.onLoadChildPage(id, false)}>Load more conversations</button>
              </Show>
              <Show when={current().evicted}>
                <div role="note">
                  <span>Some conversations are outside this bounded view.</span>
                  <button type="button" class="pt-button" disabled={current().loading} onClick={() => props.onLoadChildPage(id, true)}>Reload conversations</button>
                </div>
              </Show>
            </div>
          )}
        </Show>
      </ListRow>
    );
  }

  /** A section's heading: a toggle with its name and count, which opens the section or folds it into the stack. */
  function heading(section: () => Section, labelId: string, open: boolean) {
    const key = untrack(section).key;
    return (
      <h2 class="work-group-heading">
        <button
          type="button"
          class="work-group-toggle"
          aria-expanded={open ? "true" : "false"}
          title={section().title}
          ref={(element: HTMLButtonElement) => {
            if (refocus !== key) return;
            refocus = null;
            queueMicrotask(() => element.focus());
          }}
          onClick={(event) => toggleSection(key, event.currentTarget)}
        >
          {open ? <ChevronDownIcon /> : <ChevronRightIcon />}
          <span id={labelId} class="work-group-label">{section().label}</span>{" "}
          <span class="work-group-count">{section().count}</span>
        </button>
      </h2>
    );
  }

  const resultCount = () => results().reduce((sum, part) => sum + part.entries.length, 0);
  /** Where search looks, after its count while a filter is chosen: " in Chats" or " in <plugin>". */
  const searchScope = () => (props.filter === "all" ? "" : ` in ${filterLabel(props.filter, props.pluginTitle)}`);

  /** The chips that choose which entries Needs you lists, each with its count; a click chooses one. */
  function filterRow(chips: () => FilterChip[]) {
    return (
      <div class="work-filters" role="group" aria-label="Show in Needs you">
        <For each={chips()} keyed={(chip) => chip.value}>
          {(chip) => (
            <button type="button" class="work-filter" aria-pressed={props.filter === chip().value ? "true" : "false"} onClick={() => props.onFilter(chip().value)}>
              {chip().label} <span class="work-filter-count">{chip().count}</span>
            </button>
          )}
        </For>
      </div>
    );
  }

  /** "Hidden · N", folded at the end of Needs you; a click opens the hidden entries, each with Show again. */
  function hiddenFold(rows: () => HiddenRow[]) {
    const listId = createUniqueId();
    return (
      <div class="work-hidden">
        <button type="button" class="work-hidden-toggle" aria-expanded={hiddenOpen() ? "true" : "false"} aria-controls={listId} onClick={() => setHiddenOpen((open) => !open)}>
          {hiddenOpen() ? <ChevronDownIcon /> : <ChevronRightIcon />}
          Hidden · {rows().length}
        </button>
        <Show when={hiddenOpen()}>
          <ul id={listId} class="work-hidden-list" aria-label="Hidden">
            <For each={rows()} keyed={(row) => row.key}>
              {(row) => (
                <li class="work-hidden-row">
                  <span class="work-hidden-text">
                    <span class="work-hidden-title">{row().title}</span>
                    <span class="work-hidden-meta">{row().meta}</span>
                  </span>
                  <button type="button" class="pt-button" aria-label={`Show ${row().title} again`} onClick={() => props.onShowAgain(row().key)}>Show again</button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
    );
  }

  return (
    <div class={["all-work", { "work-selecting": selecting() }]}>
      <Show when={searching()}>
        <section
          class="work-results"
          aria-label="Search results"
          ref={(element: HTMLElement) => {
            resultsBox = element;
          }}
        >
          <p class="work-empty" role="status">{resultCount() === 0 ? "Nothing matches" : `${resultCount()} ${resultCount() === 1 ? "result" : "results"}`}{searchScope()} · Esc clears</p>
          <For each={results()} keyed={(part) => part.key}>
            {(part) => {
              const partId = createUniqueId();
              return (
                <div class="work-part">
                  <h3 class="work-part-heading"><span id={partId}>{part().label}</span> · {part().entries.length}</h3>
                  <ul role="tree" aria-labelledby={partId} class="work-rows">
                    <For each={part().entries.flat()} keyed={(row) => row.key}>{(row) => renderRow(row)}</For>
                  </ul>
                </div>
              );
            }}
          </For>
        </section>
      </Show>
      <Show when={!searching()}>
        <div class="work-open">
          <For each={openList()} keyed={(section) => section.key}>
            {(section) => {
              const labelId = createUniqueId();
              return (
                <section class="work-section">
                  {heading(section, labelId, true)}
                  <div class="work-section-body">
                    <Show when={section().needsYou?.chips.length}>{filterRow(() => section().needsYou!.chips)}</Show>
                    <For each={section().parts} keyed={(part) => part.key}>
                      {(part) => {
                        const partId = createUniqueId();
                        return (
                          <div class="work-part">
                            <Show when={part().label}>
                              {(label) => <h3 class="work-part-heading"><span id={partId}>{label()}</span> · {part().entries.length}</h3>}
                            </Show>
                            <ul role="tree" aria-labelledby={part().label ? partId : labelId} class="work-rows">
                              <For each={part().entries.flat()} keyed={(row) => row.key}>{(row) => renderRow(row)}</For>
                            </ul>
                          </div>
                        );
                      }}
                    </For>
                    <Show when={section().needsYou?.empty}>{(text) => <p class="work-empty">{text()}</p>}</Show>
                    <Show when={section().needsYou?.hidden.length}>{hiddenFold(() => section().needsYou!.hidden)}</Show>
                  </div>
                </section>
              );
            }}
          </For>
        </div>
        <Show when={foldedList().length > 0}>
          <div class="work-folded">
            <For each={foldedList()} keyed={(section) => section.key}>
              {(section) => <section class="work-section">{heading(section, createUniqueId(), false)}</section>}
            </For>
          </div>
        </Show>
      </Show>
    </div>
  );
}

/** The roots waiting on you, in list order: what ⌘J steps through. */
export function waitingRoots(conversations: readonly ConversationSummary[]): ConversationSummary[] {
  return conversations.filter((conversation) => !conversation.archived && effectiveConversationStatus(conversation) === "waiting");
}

/**
 * "Needs you" first, holding every root waiting on you (a root also waits when a descendant does)
 * that `inNeedsYou` keeps there, then one group per project, sorted by name, then conversations
 * without a folder. A root shows in one group only; a waiting root left out of Needs you, as one the
 * user hid, shows in its project's.
 */
export function groupWork(conversations: readonly ConversationSummary[], inNeedsYou: (conversation: ConversationSummary) => boolean = () => true): WorkGroup[] {
  const active = conversations.filter((conversation) => !conversation.archived);
  const needsYou: WorkGroup = { key: "needs-you", label: "Needs you", path: null, conversations: waitingRoots(active).filter(inNeedsYou) };
  const waiting = new Set(needsYou.conversations);
  const projects = new Map<string, WorkGroup>();
  const projectless: WorkGroup = { key: "projectless", label: "no folder", path: null, conversations: [] };
  for (const conversation of active) {
    if (waiting.has(conversation)) continue;
    if (!conversation.projectRoot) {
      projectless.conversations.push(conversation);
      continue;
    }
    let group = projects.get(conversation.projectRoot);
    if (!group) {
      group = { key: `project:${conversation.projectRoot}`, label: projectName(conversation.projectRoot), path: conversation.projectRoot, conversations: [] };
      projects.set(conversation.projectRoot, group);
    }
    group.conversations.push(conversation);
  }
  const sorted = [...projects.values()].sort((left, right) => left.label.localeCompare(right.label) || (left.path ?? "").localeCompare(right.path ?? ""));
  const labelCounts = new Map<string, number>();
  sorted.forEach(({ label }) => labelCounts.set(label, (labelCounts.get(label) ?? 0) + 1));
  for (const group of sorted) {
    if ((labelCounts.get(group.label) ?? 0) > 1 && group.path) group.label = `${group.label} — ${group.path}`;
  }
  return [needsYou, ...sorted, projectless].filter((group) => group.conversations.length > 0);
}

function visibleRows(roots: readonly ConversationSummary[], nodes: Props["conversationsById"],
  pages: Props["childPagesById"], expanded: Props["expandedById"], selectedPath: SelectedPath | null): ConversationRow[] {
  const rows: ConversationRow[] = [];
  const row = (conversation: ConversationSummary, depth: number): ConversationRow => ({ kind: "conversation", key: conversation.id, conversation, depth });
  const stack = [...roots].reverse().map((conversation) => row(conversation, 0));
  const seen = new Set<string>();
  while (stack.length) {
    const current = stack.pop()!;
    const id = current.conversation.id;
    if (seen.has(id)) continue;
    seen.add(id);
    rows.push(current);
    if (!expanded[id]) continue;
    const ids = new Set([...(pages[id]?.pages.flatMap((page) => page.ids) ?? []),
      ...(selectedPath?.ids.filter((childId) => nodes[childId]?.parentId === id) ?? [])]);
    for (const childId of [...ids].reverse()) {
      const child = nodes[childId];
      if (child && child.parentId === id && !child.archived) stack.push(row(child, current.depth + 1));
    }
  }
  return rows;
}

function projectName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}
