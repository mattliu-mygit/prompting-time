import { createMemo, createSignal, For, Show, untrack } from "solid-js";
import { ChatIcon, ChevronDownIcon, ChevronRightIcon, ListRow, type Status } from "@prompting-time/ui";
import type { ConversationSummary, ProviderId } from "../../bridge/types";
import { effectiveConversationStatus, type ChildPageSnapshot, type EffectiveStatus, type SelectedPath } from "../../app/store";

type Props = {
  /** Active roots in list order, already filtered by status. */
  conversations: readonly ConversationSummary[];
  conversationsById: Readonly<Record<string, ConversationSummary>>;
  childPagesById: Readonly<Record<string, ChildPageSnapshot>>;
  expandedById: Readonly<Record<string, boolean>>;
  selectedId: string | null;
  selectedPath: SelectedPath | null;
  onSelect(id: string): void;
  onToggle(id: string): void;
  onLoadChildPage(id: string, restart: boolean): void;
};
export type WorkGroup = { key: string; label: string; path: string | null; conversations: ConversationSummary[] };
type Row = { conversation: ConversationSummary; depth: number };

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
 * The All work list: roots waiting on you under "Needs you", then the rest grouped by project, each
 * root followed by its loaded children. It is a tree for keyboard users: arrow keys move and open
 * rows, Home and End jump, and Enter or Space opens a conversation.
 */
export function AllWork(props: Props) {
  const [focusedId, setFocusedId] = createSignal<string | null>(null);
  const items = new Map<string, HTMLElement>();
  const groups = createMemo(() => groupWork(props.conversations).map((group) => ({
    ...group,
    rows: visibleRows(group.conversations, props.conversationsById, props.childPagesById, props.expandedById, props.selectedPath),
  })));
  const rows = createMemo(() => groups().flatMap((group) => group.rows));
  const tabStop = () => (rows().some((row) => row.conversation.id === focusedId()) ? focusedId() : rows()[0]?.conversation.id);

  function focus(id: string | null | undefined) {
    if (!id) return;
    setFocusedId(id);
    items.get(id)?.focus();
  }

  function navigate(event: KeyboardEvent, row: Row) {
    const id = row.conversation.id;
    const all = rows();
    const index = all.findIndex((item) => item.conversation.id === id);
    const expanded = props.expandedById[id] === true;
    switch (event.key) {
      case "ArrowDown": event.preventDefault(); focus(all[index + 1]?.conversation.id); break;
      case "ArrowUp": event.preventDefault(); focus(all[index - 1]?.conversation.id); break;
      case "Home": event.preventDefault(); focus(all[0]?.conversation.id); break;
      case "End": event.preventDefault(); focus(all.at(-1)?.conversation.id); break;
      case "ArrowRight":
        event.preventDefault();
        if (row.conversation.hasChildren && !expanded) props.onToggle(id);
        else if (expanded && all[index + 1]?.depth === row.depth + 1) focus(all[index + 1]?.conversation.id);
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
              <h2 id={`work-group-${index()}`} class="work-group-heading" title={group().path ?? undefined}>{group().label}</h2>
              <ul role="tree" aria-labelledby={`work-group-${index()}`} class="work-rows">
                <For each={group().rows} keyed={(row) => row.conversation.id}>
                  {(row) => {
                    // The row's key is its conversation id, so the id never changes.
                    const id = untrack(() => row().conversation.id);
                    const conversation = () => row().conversation;
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
            </section>
          )}
        </For>
      </div>
    </Show>
  );
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
  pages: Props["childPagesById"], expanded: Props["expandedById"], selectedPath: SelectedPath | null): Row[] {
  const rows: Row[] = [];
  const stack = [...roots].reverse().map((conversation) => ({ conversation, depth: 0 }));
  const seen = new Set<string>();
  while (stack.length) {
    const row = stack.pop()!;
    const id = row.conversation.id;
    if (seen.has(id)) continue;
    seen.add(id);
    rows.push(row);
    if (!expanded[id]) continue;
    const ids = new Set([...(pages[id]?.pages.flatMap((page) => page.ids) ?? []),
      ...(selectedPath?.ids.filter((childId) => nodes[childId]?.parentId === id) ?? [])]);
    for (const childId of [...ids].reverse()) {
      const child = nodes[childId];
      if (child && child.parentId === id && !child.archived) stack.push({ conversation: child, depth: row.depth + 1 });
    }
  }
  return rows;
}

function projectName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}
