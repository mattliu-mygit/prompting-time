import type { AppSnapshot } from "../../app/store";
import type { AgentSnapshot } from "../../bridge/types";

export function knownAgents(snapshot: AppSnapshot) {
  return Object.values(snapshot.conversationsById).flatMap(conversation => {
    if (conversation.archived || !conversation.currentRunId) return [];
    const window = snapshot.agentWindow;
    const ids = new Set(conversation.agentIds);
    if (window?.conversationId === conversation.id && window.runId === conversation.currentRunId) {
      window.pages.forEach(page => page.forEach(id => ids.add(id)));
    }
    return [...ids].flatMap(id => {
      const agent = snapshot.agentsById[id];
      return agent ? [{ conversationId: conversation.id, conversationTitle: conversation.title, agent }] : [];
    });
  });
}

export function agentPath(agents: readonly AgentSnapshot[], selectedId: string | null) {
  const byId = new Map(agents.map(agent => [agent.id, agent]));
  const nodes: AgentSnapshot[] = [];
  const seen = new Set<string>();
  let id = selectedId;
  while (id !== null) {
    const agent = byId.get(id);
    if (!agent || seen.has(id)) return { nodes: nodes.reverse(), incomplete: true };
    seen.add(id);
    nodes.push(agent);
    id = agent.parentId;
  }
  return { nodes: nodes.reverse(), incomplete: false };
}

export function AgentNavigation({ title, agents, selectedId, onSelect, onLoadMore, loading }: {
  title: string; agents: readonly AgentSnapshot[]; selectedId: string | null;
  onSelect(id?: string): void; onLoadMore?(): void; loading?: boolean;
}) {
  const path = agentPath(agents, selectedId);
  if (selectedId === null) return null;
  return <nav className="agent-navigation" aria-label="Agent ancestry">
    <ol>
      <li><button type="button" title={title} onClick={() => onSelect(undefined)}>{title}</button></li>
      {path.incomplete ? <li><span>Agent path incomplete</span></li> : null}
      {path.nodes.map(agent => <li key={agent.id}>
        <button type="button" title={agent.label} aria-current={agent.id === selectedId ? "location" : undefined} onClick={() => onSelect(agent.id)}>{agent.label}</button>
      </li>)}
    </ol>
    {onLoadMore ? <button type="button" disabled={loading} onClick={onLoadMore}>{loading ? "Loading agents…" : "Load more agents"}</button> : null}
  </nav>;
}
