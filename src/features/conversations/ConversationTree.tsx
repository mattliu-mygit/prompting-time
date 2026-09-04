import { useMemo, useRef, useState } from "react";
import type {
  AgentSnapshot,
  AgentStatus,
  ConversationSummary,
  ProviderId,
  RollupStatus,
  RunStatus,
} from "../../bridge/types";
import { effectiveConversationStatus, type StatusFilter } from "../../app/store";

type ConversationTreeProps = {
  conversations: readonly ConversationSummary[];
  selectedId: string | null;
  selectedAgentId: string | null;
  statusFilter: StatusFilter;
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

export function ConversationTree({
  conversations,
  selectedId,
  selectedAgentId,
  statusFilter,
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
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const itemRefs = useRef(new Map<string, HTMLElement>());
  const visibleItems = useMemo(
    () => flattenVisibleItems(groups, collapsed),
    [groups, collapsed],
  );
  const activeFocusKey = visibleItems.some(({ key }) => key === focusedKey)
    ? focusedKey
    : visibleItems[0]?.key ?? null;

  function setItemRef(key: string, element: HTMLElement | null) {
    if (element) itemRefs.current.set(key, element);
    else itemRefs.current.delete(key);
  }

  function focusItem(key: string | undefined) {
    if (!key) return;
    setFocusedKey(key);
    itemRefs.current.get(key)?.focus();
  }

  function toggle(key: string) {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  function handleKeyDown(
    event: React.KeyboardEvent<HTMLElement>,
    item: VisibleItem,
    expandable: boolean,
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
        if (expandable && collapsed.has(item.key)) toggle(item.key);
        else if (expandable) focusItem(visibleItems[index + 1]?.key);
        break;
      case "ArrowLeft":
        event.preventDefault();
        if (expandable && !collapsed.has(item.key)) toggle(item.key);
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
    <div className="conversation-tree" role="tree" aria-label="Conversations">
      {groups.map((group) => (
        <section className="tree-group" role="presentation" key={group.key}>
          <h2 className="tree-group-heading" title={group.path ?? undefined}>{group.label}</h2>
          <ul role="group" className="tree-group-items">
            {group.conversations.map((conversation) => {
              const hierarchy = buildAgentHierarchy(conversation.agents);
              const key = conversationKey(conversation.id);
              const isCollapsed = collapsed.has(key);
              const expandable = hierarchy.length > 0;
              return (
                <ConversationItem
                  key={conversation.id}
                  conversation={conversation}
                  agents={hierarchy}
                  selected={selectedId === conversation.id && selectedAgentId === null}
                  selectedAgentId={selectedId === conversation.id ? selectedAgentId : null}
                  collapsed={collapsed}
                  isCollapsed={isCollapsed}
                  expandable={expandable}
                  activeFocusKey={activeFocusKey}
                  setItemRef={setItemRef}
                  onFocusItem={setFocusedKey}
                  onToggle={toggle}
                  onSelect={onSelect}
                  onKeyDown={handleKeyDown}
                />
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}

type AgentBranch = {
  agent: AgentSnapshot;
  children: AgentBranch[];
};

type SharedItemProps = {
  collapsed: ReadonlySet<string>;
  activeFocusKey: string | null;
  setItemRef(key: string, element: HTMLElement | null): void;
  onFocusItem(key: string): void;
  onToggle(key: string): void;
  onSelect(conversationId: string, agentId?: string): void;
  onKeyDown(
    event: React.KeyboardEvent<HTMLElement>,
    item: VisibleItem,
    expandable: boolean,
  ): void;
};

function ConversationItem({
  conversation,
  agents,
  selected,
  selectedAgentId,
  isCollapsed,
  expandable,
  activeFocusKey,
  setItemRef,
  onFocusItem,
  onToggle,
  onSelect,
  onKeyDown,
  ...shared
}: SharedItemProps & {
  conversation: ConversationSummary;
  agents: AgentBranch[];
  selected: boolean;
  selectedAgentId: string | null;
  isCollapsed: boolean;
  expandable: boolean;
}) {
  const key = conversationKey(conversation.id);
  const status = conversationStatus(conversation);
  return (
    <li
      role="treeitem"
      aria-label={`${conversation.title}, ${providerName(conversation.provider)}, ${status.label}`}
      aria-level={1}
      aria-selected={selected}
      aria-expanded={expandable ? !isCollapsed : undefined}
      tabIndex={activeFocusKey === key ? 0 : -1}
      data-tree-key={key}
      className="tree-item conversation-item"
      ref={(element) => setItemRef(key, element)}
      onFocus={(event) => {
        if (event.target === event.currentTarget) onFocusItem(key);
      }}
      onClick={() => onSelect(conversation.id)}
      onKeyDown={(event) => {
        onKeyDown(event, { key, parentKey: null }, expandable);
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
        expanded={!isCollapsed}
        onToggle={() => onToggle(key)}
      />
      {expandable && !isCollapsed ? (
        <ul role="group" className="agent-group">
          {agents.map((branch) => (
            <AgentItem
              {...shared}
              key={branch.agent.id}
              branch={branch}
              selectedAgentId={selectedAgentId}
              conversationId={conversation.id}
              level={2}
              parentKey={key}
              activeFocusKey={activeFocusKey}
              setItemRef={setItemRef}
              onFocusItem={onFocusItem}
              onToggle={onToggle}
              onSelect={onSelect}
              onKeyDown={onKeyDown}
            />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

function AgentItem({
  branch,
  selectedAgentId,
  conversationId,
  level,
  parentKey,
  collapsed,
  activeFocusKey,
  setItemRef,
  onFocusItem,
  onToggle,
  onSelect,
  onKeyDown,
}: SharedItemProps & {
  branch: AgentBranch;
  selectedAgentId: string | null;
  conversationId: string;
  level: number;
  parentKey: string;
}) {
  const key = agentKey(branch.agent.id);
  const expandable = branch.children.length > 0;
  const isCollapsed = collapsed.has(key);
  const status = { label: agentStatusLabels[branch.agent.status], key: branch.agent.status };
  return (
    <li
      role="treeitem"
      aria-label={`${branch.agent.label}, ${providerName(branch.agent.provider)}, ${status.label}`}
      aria-level={level}
      aria-selected={selectedAgentId === branch.agent.id}
      aria-expanded={expandable ? !isCollapsed : undefined}
      tabIndex={activeFocusKey === key ? 0 : -1}
      data-tree-key={key}
      className="tree-item agent-item"
      ref={(element) => setItemRef(key, element)}
      onFocus={(event) => {
        if (event.target === event.currentTarget) onFocusItem(key);
      }}
      onClick={(event) => {
        event.stopPropagation();
        onSelect(conversationId, branch.agent.id);
      }}
      onKeyDown={(event) => {
        event.stopPropagation();
        onKeyDown(event, { key, parentKey }, expandable);
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect(conversationId, branch.agent.id);
        }
      }}
    >
      <TreeRow
        label={branch.agent.label}
        provider={branch.agent.provider}
        status={status}
        expandable={expandable}
        expanded={!isCollapsed}
        onToggle={() => onToggle(key)}
      />
      {expandable && !isCollapsed ? (
        <ul role="group" className="agent-group">
          {branch.children.map((child) => (
            <AgentItem
              key={child.agent.id}
              branch={child}
              selectedAgentId={selectedAgentId}
              conversationId={conversationId}
              level={level + 1}
              parentKey={key}
              collapsed={collapsed}
              activeFocusKey={activeFocusKey}
              setItemRef={setItemRef}
              onFocusItem={onFocusItem}
              onToggle={onToggle}
              onSelect={onSelect}
              onKeyDown={onKeyDown}
            />
          ))}
        </ul>
      ) : null}
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

function buildAgentHierarchy(agents: readonly AgentSnapshot[]): AgentBranch[] {
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
  const visited = new Set<string>();
  const branch = (agent: AgentSnapshot): AgentBranch => {
    if (visited.has(agent.id)) return { agent, children: [] };
    visited.add(agent.id);
    return {
      agent,
      children: (children.get(agent.id) ?? []).map(branch),
    };
  };
  return topLevel.map(branch);
}

function flattenVisibleItems(
  groups: readonly TreeGroup[],
  collapsed: ReadonlySet<string>,
): VisibleItem[] {
  const items: VisibleItem[] = [];
  const addBranch = (branch: AgentBranch, parentKey: string) => {
    const key = agentKey(branch.agent.id);
    items.push({ key, parentKey });
    if (!collapsed.has(key)) branch.children.forEach((child) => addBranch(child, key));
  };
  groups.forEach(({ conversations }) => conversations.forEach((conversation) => {
    const key = conversationKey(conversation.id);
    items.push({ key, parentKey: null });
    if (!collapsed.has(key)) {
      buildAgentHierarchy(conversation.agents).forEach((branch) => addBranch(branch, key));
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
