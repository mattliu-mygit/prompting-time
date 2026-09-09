import { useEffect, useRef, useState } from "react";
import type { EventDetailSnapshot, TimelineItem, ToolOperationStatus } from "../../bridge/types";
import type { ConversationActions } from "../../app/store";
import { CopyButton } from "./MessageContent";

const statuses: Record<ToolOperationStatus, string> = {
  running: "Running", succeeded: "Succeeded", failed: "Failed", interrupted: "Interrupted", unknown: "Unknown",
};

export function ToolOperation({ item, actions, agentPath, grouped = false, expanded: controlledExpanded, onToggle }: {
  item: TimelineItem;
  actions: Pick<ConversationActions, "loadEventDetail">;
  agentPath?: string;
  grouped?: boolean;
  expanded?: boolean;
  onToggle?(): void;
}) {
  const [localExpanded, setLocalExpanded] = useState(false);
  const expanded = controlledExpanded ?? localExpanded;
  const [detail, setDetail] = useState<EventDetailSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const operation = item.operation;
  const version = operation?.detailRevision ?? `${item.contentBytes}:${item.content}`;
  const hasDetails = operation?.hasDetails ?? item.truncated;
  const load = useRef(actions.loadEventDetail);
  load.current = actions.loadEventDetail;
  useEffect(() => {
    let current = true;
    setDetail(null);
    setError(null);
    if (expanded && hasDetails) {
      void load.current({ eventId: item.id }).then(result => {
        if (!current) return;
        if (operation && (!result.operation || result.operation.detailRevision !== version)) {
          setError("Activity changed while loading. Retry detail.");
        } else setDetail(result);
      }, reason => {
        if (current) setError(reason instanceof Error ? reason.message : "Could not load this activity.");
      });
    }
    return () => { current = false; };
  }, [item.id, version, expanded, hasDetails, retry, operation !== null]);

  const provider = item.provider === "codex" ? "Codex" : "Claude";
  const captured = detail?.operation;
  const duration = operation?.durationMs;
  const label = item.presentation === "failure" ? "failure" : item.presentation === "notice" ? "notice" : "tool activity";
  return <article className={`tool-operation ${item.presentation} ${operation?.status ?? "legacy"}`} aria-label={`${provider} ${label}`}>
    <div className="tool-operation-summary">
      <span className="tool-operation-title" title={item.content}>{item.content}</span>
      {operation ? <span className="tool-operation-status">{statuses[operation.status]}</span> : null}
      {duration !== null && duration !== undefined ? <span className="tool-operation-meta">{Number(duration) / 1000} s</span> : null}
      {operation?.exitCode !== null && operation?.exitCode !== undefined ? <span className="tool-operation-meta">Exit {operation.exitCode}</span> : null}
      {!grouped ? <span className="tool-operation-attribution">{provider}{agentPath ? ` · ${agentPath}` : ""}</span> : null}
      {hasDetails ? <button type="button" className="disclosure-link" data-operation-disclosure aria-expanded={expanded} onClick={onToggle ?? (() => setLocalExpanded(value => !value))}>{expanded ? "Hide details" : "Show details"}</button>
        : <span className="tool-operation-meta">Details weren't captured</span>}
    </div>
    {operation?.conflicted ? <p className="tool-operation-notice">Conflicting outcomes were reported.</p> : null}
    {(operation?.truncated || item.truncated) && !expanded ? <p className="truncation-note">Preview truncated.</p> : null}
    {expanded && hasDetails ? <div className="tool-operation-detail">
      {error ? <><p role="alert">{error}</p><button type="button" className="disclosure-link" onClick={() => setRetry(value => value + 1)}>Retry detail</button></>
        : !detail ? <p>Loading bounded detail…</p>
          : captured ? <>
            {(["input", "context", "error", "output"] as const).map(field => {
              const value = captured[field];
              if (value === null) return field === "output" ? <p key={field}>Output wasn't captured</p> : null;
              if (field === "output" && value === "") return <p key={field}>Output was empty</p>;
              return <section key={field} className="tool-operation-field" aria-label={field}>
                <div><span>{field === "input" ? "Command / input" : field[0].toUpperCase() + field.slice(1)}</span><CopyButton content={value} label={`Copy ${field}${captured.truncated ? " preview" : ""}`} /></div>
                <pre>{value}</pre>
              </section>;
            })}
            {captured.truncated ? <p className="truncation-note">Captured detail is truncated.</p> : null}
          </> : <><pre>{detail.content}</pre><CopyButton content={detail.content} label={detail.truncated ? "Copy detail preview" : "Copy detail"} />{detail.truncated ? <p className="truncation-note">Bounded detail remains truncated.</p> : null}</>}
    </div> : null}
  </article>;
}
