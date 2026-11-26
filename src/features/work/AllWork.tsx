import { createMemo, createSignal, For, Show, untrack } from "solid-js";
import { AlertIcon, ChatIcon, ChevronDownIcon, ChevronRightIcon, ListRow, PullRequestIcon, type Status } from "@prompting-time/ui";
import type { ConversationSummary, ProviderId } from "../../bridge/types";
import { effectiveConversationStatus, type ChildPageSnapshot, type EffectiveStatus, type SelectedPath } from "../../app/store";
import { causeKey, isFinished, reviewKey, reviewPlace, reviewState, reviewTitle, shortPullRequest, waitingText, type BlockingCause, type PluginReview } from "../agents/reviews";

type Props = {
  /** Active roots in list order, already filtered by status. */
  conversations: readonly ConversationSummary[];
  conversationsById: Readonly<Record<string, ConversationSummary>>;
  childPagesById: Readonly<Record<string, ChildPageSnapshot>>;
  expandedById: Readonly<Record<string, boolean>>;
  selectedId: string | null;
  selectedPath: SelectedPath | null;
  /**
   * The plugins' reviews, the newest run of each pull request. A review waiting on a decision sits
   * under "Needs you", one the user finished with before today in Done, and the rest in their
   * repository's group.
   */
  reviews: readonly PluginReview[];
  /** The causes that block every review, one row each, first under "Needs you". */
  causes: readonly PluginCause[];
  /** The key (reviewKey or causeKey) of the review or cause the workspace shows, if any. */
  selectedReview: string | null;
  onSelect(id: string): void;
  onOpenReview(plugin: string, runId: string): void;
  onOpenCause(plugin: string, cause: string): void;
  onToggle(id: string): void;
  onLoadChildPage(id: string, restart: boolean): void;
};
export type WorkGroup = { key: string; label: string; path: string | null; conversations: ConversationSummary[] };
/** A cause that blocks every review, and the plugin that reports it. */
export type PluginCause = { plugin: string; cause: BlockingCause };
type ConversationRow = { kind: "conversation"; key: string; conversation: ConversationSummary; depth: number };
type ReviewRow = { kind: "review"; key: string; item: PluginReview; depth: 0 };
type CauseRow = { kind: "cause"; key: string; item: PluginCause; depth: 0 };
type Row = ConversationRow | ReviewRow | CauseRow;
/** A group as the list draws it; Done folds to its heading. */
type DisplayGroup = WorkGroup & { rows: Row[]; done?: boolean };

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
 * The All work list: the causes that block every review, the reviews that wait on a decision, and
 * the roots waiting on you under "Needs you"; then the rest grouped by project, each root followed
 * by its loaded children and its repository's other reviews; then Done, folded, with the reviews
 * finished before today. It is a tree for keyboard users: arrow keys move and open rows, Home and
 * End jump, and Enter or Space opens a conversation, a review, or a cause.
 */
export function AllWork(props: Props) {
  const [focusedId, setFocusedId] = createSignal<string | null>(null);
  const [doneOpen, setDoneOpen] = createSignal(false);
  const items = new Map<string, HTMLElement>();
  const groups = createMemo(() => {
    const now = new Date();
    const conversationGroups: DisplayGroup[] = groupWork(props.conversations).map((group) => ({
      ...group,
      rows: visibleRows(group.conversations, props.conversationsById, props.childPagesById, props.expandedById, props.selectedPath) as Row[],
    }));
    const needsYou: Row[] = props.causes.map((item) => ({ kind: "cause", key: causeKey(item.plugin, item.cause.id), item, depth: 0 }));
    const inProjects: ReviewRow[] = [];
    const done: ReviewRow[] = [];
    for (const item of props.reviews) {
      const row: ReviewRow = { kind: "review", key: reviewKey(item.plugin, item.review.run_id), item, depth: 0 };
      const place = reviewPlace(item.review, now);
      (place === "needs-you" ? needsYou : place === "done" ? done : inProjects).push(row);
    }
    return placeReviews(conversationGroups, needsYou, inProjects, done);
  });
  const rows = createMemo(() => groups().flatMap((group) => (group.done && !doneOpen() ? [] : group.rows)));
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

  return (
    <Show when={groups().length > 0} fallback={<p class="work-empty">No conversations match this status.</p>}>
      <div class="all-work">
        <For each={groups()} keyed={(group) => group.key}>
          {(group, index) => (
            <section class={["work-group", { "work-group-needs-you": group().key === "needs-you" }]}>
              <h2 id={`work-group-${index()}`} class="work-group-heading" title={group().path ?? undefined}>
                <Show when={group().done} fallback={group().label}>
                  <button type="button" class="work-group-toggle" aria-expanded={doneOpen() ? "true" : "false"} onClick={() => setDoneOpen(!doneOpen())}>
                    {doneOpen() ? <ChevronDownIcon /> : <ChevronRightIcon />}
                    <span>{group().label}</span>{" "}
                    <span class="work-group-count">{group().rows.length}</span>
                  </button>
                </Show>
              </h2>
              <Show when={!group().done || doneOpen()}>
              <ul role="tree" aria-labelledby={`work-group-${index()}`} class="work-rows">
                <For each={group().rows} keyed={(row) => row.key}>
                  {(row) => {
                    const first = untrack(row);
                    if (first.kind === "cause") {
                      // A cause's key holds its plugin and ID, so neither changes.
                      const item = () => (row() as CauseRow).item;
                      const meta = () => waitingText(item().cause.reviews.length);
                      return (
                        <ListRow
                          element="li"
                          role="treeitem"
                          ref={(element: HTMLElement) => items.set(first.key, element)}
                          aria-label={`${item().cause.summary}, ${meta()}`}
                          aria-level={1}
                          aria-selected={props.selectedReview === first.key ? "true" : "false"}
                          tabindex={tabStop() === first.key ? 0 : -1}
                          class="work-row"
                          style={{ "--work-depth": "0" }}
                          icon={<AlertIcon />}
                          title={item().cause.summary}
                          status="error"
                          meta={meta()}
                          onFocus={(event: FocusEvent) => {
                            if (event.target === event.currentTarget) setFocusedId(first.key);
                          }}
                          onClick={() => open(row() as CauseRow)}
                          onKeyDown={(event: KeyboardEvent) => {
                            if (event.target === event.currentTarget) navigate(event, row());
                          }}
                        />
                      );
                    }
                    if (first.kind === "review") {
                      // A review's key holds its plugin and run, so neither changes.
                      const item = () => (row() as ReviewRow).item;
                      const state = () => reviewState(item().review);
                      const meta = () => `${shortPullRequest(item().review)} · ${state().words}`;
                      // Findings count while the review waits on a decision; after it, they are history.
                      const count = () => (item().review.outcome === "reviewed" && !isFinished(item().review) && item().review.findings > 0 ? item().review.findings : undefined);
                      return (
                        <ListRow
                          element="li"
                          role="treeitem"
                          ref={(element: HTMLElement) => items.set(first.key, element)}
                          aria-label={`${reviewTitle(item().review)}, ${count() === undefined ? "" : `${count()} ${count() === 1 ? "finding" : "findings"}, `}${meta()}`}
                          aria-level={1}
                          aria-selected={props.selectedReview === first.key ? "true" : "false"}
                          tabindex={tabStop() === first.key ? 0 : -1}
                          class="work-row"
                          style={{ "--work-depth": "0" }}
                          icon={<PullRequestIcon />}
                          title={reviewTitle(item().review)}
                          count={count()}
                          status={state().status}
                          meta={meta()}
                          onFocus={(event: FocusEvent) => {
                            if (event.target === event.currentTarget) setFocusedId(first.key);
                          }}
                          onClick={() => props.onOpenReview(item().plugin, item().review.run_id)}
                          onKeyDown={(event: KeyboardEvent) => {
                            if (event.target === event.currentTarget) navigate(event, row());
                          }}
                        />
                      );
                    }
                    // The row's key is its conversation id, so the id never changes.
                    const id = first.key;
                    const conversation = () => (row() as ConversationRow).conversation;
                    const status = () => effectiveConversationStatus(conversation());
                    const provider = () => (conversation().provider ? providerNames[conversation().provider!] : "No provider");
                    const expanded = () => props.expandedById[id] === true;
                    const page = () => props.childPagesById[id];
                    const expandable = () => conversation().hasChildren || page() !== undefined;
                    return (
                      <ListRow
                        element="li"
                        role="treeitem"
                        ref={(element: HTMLElement) => items.set(id, element)}
                        aria-label={`${conversation().title}, ${provider()}, ${statusText[status()]}`}
                        aria-level={row().depth + 1}
                        aria-selected={props.selectedId === id ? "true" : "false"}
                        aria-expanded={expandable() ? (expanded() ? "true" : "false") : undefined}
                        tabindex={tabStop() === id ? 0 : -1}
                        class="work-row"
                        style={{ "--work-depth": String(Math.min(row().depth, 8)) }}
                        icon={<ChatIcon />}
                        title={conversation().title}
                        status={statusDots[status()]}
                        meta={`${provider()} · ${statusText[status()]}`}
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
                        onFocus={(event: FocusEvent) => {
                          if (event.target === event.currentTarget) setFocusedId(id);
                        }}
                        onClick={() => props.onSelect(id)}
                        onKeyDown={(event: KeyboardEvent) => {
                          if (event.target === event.currentTarget) navigate(event, row());
                        }}
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
                  }}
                </For>
              </ul>
              </Show>
            </section>
          )}
        </For>
      </div>
    </Show>
  );
}

/**
 * Adds the reviews to the conversation groups: `needsYou` first under "Needs you", each review of
 * `inProjects` at the end of the project whose folder has its repository's name, such as widgets
 * for acme/widgets, else in a group named after the repository among the projects, and `done` in a
 * Done group at the end. Matching by folder name can put two repositories with the same name in
 * one project; reading each project's remote would tell them apart.
 */
function placeReviews(groups: DisplayGroup[], needsYou: Row[], inProjects: ReviewRow[], done: ReviewRow[]): DisplayGroup[] {
  const result = [...groups];
  if (needsYou.length > 0) {
    if (result[0]?.key === "needs-you") result[0] = { ...result[0], rows: [...needsYou, ...result[0].rows] };
    else result.unshift({ key: "needs-you", label: "Needs you", path: null, conversations: [], rows: needsYou });
  }
  for (const row of inProjects) {
    const repo = row.item.review.repo;
    const name = projectName(repo);
    let index = result.findIndex((group) => group.key.startsWith("project:") && group.path !== null && projectName(group.path) === name);
    if (index < 0) index = result.findIndex((group) => group.key === `repo:${repo}`);
    if (index >= 0) {
      result[index] = { ...result[index]!, rows: [...result[index]!.rows, row] };
      continue;
    }
    const label = result.some((group) => group.label === name) ? repo : name;
    const before = result.findIndex((group) => group.key === "projectless" || ((group.key.startsWith("project:") || group.key.startsWith("repo:")) && group.label.localeCompare(label) > 0));
    result.splice(before < 0 ? result.length : before, 0, { key: `repo:${repo}`, label, path: null, conversations: [], rows: [row] });
  }
  if (done.length > 0) result.push({ key: "done", label: "Done", path: null, conversations: [], rows: done, done: true });
  return result;
}

/** The roots waiting on you, in list order: what ⌘J steps through. */
export function waitingRoots(conversations: readonly ConversationSummary[]): ConversationSummary[] {
  return conversations.filter((conversation) => !conversation.archived && effectiveConversationStatus(conversation) === "waiting");
}

/**
 * "Needs you" first, holding every root waiting on you (a root also waits when a descendant does),
 * then one group per project, sorted by name, then conversations without a project. A root shows
 * in one group only.
 */
export function groupWork(conversations: readonly ConversationSummary[]): WorkGroup[] {
  const active = conversations.filter((conversation) => !conversation.archived);
  const needsYou: WorkGroup = { key: "needs-you", label: "Needs you", path: null, conversations: waitingRoots(active) };
  const waiting = new Set(needsYou.conversations);
  const projects = new Map<string, WorkGroup>();
  const projectless: WorkGroup = { key: "projectless", label: "No project", path: null, conversations: [] };
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
