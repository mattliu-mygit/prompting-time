import type { ConversationSummary } from "../../bridge/types";
import type { SelectedPath } from "../../app/store";

export function ConversationNavigation({ conversation, conversationsById, path, onSelect }: {
  conversation: ConversationSummary;
  conversationsById: Readonly<Record<string, ConversationSummary>>;
  path: SelectedPath | null;
  onSelect(id: string): void;
}) {
  if (conversation.parentId === null) return null;
  return <nav className="conversation-navigation" aria-label="Conversation ancestry"><ol>
    {path?.truncated ? <li><span>Earlier ancestry is outside this view</span></li> : null}
    {(path?.ids ?? [conversation.id]).map(id => {
      const node = conversationsById[id];
      return node ? <li key={id}><button type="button" title={node.title}
        aria-current={id === conversation.id ? "location" : undefined} onClick={() => onSelect(id)}>{node.title}</button></li> : null;
    })}
  </ol></nav>;
}
