import { For, Show } from "solid-js";
import type { ConversationSummary } from "../../bridge/types";
import type { SelectedPath } from "../../app/store";

/** The path from the root to a selected child, so you can go back up. A root shows none. */
export function ConversationNavigation(props: {
  conversation: ConversationSummary;
  conversationsById: Readonly<Record<string, ConversationSummary>>;
  path: SelectedPath | null;
  onSelect(id: string): void;
}) {
  return (
    <Show when={props.conversation.parentId !== null}>
      <nav class="conversation-navigation" aria-label="Conversation ancestry">
        <ol>
          <Show when={props.path?.truncated}>
            <li><span>Earlier ancestry is outside this view</span></li>
          </Show>
          <For each={props.path?.ids ?? [props.conversation.id]}>
            {(id) => (
              <Show when={props.conversationsById[id]}>
                {(node) => (
                  <li>
                    <button type="button" title={node().title} aria-current={id === props.conversation.id ? "location" : undefined} onClick={() => props.onSelect(id)}>
                      {node().title}
                    </button>
                  </li>
                )}
              </Show>
            )}
          </For>
        </ol>
      </nav>
    </Show>
  );
}
