import { createEffect, createMemo, createSignal, createUniqueId, For, Show, untrack } from "solid-js";
import type { JSX } from "@solidjs/web";
import { AlertIcon, BotIcon, ChatIcon, ChevronDownIcon, ChevronRightIcon, EllipsisIcon, ListRow, Menu, type Status } from "@prompting-time/ui";
import type { ConversationSummary, ProviderId } from "../../bridge/types";
import { effectiveConversationStatus, type ChildPageSnapshot, type EffectiveStatus, type SelectedPath } from "../../app/store";
import { readPreference, savePreference } from "../../app/preferences";
import { blockerKey, itemKey, type PluginBlocker, type PluginGroup, type StatusV2Blocker, type StatusV2Item, type StatusV2Work } from "../agents/statusV2";

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
  onToggle(id: string): void;
  onLoadChildPage(id: string, restart: boolean): void;
};
export type WorkGroup = { key: string; label: string; path: string | null; conversations: ConversationSummary[] };
/** A conversation; a search result shows its folder and no children (`found`). */
type ConversationRow = { kind: "conversation"; key: string; conversation: ConversationSummary; depth: number; found?: boolean };
/**
 * An item or a blocker of a plugin that prints status format 2. `reason` is why it needs you: its
 * Needs you group's title, such as "Run failed", or "blocks every job" for a blocker.
 */
type EntryRow = { kind: "entry"; key: string; plugin: string; pluginTitle: string; reason: string; entry: { item: StatusV2Item } | { blocker: StatusV2Blocker }; depth: 0 };
type Row = ConversationRow | EntryRow;
/** One item of a part: a plugin's item or blocker, or a root with its loaded children. */
type Entry = Row[];
/** A labeled part of the search results, or a section's only, unlabeled part. */
type Part = { key: string; label: string | null; entries: Entry[] };
type Section = { key: string; label: string; title?: string; count: number; parts: Part[] };

/** The sections the user opened or folded, by key, which this device keeps across launches; the rest start folded except Needs you. */
const SECTIONS_PREFERENCE = "prompting-time.sidebar-sections";

const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
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
 * name, and the roots waiting on you. Then the chats of each project and those without a folder,
 * each root followed by its loaded children, folded until opened. Each item or blocker row has a ⋯
 * menu of its actions.
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
    const groups = groupWork(props.conversations);
    const waitingChats = groups.find((group) => group.key === "needs-you")?.conversations ?? [];
    const work = props.pluginWork;
    const needsYou = [...blockerEntries(work.blockers), ...work.needsYou.flatMap(groupEntries), ...chatEntries(waitingChats)];
    const single = (key: string, entries: Entry[]): Part[] => [{ key, label: null, entries }];
    return [
      ...(needsYou.length > 0 ? [{ key: "needs-you", label: "Needs you", count: needsYou.length, parts: single("needs-you", needsYou) }] : []),
      ...groups.filter((group) => group.key !== "needs-you").map((group): Section => {
        const key = `chats:${group.path ?? "no-folder"}`;
        return { key, label: `Chats · ${group.label}`, title: group.path ?? undefined, count: group.conversations.length, parts: single(key, chatEntries(group.conversations)) };
      }),
    ];
  });

  const searching = () => searchTokens(props.query).length > 0;
  /** While searching, the results as parts: the blockers and items under Needs you, then the chats. */
  const results = createMemo((): Part[] => {
    if (!searching()) return [];
    const { blockers, groups, chats } = searchWork(props.pluginWork, props.conversations, props.query);
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

  function focus(id: string | null | undefined) {
    if (!id) return;
    setFocusedId(id);
    items.get(id)?.focus();
  }

  function open(row: EntryRow) {
    props.onOpenEntry(row.plugin, row.key, "item" in row.entry ? row.entry.item.route : row.entry.blocker.route);
  }

  function navigate(event: KeyboardEvent, row: Row) {
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
      const title = () => item()?.title ?? blocker()?.title ?? "";
      // Why it needs you, beside its dot, then its plugin's name.
      const meta = () => `${current().reason} · ${current().pluginTitle}`;
      const menuItems = () => {
        const names = item()?.actions ?? (blocker()?.action ? [blocker()!.action!] : []);
        return names.map((name) => ({ value: name, label: props.actionTitle(current().plugin, name) }));
      };
      const isBlocker = "blocker" in first.entry;
      return (
        <ListRow
          {...treeItem(first.key, row)}
          aria-label={`${title()}, ${meta()}`}
          aria-level={1}
          aria-selected={props.selectedEntry === first.key ? "true" : "false"}
          class="work-row work-entry"
          style={{ "--work-depth": "0" }}
          icon={isBlocker ? <AlertIcon /> : <BotIcon />}
          title={title()}
          status={item()?.dot ?? blocker()?.dot ?? "idle"}
          meta={meta()}
          trailing={
            <Show when={menuItems().length > 0}>
              {/* The menu opens on a click and runs the choice; neither opens the row. */}
              <span class="work-entry-menu" onClick={(event) => event.stopPropagation()}>
                <Menu
                  label={`Actions for ${title()}`}
                  icon={<EllipsisIcon />}
                  items={menuItems()}
                  onSelect={(action) => {
                    const shown = item();
                    props.onEntryAction(current().plugin, action, shown ? ["--item", shown.id] : [], title());
                  }}
                />
              </span>
            </Show>
          }
          onClick={() => open(current())}
        />
      );
    }
    // The row's key is its conversation id, so the id never changes.
    const id = first.key;
    const conversation = () => (row() as ConversationRow).conversation;
    const state = () => conversationState(conversation());
    const provider = () => (conversation().provider ? providerNames[conversation().provider!] : "No provider");
    const found = first.kind === "conversation" && first.found === true;
    const expanded = () => !found && props.expandedById[id] === true;
    const page = () => props.childPagesById[id];
    const expandable = () => !found && (conversation().hasChildren || page() !== undefined);
    const meta = () => `${found ? `${conversation().projectRoot ? folderName(conversation().projectRoot!) : "no folder"} · ` : ""}${provider()} · ${state().words}`;
    return (
      <ListRow
        {...treeItem(id, row)}
        aria-label={`${conversation().title}, ${meta().replaceAll(" · ", ", ")}`}
        aria-level={row().depth + 1}
        aria-selected={props.selectedId === id ? "true" : "false"}
        aria-expanded={expandable() ? (expanded() ? "true" : "false") : undefined}
        class="work-row"
        style={{ "--work-depth": String(Math.min(row().depth, 8)) }}
        icon={<ChatIcon />}
        title={conversation().title}
        status={state().status}
        meta={meta()}
        trailing={
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

  return (
    <div class="all-work">
      <Show when={searching()}>
        <section
          class="work-results"
          aria-label="Search results"
          ref={(element: HTMLElement) => {
            resultsBox = element;
          }}
        >
          <p class="work-empty" role="status">{resultCount() === 0 ? "Nothing matches" : `${resultCount()} ${resultCount() === 1 ? "result" : "results"}`} · Esc clears</p>
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
 * "Needs you" first, holding every root waiting on you (a root also waits when a descendant does),
 * then one group per project, sorted by name, then conversations without a folder. A root shows in
 * one group only.
 */
export function groupWork(conversations: readonly ConversationSummary[]): WorkGroup[] {
  const active = conversations.filter((conversation) => !conversation.archived);
  const needsYou: WorkGroup = { key: "needs-you", label: "Needs you", path: null, conversations: waitingRoots(active) };
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
