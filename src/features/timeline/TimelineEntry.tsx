import { useLayoutEffect, useRef, useState } from "react";
import type { TimelineItem } from "../../bridge/types";
import type { ConversationActions } from "../../app/store";
import { CopyButton, MessageContent } from "./MessageContent";

export function TimelineEntry({ item, actions, agentPath }: {
  item: TimelineItem;
  actions: Pick<ConversationActions, "loadEventDetail">;
  agentPath?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const [detail, setDetail] = useState<{ content: string; truncated: boolean } | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const detailVersion = `${item.id}:${item.contentBytes}:${item.truncated}:${item.content}`;
  const detailVersionRef = useRef(detailVersion);
  const detailRequestGeneration = useRef(0);
  const provider = item.provider === "codex" ? "Codex" : "Claude";

  useLayoutEffect(() => {
    detailVersionRef.current = detailVersion;
    detailRequestGeneration.current += 1;
    setExpanded(false);
    setDetail(null);
    setDetailError(null);
    return () => { detailRequestGeneration.current += 1; };
  }, [detailVersion]);

  async function loadDetail() {
    setDetailError(null);
    const generation = ++detailRequestGeneration.current;
    try {
      const result = await actions.loadEventDetail({ eventId: item.id });
      if (detailVersionRef.current === detailVersion && generation === detailRequestGeneration.current) {
        setDetail({ content: result.content, truncated: result.truncated });
      }
    } catch (reason) {
      if (detailVersionRef.current === detailVersion && generation === detailRequestGeneration.current) {
        setDetailError(reason instanceof Error ? reason.message : "Could not load this activity.");
      }
    }
  }

  function toggleDetail() {
    if (expanded) {
      detailRequestGeneration.current += 1;
      setExpanded(false);
      setDetail(null);
      setDetailError(null);
    } else {
      setExpanded(true);
      if (item.truncated) void loadDetail();
    }
  }

  const displayedDetail = expanded ? detail : null;
  const content = displayedDetail?.content ?? item.content;
  const truncated = displayedDetail?.truncated ?? item.truncated;
  const message = item.kind === "message";
  const user = message && item.role === "user";
  const label = item.presentation === "failure" ? "Failure"
    : item.presentation === "notice" ? "Notice"
    : item.kind === "progress" ? "Progress"
    : item.kind === "lifecycle" ? "Run lifecycle"
    : item.kind === "tool" ? "Tool activity" : "Provider activity";
  const detailLabel = message ? "full message" : item.kind === "tool" ? "tool output" : `full ${label.toLowerCase()}`;
  return <article
    className={message ? `timeline-message ${user ? "user" : "assistant"}` : `timeline-activity ${item.kind} ${item.presentation}`}
    aria-label={message ? user ? "You message" : `${provider} assistant message` : `${provider} ${label.toLowerCase()}`}
  >
    <header>{user ? <span>You</span> : <><span>{provider}{agentPath ? ` · ${agentPath}` : ""}</span>{message ? null : <span>{label}</span>}</>}</header>
    {message && !user ? <MessageContent content={content} /> : item.kind === "tool" && expanded ? <pre>{content}</pre> : <p className="literal-content">{content}</p>}
    {truncated ? <p className="truncation-note">{displayedDetail ? "Bounded detail remains truncated." : "Preview truncated."}</p> : null}
    <div className="entry-actions">
      {item.truncated || item.kind === "tool" ? <button type="button" className="disclosure-link" aria-expanded={expanded} onClick={toggleDetail}>
        {expanded ? `Hide ${detailLabel}` : `Show ${detailLabel}`}
      </button> : null}
      {message ? <CopyButton content={content} label={truncated ? "Copy preview" : "Copy message"} /> : null}
    </div>
    {expanded && item.truncated && !detail && !detailError ? <span className="truncation-note">Loading bounded detail…</span> : null}
    {detailError ? <div><p role="alert">{detailError}</p><button type="button" className="disclosure-link" onClick={() => void loadDetail()}>Retry detail</button></div> : null}
  </article>;
}
