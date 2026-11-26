import { createMemo, createSignal, For, onSettled, Show } from "solid-js";
import type { TimelineItem } from "../../bridge/types";
import type { ConversationActions } from "../../app/store";
import { TimelineEntry } from "../timeline/TimelineEntry";

const PAGE_SIZE = 30;
const MAX_PAGES = 4;

/**
 * A conversation's routine protocol notifications. Nothing loads until you open the section, and
 * closing it forgets the loaded pages. The caller makes a new one for another conversation.
 */
export function Diagnostics(props: {
  conversationId: string;
  actions: Pick<ConversationActions, "loadDiagnostics" | "loadEventDetail">;
}) {
  const [open, setOpen] = createSignal(false);
  const [pages, setPages] = createSignal<TimelineItem[][]>([]);
  const [cursor, setCursor] = createSignal<string | null>(null);
  const [evicted, setEvicted] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let generation = 0;
  let inFlight = false;
  let failedCursor: string | null = null;
  onSettled(() => () => {
    generation += 1;
  });

  async function load(requested: string | null) {
    if (inFlight) return;
    inFlight = true;
    const current = ++generation;
    setLoading(true);
    setError(null);
    failedCursor = requested;
    try {
      const page = await props.actions.loadDiagnostics({ conversationId: props.conversationId, cursor: requested, limit: PAGE_SIZE });
      if (current !== generation) return;
      const bounded = page.items.slice(-PAGE_SIZE);
      const next = requested === null ? [bounded] : [...pages(), bounded];
      if (next.length > MAX_PAGES) setEvicted(true);
      else if (requested === null) setEvicted(false);
      setPages(next.slice(-MAX_PAGES));
      setCursor(page.nextCursor);
    } catch (reason) {
      if (current === generation) setError(reason instanceof Error ? reason.message : "Could not load diagnostics.");
    } finally {
      if (current === generation) {
        inFlight = false;
        setLoading(false);
      }
    }
  }

  function toggle() {
    if (open()) {
      generation += 1;
      inFlight = false;
      setLoading(false);
      setPages([]);
      setCursor(null);
      setEvicted(false);
      setError(null);
      setOpen(false);
    } else {
      setOpen(true);
      void load(null);
    }
  }

  const items = createMemo(() => [...new Map(pages().flat().map((item) => [item.id, item])).values()]
    .sort((left, right) => (BigInt(left.sequence) < BigInt(right.sequence) ? -1 : BigInt(left.sequence) > BigInt(right.sequence) ? 1 : 0)));
  return (
    <section class="inspector-section diagnostics" aria-labelledby="diagnostics-heading">
      <div class="inspector-section-heading">
        <h2 id="diagnostics-heading">Diagnostics</h2>
        <button type="button" class="disclosure-button" aria-expanded={open() ? "true" : "false"} aria-controls="diagnostics-content" aria-label={`${open() ? "Collapse" : "Expand"} diagnostics`} onClick={toggle}>{open() ? "−" : "+"}</button>
      </div>
      <Show when={open()}>
        <div id="diagnostics-content">
          <div class="diagnostics-controls"><button type="button" class="pt-button" disabled={loading()} onClick={() => void load(null)}>Refresh diagnostics</button></div>
          <Show when={loading()}><p role="status">Loading diagnostics…</p></Show>
          <Show when={error()}>
            <div><p role="alert" class="inline-error">{error()}</p><button type="button" class="disclosure-link" disabled={loading()} onClick={() => void load(failedCursor)}>Retry diagnostics</button></div>
          </Show>
          <Show when={!loading() && !error() && items().length === 0}><p class="empty-note">No diagnostics in this conversation.</p></Show>
          <Show when={evicted()}>
            <div class="history-window-note"><span>Newer diagnostics are outside this bounded view.</span><button type="button" class="disclosure-link" disabled={loading()} onClick={() => void load(null)}>Reload newest diagnostics</button></div>
          </Show>
          <ol class="diagnostics-list">
            <For each={items()} keyed={(item) => item.id}>{(item) => <li><TimelineEntry item={item()} actions={props.actions} /></li>}</For>
          </ol>
          <Show when={cursor()}>
            <button type="button" class="pt-button" disabled={loading()} onClick={() => void load(cursor())}>Load older diagnostics</button>
          </Show>
        </div>
      </Show>
    </section>
  );
}
