import { useEffect, useRef, useState } from "react";
import type { TimelineItem } from "../../bridge/types";
import type { ConversationActions } from "../../app/store";
import { TimelineEntry } from "../timeline/TimelineEntry";

const PAGE_SIZE = 30;
const MAX_PAGES = 4;

export function Diagnostics({ conversationId, actions }: {
  conversationId: string;
  actions: Pick<ConversationActions, "loadDiagnostics" | "loadEventDetail">;
}) {
  // Keying the reader also fences local disclosure and retained pages on selection.
  return <DiagnosticReader key={conversationId} conversationId={conversationId} actions={actions} />;
}

function DiagnosticReader({ conversationId, actions }: {
  conversationId: string;
  actions: Pick<ConversationActions, "loadDiagnostics" | "loadEventDetail">;
}) {
  const [open, setOpen] = useState(false);
  const [pages, setPages] = useState<TimelineItem[][]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [evicted, setEvicted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestGeneration = useRef(0);
  const inFlight = useRef(false);
  const failedCursor = useRef<string | null>(null);
  useEffect(() => () => { requestGeneration.current += 1; }, []);

  async function load(requestedCursor: string | null) {
    if (inFlight.current) return;
    inFlight.current = true;
    const generation = ++requestGeneration.current;
    setLoading(true);
    setError(null);
    failedCursor.current = requestedCursor;
    try {
      const page = await actions.loadDiagnostics({ conversationId, cursor: requestedCursor, limit: PAGE_SIZE });
      if (generation !== requestGeneration.current) return;
      const bounded = page.items.slice(-PAGE_SIZE);
      setPages((current) => {
        const next = requestedCursor === null ? [bounded] : [...current, bounded];
        if (next.length > MAX_PAGES) setEvicted(true);
        return next.slice(-MAX_PAGES);
      });
      if (requestedCursor === null) setEvicted(false);
      setCursor(page.nextCursor);
    } catch (reason) {
      if (generation === requestGeneration.current) setError(reason instanceof Error ? reason.message : "Could not load diagnostics.");
    } finally {
      if (generation === requestGeneration.current) {
        inFlight.current = false;
        setLoading(false);
      }
    }
  }

  function toggle() {
    if (open) {
      requestGeneration.current += 1;
      inFlight.current = false;
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

  const items = [...new Map(pages.flat().map((item) => [item.id, item])).values()]
    .sort((left, right) => BigInt(left.sequence) < BigInt(right.sequence) ? -1 : BigInt(left.sequence) > BigInt(right.sequence) ? 1 : 0);
  return <section className="inspector-section diagnostics" aria-labelledby="diagnostics-heading">
    <div className="inspector-section-heading">
      <h2 id="diagnostics-heading">Diagnostics</h2>
      <button type="button" className="disclosure-button" aria-expanded={open} aria-controls="diagnostics-content" aria-label={`${open ? "Collapse" : "Expand"} diagnostics`} onClick={toggle}>{open ? "−" : "+"}</button>
    </div>
    {open ? <div id="diagnostics-content">
      <div className="diagnostics-controls"><button type="button" className="secondary-button" disabled={loading} onClick={() => void load(null)}>Refresh diagnostics</button></div>
      {loading ? <p role="status">Loading diagnostics…</p> : null}
      {error ? <div><p role="alert" className="inline-error">{error}</p><button type="button" className="disclosure-link" disabled={loading} onClick={() => void load(failedCursor.current)}>Retry diagnostics</button></div> : null}
      {!loading && !error && items.length === 0 ? <p className="empty-note">No diagnostics in this conversation.</p> : null}
      {evicted ? <div className="history-window-note"><span>Newer diagnostics are outside this bounded view.</span><button type="button" className="disclosure-link" disabled={loading} onClick={() => void load(null)}>Reload newest diagnostics</button></div> : null}
      <ol className="diagnostics-list">{items.map((item) => <li key={item.id}><TimelineEntry item={item} actions={actions} /></li>)}</ol>
      {cursor ? <button type="button" className="secondary-button" disabled={loading} onClick={() => void load(cursor)}>Load older diagnostics</button> : null}
    </div> : null}
  </section>;
}
