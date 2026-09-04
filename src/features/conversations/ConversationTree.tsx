import { useEffect, useMemo, useRef, useState } from "react";
import type {
  AgentSnapshot,
  AgentStatus,
  ConversationSummary,
  ProviderId,
  RollupStatus,
  RunStatus,
} from "../../bridge/types";
import {
  effectiveConversationStatus,
  type AgentWindowSnapshot,
  type StatusFilter,
} from "../../app/store";

type ConversationTreeProps = {
  conversations: readonly ConversationSummary[];
  selectedId: string | null;
  selectedAgentId: string | null;
  statusFilter: StatusFilter;
  agentWindow?: AgentWindowSnapshot | null;
  onLoadAgentPage?(conversationId: string, restart: boolean): void;
  onSelect(conversationId: string, agentId?: string): void;
};

type TreeGroup = {
  key: string;
  label: string;
  path: string | null;
  conversations: ConversationSummary[];
};

type VisibleItem = {
  key: string;
  parentKey: string | null;
};

const providerLabels: Record<ProviderId, string> = {
  codex: "Codex",
  claude: "Claude",
};

const agentStatusLabels: Record<AgentStatus, string> = {
  queued: "Queued",
  running: "Running",
  waiting: "Waiting",
  completed: "Completed",
  interrupted: "Interrupted",
  failed: "Failed",
};

const runStatusLabels: Record<RunStatus, string> = agentStatusLabels;

const rollupStatusLabels: Record<RollupStatus, string> = {
  needsAttention: "Needs attention",
  active: "Active",
  failed: "Failed",
  interrupted: "Interrupted",
  completed: "Completed",
};
const MAX_MOUNTED_AGENT_ROWS = 80;

export function ConversationTree({
  conversations,
  selectedId,
  selectedAgentId,
  statusFilter,
  agentWindow = null,
  onLoadAgentPage = () => {},
  onSelect,
}: ConversationTreeProps) {
  const groups = useMemo(
    () => groupConversations(
      conversations.filter((conversation) => (
        !conversation.archived
        && (statusFilter === "all" || effectiveConversationStatus(conversation) === statusFilter)
      )),
    ),
    [conversations, statusFilter],
  );
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const itemRefs = useRef(new Map<string, HTMLElement>());
  const visibleItems = useMemo(
    () => flattenVisibleItems(groups, expanded),
    [groups, expanded],
  );
  const activeFocusKey = visibleItems.some(({ key }) => key === focusedKey)
    ? focusedKey
    : visibleItems[0]?.key ?? null;

  useEffect(() => {
    const valid = new Set<string>();
    conversations.forEach((conversation) => {
      valid.add(conversationKey(conversation.id));
      conversation.agents.forEach((agent) => valid.add(agentKey(agent.id)));
    });
    setExpanded((current) => {
      const retained = new Set([...current].filter((key) => valid.has(key)));
      return retained.size === current.size ? current : retained;
    });
  }, [conversations]);

  function setItemRef(key: string, element: HTMLElement | null) {
    if (element) itemRefs.current.set(key, element);
    else itemRefs.current.delete(key);
  }

  function focusItem(key: string | undefined) {
    if (!key) return;
    setFocusedKey(key);
    itemRefs.current.get(key)?.focus();
  }

  function toggleAgent(key: string) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  }

  function toggleConversation(conversation: ConversationSummary) {
    const key = conversationKey(conversation.id);
    const opens = !expanded.has(key);
    setExpanded((current) => {
      if (!opens) {
        const next = new Set(current);
        next.delete(key);
        return next;
      }
      const next = new Set([...current].filter((item) => !item.startsWith("conversation:")));
      next.add(key);
      return next;
    });
    if (
      opens
      && conversation.agentsTruncated
      && (agentWindow?.conversationId !== conversation.id
        || agentWindow.runId !== conversation.currentRunId)
    ) {
      onLoadAgentPage(conversation.id, true);
    }
  }

  function handleKeyDown(
    event: React.KeyboardEvent<HTMLElement>,
    item: VisibleItem,
    expandable: boolean,
    toggleItem: () => void,
  ) {
    const index = visibleItems.findIndex(({ key }) => key === item.key);
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        focusItem(visibleItems[index + 1]?.key);
        break;
      case "ArrowUp":
        event.preventDefault();
        focusItem(visibleItems[index - 1]?.key);
        break;
      case "ArrowRight":
        event.preventDefault();
        if (expandable && !expanded.has(item.key)) toggleItem();
        else if (expandable) focusItem(visibleItems[index + 1]?.key);
        break;
      case "ArrowLeft":
        event.preventDefault();
        if (expandable && expanded.has(item.key)) toggleItem();
        else focusItem(item.parentKey ?? undefined);
        break;
      case "Home":
        event.preventDefault();
        focusItem(visibleItems[0]?.key);
        break;
      case "End":
        event.preventDefault();
        focusItem(visibleItems.at(-1)?.key);
        break;
    }
  }

  if (groups.length === 0) {
    return <p className="tree-empty">No conversations match this status.</p>;
  }

  return (
    <div className="conversation-tree">
      {groups.map((group, groupIndex) => {
        const headingId = `conversation-group-${groupIndex}`;
        return (
        <section className="tree-group" key={group.key}>
          <h2 id={headingId} className="tree-group-heading" title={group.path ?? undefined}>{group.label}</h2>
          <ul role="tree" aria-labelledby={headingId} className="tree-group-items">
            {group.conversations.map((conversation) => {
              const key = conversationKey(conversation.id);
              const isExpanded = expanded.has(key);
              const agents = buildVisibleAgentRows(conversation.agents, expanded, key);
              const expandable = conversation.agentsTruncated || agents.length > 0;
              const window = agentWindow?.conversationId === conversation.id
                && agentWindow.runId === conversation.currentRunId
                ? agentWindow
                : null;
              return (
                <ConversationItem
                  key={conversation.id}
                  conversation={conversation}
                  agents={agents}
                  selected={selectedId === conversation.id && selectedAgentId === null}
                  selectedAgentId={selectedId === conversation.id ? selectedAgentId : null}
                  expanded={expanded}
                  isExpanded={isExpanded}
                  expandable={expandable}
                  agentWindow={window}
                  activeFocusKey={activeFocusKey}
                  setItemRef={setItemRef}
                  onFocusItem={setFocusedKey}
                  onToggleAgent={toggleAgent}
                  onToggleConversation={() => toggleConversation(conversation)}
                  onLoadAgentPage={onLoadAgentPage}
                  onSelect={onSelect}
                  onKeyDown={handleKeyDown}
                />
              );
            })}
          </ul>
        </section>
        );
      })}
    </div>
  );
}

type AgentRow = {
  agent: AgentSnapshot;
  level: number;
  parentKey: string;
  expandable: boolean;
};

type SharedItemProps = {
  expanded: ReadonlySet<string>;
  activeFocusKey: string | null;
  setItemRef(key: string, element: HTMLElement | null): void;
  onFocusItem(key: string): void;
  onToggleAgent(key: string): void;
  onSelect(conversationId: string, agentId?: string): void;
  onKeyDown(
    event: React.KeyboardEvent<HTMLElement>,
    item: VisibleItem,
    expandable: boolean,
    toggleItem: () => void,
  ): void;
};

function ConversationItem({
  conversation,
  agents,
  selected,
  selectedAgentId,
  expanded,
  isExpanded,
  expandable,
  agentWindow,
  activeFocusKey,
  setItemRef,
  onFocusItem,
  onToggleAgent,
  onToggleConversation,
  onLoadAgentPage,
  onSelect,
  onKeyDown,
}: SharedItemProps & {
  conversation: ConversationSummary;
  agents: AgentRow[];
  selected: boolean;
  selectedAgentId: string | null;
  isExpanded: boolean;
  expandable: boolean;
  agentWindow: AgentWindowSnapshot | null;
  onToggleConversation(): void;
  onLoadAgentPage(conversationId: string, restart: boolean): void;
}) {
  const key = conversationKey(conversation.id);
  const status = conversationStatus(conversation);
  return (
    <li
      role="treeitem"
      aria-label={`${conversation.title}, ${providerName(conversation.provider)}, ${status.label}`}
      aria-level={1}
      aria-selected={selected}
      aria-expanded={expandable ? isExpanded : undefined}
      tabIndex={activeFocusKey === key ? 0 : -1}
      data-tree-key={key}
      className="tree-item conversation-item"
      ref={(element) => setItemRef(key, element)}
      onFocus={(event) => {
        if (event.target === event.currentTarget) onFocusItem(key);
      }}
      onClick={() => onSelect(conversation.id)}
      onKeyDown={(event) => {
        onKeyDown(event, { key, parentKey: null }, expandable, onToggleConversation);
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect(conversation.id);
        }
      }}
    >
      <TreeRow
        label={conversation.title}
        provider={conversation.provider}
        status={status}
        expandable={expandable}
        expanded={isExpanded}
        onToggle={onToggleConversation}
      />
      {expandable && isExpanded ? (
        <>
          <ul role="group" className="agent-group">
          {agents.map((row) => (
            <AgentItem
              key={row.agent.id}
              row={row}
              selectedAgentId={selectedAgentId}
              conversationId={conversation.id}
              expanded={expanded}
              activeFocusKey={activeFocusKey}
              setItemRef={setItemRef}
              onFocusItem={onFocusItem}
              onToggleAgent={onToggleAgent}
              onSelect={onSelect}
              onKeyDown={onKeyDown}
            />
          ))}
          </ul>
          {agentWindow?.error ? <p role="alert">{agentWindow.error}</p> : null}
          {agentWindow?.error && agentWindow.pages.length === 0 ? (
            <button
              type="button"
              className="secondary-button"
              onClick={(event) => {
                event.stopPropagation();
                onLoadAgentPage(conversation.id, true);
              }}
            >
              Retry agents
            </button>
          ) : null}
          {agentWindow?.loading ? <p role="status">Loading agents…</p> : null}
          {agentWindow?.nextCursor ? (
            <button
              type="button"
              className="secondary-button"
              disabled={agentWindow.loading}
              onClick={(event) => {
                event.stopPropagation();
                onLoadAgentPage(conversation.id, false);
              }}
            >
              Load more agents
            </button>
          ) : null}
          {agentWindow?.evicted ? (
            <div role="note" className="history-window-note">
              <span>Some agents are outside this bounded view.</span>
              <button
                type="button"
                className="secondary-button"
                onClick={(event) => {
                  event.stopPropagation();
                  onLoadAgentPage(conversation.id, true);
                }}
              >
                Reload first agents
              </button>
            </div>
          ) : null}
        </>
      ) : null}
    </li>
  );
}

function AgentItem({
  row,
  selectedAgentId,
  conversationId,
  expanded,
  activeFocusKey,
  setItemRef,
  onFocusItem,
  onToggleAgent,
  onSelect,
  onKeyDown,
}: SharedItemProps & {
  row: AgentRow;
  selectedAgentId: string | null;
  conversationId: string;
}) {
  const key = agentKey(row.agent.id);
  const isExpanded = expanded.has(key);
  const status = { label: agentStatusLabels[row.agent.status], key: row.agent.status };
  return (
    <li
      role="treeitem"
      aria-label={`${row.agent.label}, ${providerName(row.agent.provider)}, ${status.label}`}
      aria-level={row.level}
      aria-selected={selectedAgentId === row.agent.id}
      aria-expanded={row.expandable ? isExpanded : undefined}
      tabIndex={activeFocusKey === key ? 0 : -1}
      data-tree-key={key}
      className="tree-item agent-item"
      ref={(element) => setItemRef(key, element)}
      onFocus={(event) => {
        if (event.target === event.currentTarget) onFocusItem(key);
      }}
      onClick={(event) => {
        event.stopPropagation();
        onSelect(conversationId, row.agent.id);
      }}
      onKeyDown={(event) => {
        event.stopPropagation();
        onKeyDown(
          event,
          { key, parentKey: row.parentKey },
          row.expandable,
          () => onToggleAgent(key),
        );
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect(conversationId, row.agent.id);
        }
      }}
    >
      <TreeRow
        label={row.agent.label}
        provider={row.agent.provider}
        status={status}
        expandable={row.expandable}
        expanded={isExpanded}
        onToggle={() => onToggleAgent(key)}
      />
    </li>
  );
}

function TreeRow({
  label,
  provider,
  status,
  expandable,
  expanded,
  onToggle,
}: {
  label: string;
  provider: ProviderId | null;
  status: { label: string; key: string };
  expandable: boolean;
  expanded: boolean;
  onToggle(): void;
}) {
  return (
    <div className="tree-row">
      {expandable ? (
        <button
          type="button"
          className="disclosure-button"
          aria-label={`${expanded ? "Collapse" : "Expand"} ${label}`}
          tabIndex={-1}
          onClick={(event) => {
            event.stopPropagation();
            onToggle();
          }}
        >
          {expanded ? "▾" : "▸"}
        </button>
      ) : <span className="disclosure-spacer" aria-hidden="true" />}
      <span className="tree-label">{label}</span>
      {provider ? <span className="provider-badge">{providerLabels[provider]}</span> : null}
      <span className="status-badge" data-status={status.key}>{status.label}</span>
    </div>
  );
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

function buildVisibleAgentRows(
  agents: readonly AgentSnapshot[],
  expanded: ReadonlySet<string>,
  conversationParentKey: string,
): AgentRow[] {
  const roots = new Set(agents.filter(({ parentId }) => parentId === null).map(({ id }) => id));
  const visibleAgents = agents.filter(({ id }) => !roots.has(id));
  const visibleIds = new Set(visibleAgents.map(({ id }) => id));
  const children = new Map<string, AgentSnapshot[]>();
  const topLevel: AgentSnapshot[] = [];
  visibleAgents.forEach((agent) => {
    if (agent.parentId && visibleIds.has(agent.parentId)) {
      const siblings = children.get(agent.parentId) ?? [];
      siblings.push(agent);
      children.set(agent.parentId, siblings);
    } else {
      topLevel.push(agent);
    }
  });
  const rows: AgentRow[] = [];
  const visited = new Set<string>();
  const stack: Array<{ agent: AgentSnapshot; level: number; parentKey: string }> = [];
  for (let index = topLevel.length - 1; index >= 0; index -= 1) {
    stack.push({ agent: topLevel[index]!, level: 2, parentKey: conversationParentKey });
  }
  while (stack.length > 0) {
    if (rows.length >= MAX_MOUNTED_AGENT_ROWS) break;
    const current = stack.pop()!;
    if (visited.has(current.agent.id)) continue;
    visited.add(current.agent.id);
    const descendants = children.get(current.agent.id) ?? [];
    rows.push({
      agent: current.agent,
      level: current.level,
      parentKey: current.parentKey,
      expandable: descendants.length > 0,
    });
    if (!expanded.has(agentKey(current.agent.id))) continue;
    for (let index = descendants.length - 1; index >= 0; index -= 1) {
      stack.push({
        agent: descendants[index]!,
        level: current.level + 1,
        parentKey: agentKey(current.agent.id),
      });
    }
  }
  return rows;
}

function flattenVisibleItems(
  groups: readonly TreeGroup[],
  expanded: ReadonlySet<string>,
): VisibleItem[] {
  const items: VisibleItem[] = [];
  groups.forEach(({ conversations }) => conversations.forEach((conversation) => {
    const key = conversationKey(conversation.id);
    items.push({ key, parentKey: null });
    if (expanded.has(key)) {
      buildVisibleAgentRows(conversation.agents, expanded, key).forEach((row) => {
        items.push({ key: agentKey(row.agent.id), parentKey: row.parentKey });
      });
    }
  }));
  return items;
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

function conversationKey(id: string) {
  return `conversation:${id}`;
}

function agentKey(id: string) {
  return `agent:${id}`;
}

function providerName(provider: ProviderId | null): string {
  return provider ? providerLabels[provider] : "No provider";
}
