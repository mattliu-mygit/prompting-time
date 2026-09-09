import { useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import type { ConversationSummary, ProviderId, RollupStatus, RunStatus } from "../../bridge/types";
import type { ChildPageSnapshot, SelectedPath } from "../../app/store";

type Props = {
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
type TreeGroup = { key: string; label: string; path: string | null; conversations: ConversationSummary[] };
type Row = { conversation: ConversationSummary; depth: number };
const providerLabels: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
const runStatusLabels: Record<RunStatus, string> = { queued: "Queued", running: "Running", waiting: "Waiting",
  completed: "Completed", interrupted: "Interrupted", failed: "Failed" };
const rollupStatusLabels: Record<RollupStatus, string> = { needsAttention: "Needs attention",
  active: "Active", failed: "Failed", interrupted: "Interrupted", completed: "Completed" };

export function ConversationTree({ conversations, conversationsById, childPagesById, expandedById,
  selectedId, selectedPath, onSelect, onToggle, onLoadChildPage }: Props) {
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const refs = useRef(new Map<string, HTMLLIElement>());
  const groups = useMemo(() => groupConversations(conversations.filter(item => !item.archived))
    .map(group => ({ ...group, rows: visibleRows(group.conversations, conversationsById, childPagesById, expandedById, selectedPath) })),
    [conversations, conversationsById, childPagesById, expandedById, selectedPath]);
  const rows = groups.flatMap(group => group.rows);
  const focusId = rows.some(row => row.conversation.id === focusedId) ? focusedId : rows[0]?.conversation.id;
  function focus(id: string | null | undefined) {
    if (!id) return;
    setFocusedId(id);
    refs.current.get(id)?.focus();
  }
  function navigate(event: KeyboardEvent, row: Row) {
    const id = row.conversation.id;
    const index = rows.findIndex(item => item.conversation.id === id);
    switch (event.key) {
      case "ArrowDown": event.preventDefault(); focus(rows[index + 1]?.conversation.id); break;
      case "ArrowUp": event.preventDefault(); focus(rows[index - 1]?.conversation.id); break;
      case "Home": event.preventDefault(); focus(rows[0]?.conversation.id); break;
      case "End": event.preventDefault(); focus(rows.at(-1)?.conversation.id); break;
      case "ArrowRight":
        event.preventDefault();
        if (row.conversation.hasChildren && !expandedById[id]) onToggle(id);
        else if (expandedById[id] && rows[index + 1]?.depth === row.depth + 1) focus(rows[index + 1]?.conversation.id);
        break;
      case "ArrowLeft":
        event.preventDefault();
        if (expandedById[id]) onToggle(id); else focus(row.conversation.parentId);
        break;
      case "Enter": case " ": event.preventDefault(); onSelect(id); break;
    }
  }
  if (!groups.length) return <p className="tree-empty">No conversations match this status.</p>;
  return <div className="conversation-tree">
    {groups.map((group, index) => <section className="tree-group" key={group.key}>
      <h2 id={`conversation-group-${index}`} className="tree-group-heading" title={group.path ?? undefined}>{group.label}</h2>
      <ul role="tree" aria-labelledby={`conversation-group-${index}`} className="tree-group-items">
        {group.rows.map(row => {
          const conversation = row.conversation;
          const id = conversation.id;
          const status = conversationStatus(conversation);
          const provider = conversation.provider ? providerLabels[conversation.provider] : "No provider";
          const expanded = expandedById[id] === true;
          const page = childPagesById[id];
          return <li key={id} role="treeitem" aria-label={`${conversation.title}, ${provider}, ${status.label}`}
            aria-level={row.depth + 1} aria-selected={selectedId === id}
            aria-expanded={conversation.hasChildren || page ? expanded : undefined}
            tabIndex={focusId === id ? 0 : -1} className="tree-item conversation-item"
            style={{ "--conversation-depth": Math.min(row.depth, 8) } as CSSProperties}
            ref={element => { if (element) refs.current.set(id, element); else refs.current.delete(id); }}
            onFocus={event => { if (event.target === event.currentTarget) setFocusedId(id); }}
            onClick={() => onSelect(id)} onKeyDown={event => { if (event.target === event.currentTarget) navigate(event, row); }}>
            <div className="tree-row">
              {conversation.hasChildren || page ? <button type="button" className="disclosure-button" tabIndex={-1}
                aria-label={`${expanded ? "Collapse" : "Expand"} ${conversation.title}`}
                onClick={event => { event.stopPropagation(); onToggle(id); }}>{expanded ? "▾" : "▸"}</button>
                : <span className="disclosure-spacer" aria-hidden="true" />}
              <span className="tree-label" title={conversation.title}>{conversation.title}</span>
              <span className="tree-metadata">{conversation.provider ? <span className="provider-badge">{provider}</span> : null}
                <span className="status-badge" data-status={status.key}>{status.label}</span></span>
            </div>
            {expanded && page ? <div className="child-page-controls" onClick={event => event.stopPropagation()}>
              {page.loading ? <span role="status">Loading conversations…</span> : null}
              {page.error ? <><p role="alert">{page.error}</p><button type="button" className="secondary-button"
                onClick={() => onLoadChildPage(id, true)}>Retry conversations</button></> : null}
              {page.nextCursor ? <button type="button" className="secondary-button" disabled={page.loading}
                onClick={() => onLoadChildPage(id, false)}>Load more conversations</button> : null}
              {page.evicted ? <div role="note"><span>Some conversations are outside this bounded view.</span>
                <button type="button" className="secondary-button" disabled={page.loading}
                  onClick={() => onLoadChildPage(id, true)}>Reload conversations</button></div> : null}
            </div> : null}
          </li>;
        })}
      </ul>
    </section>)}
  </div>;
}

function visibleRows(roots: readonly ConversationSummary[], nodes: Props["conversationsById"],
  pages: Props["childPagesById"], expanded: Props["expandedById"], selectedPath: SelectedPath | null): Row[] {
  const rows: Row[] = [];
  const stack = [...roots].reverse().map(conversation => ({ conversation, depth: 0 }));
  const seen = new Set<string>();
  while (stack.length) {
    const row = stack.pop()!;
    const id = row.conversation.id;
    if (seen.has(id)) continue;
    seen.add(id);
    rows.push(row);
    if (!expanded[id]) continue;
    const ids = new Set([...(pages[id]?.pages.flatMap(page => page.ids) ?? []),
      ...(selectedPath?.ids.filter(childId => nodes[childId]?.parentId === id) ?? [])]);
    for (const childId of [...ids].reverse()) {
      const child = nodes[childId];
      if (child && child.parentId === id && !child.archived) stack.push({ conversation: child, depth: row.depth + 1 });
    }
  }
  return rows;
}

function groupConversations(conversations: readonly ConversationSummary[]): TreeGroup[] {
  const projects = new Map<string, TreeGroup>();
  const projectless: TreeGroup = {
    key: "projectless",
    label: "No project",
    path: null,
    conversations: [],
  };
  conversations.forEach((conversation) => {
    if (!conversation.projectRoot) {
      projectless.conversations.push(conversation);
      return;
    }
    let group = projects.get(conversation.projectRoot);
    if (!group) {
      group = {
        key: `project:${conversation.projectRoot}`,
        label: projectName(conversation.projectRoot),
        path: conversation.projectRoot,
        conversations: [],
      };
      projects.set(conversation.projectRoot, group);
    }
    group.conversations.push(conversation);
  });
  const groups = [...projects.values()].sort((left, right) => (
    left.label.localeCompare(right.label)
    || (left.path ?? "").localeCompare(right.path ?? "")
  ));
  const labelCounts = new Map<string, number>();
  groups.forEach(({ label }) => labelCounts.set(label, (labelCounts.get(label) ?? 0) + 1));
  groups.forEach((group) => {
    if ((labelCounts.get(group.label) ?? 0) > 1 && group.path) {
      group.label = `${group.label} — ${group.path}`;
    }
  });
  if (projectless.conversations.length > 0) groups.push(projectless);
  return groups;
}

function conversationStatus(conversation: ConversationSummary): { label: string; key: string } {
  if (conversation.runStatus === "queued") {
    return { label: runStatusLabels.queued, key: "queued" };
  }
  if (conversation.rollupStatus) {
    return {
      label: rollupStatusLabels[conversation.rollupStatus],
      key: conversation.rollupStatus,
    };
  }
  if (conversation.runStatus) {
    return { label: runStatusLabels[conversation.runStatus], key: conversation.runStatus };
  }
  return { label: "Ready", key: "idle" };
}

function projectName(path: string): string {
  const segments = path.split(/[\\/]/).filter(Boolean);
  return segments.at(-1) ?? path;
}
