import { createEffect, createMemo, createSignal, createUniqueId, For, Show, untrack } from "solid-js";
import type { JSX } from "@solidjs/web";
import { AlertIcon, ChatIcon, ChevronDownIcon, ChevronRightIcon, CloseIcon, ListRow, PullRequestIcon, type Status } from "@prompting-time/ui";
import type { ConversationSummary, ProviderId } from "../../bridge/types";
import { effectiveConversationStatus, type ChildPageSnapshot, type EffectiveStatus, type SelectedPath } from "../../app/store";
import { readPreference, savePreference } from "../../app/preferences";
import { causeKey, needsYou, reviewCategory, reviewKey, reviewMatches, reviewRowMeta, reviewState, reviewTime, reviewTitle, searchTokens, shortDate, shortPullRequest, waitingText, type BlockingCause, type PluginReview, type ReviewCategory } from "../agents/reviews";

type Props = {
  /** Active roots in list order, already filtered by status. */
  conversations: readonly ConversationSummary[];
  conversationsById: Readonly<Record<string, ConversationSummary>>;
  childPagesById: Readonly<Record<string, ChildPageSnapshot>>;
  expandedById: Readonly<Record<string, boolean>>;
  selectedId: string | null;
  selectedPath: SelectedPath | null;
  /** The plugins' reviews, the newest run of each pull request, in every state; reviewCategory places each. */
  reviews: readonly PluginReview[];
  /** The causes that block every review; each with a waiting review gets a section of its own. */
  causes: readonly PluginCause[];
  /** The key (reviewKey or causeKey) of the review or cause the workspace shows, if any. */
  selectedReview: string | null;
  /** What the sidebar's search holds; while it has words, searchWork's results replace the sections. */
  query: string;
  onSelect(id: string): void;
  onOpenReview(plugin: string, runId: string): void;
  onOpenCause(plugin: string, cause: string): void;
  /** Whether a plugin can take its reviews out of the lists, which puts Remove on their rows. */
  canRemove(plugin: string): boolean;
  /** Remove on a review's row, which shows while the pointer is on the row. */
  onRemoveReview(plugin: string, runId: string): void;
  onToggle(id: string): void;
  onLoadChildPage(id: string, restart: boolean): void;
};
export type WorkGroup = { key: string; label: string; path: string | null; conversations: ConversationSummary[] };
/** A cause that blocks every review, and the plugin that reports it. */
export type PluginCause = { plugin: string; cause: BlockingCause };
/** A conversation; a search result shows its folder and no children (`found`). */
type ConversationRow = { kind: "conversation"; key: string; conversation: ConversationSummary; depth: number; found?: boolean };
type ReviewRow = { kind: "review"; key: string; item: PluginReview; depth: 0 };
type CauseRow = { kind: "cause"; key: string; item: PluginCause; depth: 0 };
type Row = ConversationRow | ReviewRow | CauseRow;
/** One item of a part: a review, a cause, or a root with its loaded children. */
type Entry = Row[];
/** A labeled part of a section, or its only, unlabeled part. */
type Part = { key: string; label: string | null; hint?: string; entries: Entry[] };
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

/** The groups search results show reviews in, in this order, by where each review lists. */
const RESULT_GROUPS: ReadonlyArray<[label: string, categories: readonly ReviewCategory[]]> = [
  ["Needs you", ["ready", "look", "failed"]],
  ["Reviewing", ["reviewing"]],
  ["Waiting", ["waiting"]],
  ["Done", ["done"]],
  ["Removed", ["removed"]],
];

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;

/**
 * What the sidebar's search finds: the reviews every word of `query` finds (reviewMatches), in
 * every state, grouped by where each lists and newest first in each group, then the roots whose
 * title or folder holds every word ("no folder" for one without).
 */
export function searchWork(reviews: readonly PluginReview[], conversations: readonly ConversationSummary[], query: string) {
  const tokens = searchTokens(query);
  const found = reviews.filter(({ review }) => reviewMatches(review, tokens))
    .sort((left, right) => reviewTime(right.review).localeCompare(reviewTime(left.review)));
  return {
    groups: RESULT_GROUPS.map(([label, categories]) => ({ label, reviews: found.filter(({ review }) => categories.includes(reviewCategory(review))) }))
      .filter((group) => group.reviews.length > 0),
    chats: conversations.filter((conversation) => {
      const words = `${conversation.title} ${conversation.projectRoot ? folderName(conversation.projectRoot) : "no folder"}`.toLowerCase();
      return !conversation.archived && tokens.every((token) => words.includes(token.replace(/^[#@]/, "")));
    }),
  };
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
 * section into that space or folds it back into the stack. Only "Needs you" starts open: every
 * review ready to approve, review that needs a look, review that failed for a reason of its own,
 * and root waiting on you. Then Reviewing, one section per cause that blocks every review with its
 * one row, the chats of each project and those without a folder, each root followed by its loaded
 * children, and Done. The sections the user opens or folds stay so across launches. The rows are a
 * tree for keyboard users: arrow keys move and open rows across the open sections, Home and End
 * jump, and Enter or Space opens a conversation, a review, or a cause.
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

  const sections = createMemo((): Section[] => {
    const reviewRow = (item: PluginReview): Entry => [{ kind: "review", key: reviewKey(item.plugin, item.review.run_id), item, depth: 0 }];
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
    const { ready, look, failed } = needsYou(props.reviews);
    const needsParts: Part[] = [
      { key: "ready", label: "Ready to approve", hint: "The plugin judges each safe to approve, or, when it doesn't say, says approve once CI passes.", entries: ready.map(reviewRow) },
      { key: "look", label: "Needs a look", hint: "The plugin judges each not safe to approve, or, when it doesn't say, says look first or skipped part of the change.", entries: look.map(reviewRow) },
      { key: "failed", label: "Review failed", hint: "Each failed for a reason of its own; open one for the reason, Retry review, and Ask why.", entries: failed.map(reviewRow) },
      { key: "chats", label: "Chats waiting on you", hint: "An agent asked you something or waits for your approval.", entries: chatEntries(waitingChats) },
    ].filter((part) => part.entries.length > 0);
    const reviewing = props.reviews.filter(({ review }) => reviewCategory(review) === "reviewing")
      .sort((left, right) => right.review.started_at.localeCompare(left.review.started_at));
    const done = props.reviews.filter(({ review }) => reviewCategory(review) === "done")
      .sort((left, right) => reviewTime(right.review).localeCompare(reviewTime(left.review)));
    const single = (key: string, entries: Entry[]): Part[] => [{ key, label: null, entries }];
    return [
      { key: "needs-you", label: "Needs you", count: needsParts.reduce((sum, part) => sum + part.entries.length, 0), parts: needsParts },
      ...(reviewing.length > 0 ? [{ key: "reviewing", label: "Reviewing", count: reviewing.length, parts: single("reviewing", reviewing.map(reviewRow)) }] : []),
      ...props.causes.filter(({ cause }) => cause.reviews.length > 0).map(({ plugin, cause }): Section => {
        const key = causeKey(plugin, cause.id);
        return { key: `waiting:${key}`, label: `Waiting · ${cause.summary}`, count: cause.reviews.length, parts: single(key, [[{ kind: "cause", key, item: { plugin, cause }, depth: 0 }]]) };
      }),
      ...groups.filter((group) => group.key !== "needs-you").map((group): Section => {
        const key = `chats:${group.path ?? "no-folder"}`;
        return { key, label: `Chats · ${group.label}`, title: group.path ?? undefined, count: group.conversations.length, parts: single(key, chatEntries(group.conversations)) };
      }),
      ...(done.length > 0 ? [{ key: "done", label: "Done", count: done.length, parts: single("done", done.map(reviewRow)) }] : []),
    ];
  });

  const searching = () => searchTokens(props.query).length > 0;
  /** While searching, the results as parts: one per group of reviews, then the chats. */
  const results = createMemo((): Part[] => {
    if (!searching()) return [];
    const { groups, chats } = searchWork(props.reviews, props.conversations, props.query);
    return [
      ...groups.map((group): Part => ({
        key: `found:${group.label}`, label: group.label,
        entries: group.reviews.map((item) => [{ kind: "review", key: reviewKey(item.plugin, item.review.run_id), item, depth: 0 }]),
      })),
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

  function open(row: ReviewRow | CauseRow) {
    if (row.kind === "review") props.onOpenReview(row.item.plugin, row.item.review.run_id);
    else props.onOpenCause(row.item.plugin, row.item.cause.id);
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

  function renderRow(row: () => Row): JSX.Element {
    const first = untrack(row);
    if (first.kind === "cause") {
      // A cause's key holds its plugin and ID, so neither changes.
      const item = () => (row() as CauseRow).item;
      const meta = () => `${waitingText(item().cause.reviews.length)}${item().cause.since ? ` · since ${shortDate(item().cause.since!)}` : ""}`;
      return (
        <ListRow
          {...treeItem(first.key, row)}
          aria-label={`${item().cause.summary}, ${meta()}`}
          aria-level={1}
          aria-selected={props.selectedReview === first.key ? "true" : "false"}
          class="work-row"
          style={{ "--work-depth": "0" }}
          icon={<AlertIcon />}
          title={item().cause.summary}
          status="error"
          meta={meta()}
          onClick={() => open(row() as CauseRow)}
        />
      );
    }
    if (first.kind === "review") {
      // A review's key holds its plugin and run, so neither changes.
      const item = () => (row() as ReviewRow).item;
      const meta = () => reviewRowMeta(item().review);
      // Only search finds a removed review, which it tags.
      const removed = () => item().review.removed_by_user;
      return (
        <ListRow
          {...treeItem(first.key, row)}
          aria-label={`${reviewTitle(item().review)}, ${removed() ? "removed, " : ""}${meta()}`}
          aria-level={1}
          aria-selected={props.selectedReview === first.key ? "true" : "false"}
          class="work-row work-review"
          data-removed={removed() ? "" : undefined}
          style={{ "--work-depth": "0" }}
          icon={<PullRequestIcon />}
          title={reviewTitle(item().review)}
          count={removed() ? "removed" : undefined}
          status={removed() ? "idle" : reviewState(item().review).status}
          meta={meta()}
          trailing={
            <Show when={!removed() && props.canRemove(item().plugin)}>
              <button
                type="button"
                class="work-remove"
                tabindex={-1}
                aria-label={`Remove ${shortPullRequest(item().review)}`}
                title={`Remove ${shortPullRequest(item().review)} from your lists`}
                onClick={(event) => {
                  event.stopPropagation();
                  props.onRemoveReview(item().plugin, item().review.run_id);
                }}
              >
                <CloseIcon />
              </button>
            </Show>
          }
          onClick={() => props.onOpenReview(item().plugin, item().review.run_id)}
        />
      );
    }
    // The row's key is its conversation id, so the id never changes.
    const id = first.key;
    const conversation = () => (row() as ConversationRow).conversation;
    const status = () => effectiveConversationStatus(conversation());
    const provider = () => (conversation().provider ? providerNames[conversation().provider!] : "No provider");
    const found = first.kind === "conversation" && first.found === true;
    const expanded = () => !found && props.expandedById[id] === true;
    const page = () => props.childPagesById[id];
    const expandable = () => !found && (conversation().hasChildren || page() !== undefined);
    const meta = () => `${found ? `${conversation().projectRoot ? folderName(conversation().projectRoot!) : "no folder"} · ` : ""}${provider()} · ${statusText[status()]}`;
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
        status={statusDots[status()]}
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
          <p class="work-empty" role="status">{resultCount() === 0 ? "No PRs or chats match" : `${resultCount()} ${resultCount() === 1 ? "result" : "results"}`} · Esc clears</p>
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
                    <Show when={section().count === 0}><p class="work-empty">Nothing needs you right now.</p></Show>
                    <For each={section().parts} keyed={(part) => part.key}>
                      {(part) => {
                        const partId = createUniqueId();
                        return (
                          <div class="work-part">
                            <Show when={part().label}>
                              {(label) => <h3 class="work-part-heading" title={part().hint}><span id={partId}>{label()}</span> · {part().entries.length}</h3>}
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
