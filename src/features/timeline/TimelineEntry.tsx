import { createSignal, Show } from "solid-js";
import { CopyButton, Markdown } from "@prompting-time/ui";
import type { TimelineItem } from "../../bridge/types";
import type { ConversationActions } from "../../app/store";
import { ToolOperation } from "./ToolOperation";

type EntryProps = {
  item: TimelineItem;
  actions: Pick<ConversationActions, "loadEventDetail">;
  agentPath?: string;
  grouped?: boolean;
  expanded?: boolean;
  onToggle?(): void;
};

export function TimelineEntry(props: EntryProps) {
  return (
    <Show when={props.item.kind === "tool"} fallback={<TextEntry {...props} />}>
      <ToolOperation {...props} />
    </Show>
  );
}

type Disclosure = { version: string; detail: { content: string; truncated: boolean } | null; error: string | null };

function TextEntry(props: EntryProps) {
  // Disclosed detail belongs to one version of the entry; a streamed change closes it.
  const version = () => `${props.item.id}:${props.item.contentBytes}:${props.item.truncated}:${props.item.content}`;
  const [disclosure, setDisclosure] = createSignal<Disclosure | null>(null);
  const open = () => (disclosure()?.version === version() ? disclosure() : null);
  let attempt = 0;
  const provider = () => (props.item.provider === "codex" ? "Codex" : "Claude");

  async function loadDetail() {
    const requested = version();
    const current = ++attempt;
    setDisclosure({ version: requested, detail: null, error: null });
    try {
      const result = await props.actions.loadEventDetail({ eventId: props.item.id });
      if (current === attempt && disclosure()?.version === requested) setDisclosure({ version: requested, detail: { content: result.content, truncated: result.truncated }, error: null });
    } catch (reason) {
      if (current === attempt && disclosure()?.version === requested) {
        setDisclosure({ version: requested, detail: null, error: reason instanceof Error ? reason.message : "Could not load this activity." });
      }
    }
  }

  function toggleDetail() {
    if (open()) {
      attempt += 1;
      setDisclosure(null);
    } else if (props.item.truncated) void loadDetail();
    else setDisclosure({ version: version(), detail: null, error: null });
  }

  const content = () => open()?.detail?.content ?? props.item.content;
  const truncated = () => open()?.detail?.truncated ?? props.item.truncated;
  const message = () => props.item.kind === "message";
  const user = () => message() && props.item.role === "user";
  const label = () => (props.item.presentation === "failure" ? "Failure"
    : props.item.presentation === "notice" ? "Notice"
      : props.item.kind === "progress" ? "Progress"
        : props.item.kind === "lifecycle" ? "Run lifecycle" : "Provider activity");
  const detailLabel = () => (message() ? "full message" : `full ${label().toLowerCase()}`);
  const attribution = () => `${provider()}${props.agentPath ? ` · ${props.agentPath}` : ""}`;
  const compactLifecycle = () => props.item.kind === "lifecycle" && props.item.presentation === "normal" && !props.item.truncated;

  return (
    <Show
      when={!compactLifecycle()}
      fallback={
        <article class="timeline-activity lifecycle normal compact-lifecycle" aria-label={`${provider()} run lifecycle`}>
          <span>{attribution()}</span>{" · "}<span class="literal-content">{content()}</span>
        </article>
      }
    >
      <article
        class={message() ? ["timeline-message", user() ? "user" : "assistant"] : ["timeline-activity", props.item.kind, props.item.presentation]}
        aria-label={message() ? (user() ? "You message" : `${provider()} assistant message`) : `${provider()} ${label().toLowerCase()}`}
      >
        <header>
          <Show when={!user()} fallback={<span>You</span>}>
            <span>{attribution()}</span>
            <Show when={!message()}><span>{label()}</span></Show>
          </Show>
          <Show when={message()}>
            <CopyButton content={content()} label={truncated() ? "Copy preview" : "Copy message"} iconOnly />
          </Show>
        </header>
        <Show when={message()} fallback={<p class="literal-content">{content()}</p>}>
          <Markdown content={content()} />
        </Show>
        <Show when={truncated()}>
          <p class="truncation-note">{open()?.detail ? "Bounded detail remains truncated." : "Preview truncated."}</p>
        </Show>
        <Show when={props.item.truncated}>
          <div class="entry-actions">
            <button type="button" class="disclosure-link" aria-expanded={open() ? "true" : "false"} onClick={toggleDetail}>
              {open() ? `Hide ${detailLabel()}` : `Show ${detailLabel()}`}
            </button>
          </div>
        </Show>
        <Show when={open() && props.item.truncated && !open()!.detail && !open()!.error}>
          <span class="truncation-note">Loading bounded detail…</span>
        </Show>
        <Show when={open()?.error}>
          <div>
            <p role="alert">{open()?.error}</p>
            <button type="button" class="disclosure-link" onClick={() => void loadDetail()}>Retry detail</button>
          </div>
        </Show>
      </article>
    </Show>
  );
}
