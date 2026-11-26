import { createEffect, createMemo, createSignal, For, Show, untrack } from "solid-js";
import { CopyButton } from "@prompting-time/ui";
import type { EventDetailSnapshot, TimelineItem, ToolOperationStatus } from "../../bridge/types";
import type { ConversationActions } from "../../app/store";

const statuses: Record<ToolOperationStatus, string> = {
  running: "Running", succeeded: "Succeeded", failed: "Failed", interrupted: "Interrupted", unknown: "Unknown",
};
const fields = ["input", "context", "error", "output"] as const;

/** One tool call: what it did, its status, and its captured input and output on request. */
export function ToolOperation(props: {
  item: TimelineItem;
  actions: Pick<ConversationActions, "loadEventDetail">;
  agentPath?: string;
  grouped?: boolean;
  expanded?: boolean;
  onToggle?(): void;
}) {
  const [localExpanded, setLocalExpanded] = createSignal(false);
  const expanded = () => props.expanded ?? localExpanded();
  const [retry, setRetry] = createSignal(0);
  // A loaded detail answers one request; a new revision, retry, or collapse asks again.
  const [loaded, setLoaded] = createSignal<{ request: string; detail: EventDetailSnapshot | null; error: string | null } | null>(null);
  const operation = () => props.item.operation;
  const version = () => operation()?.detailRevision ?? `${props.item.contentBytes}:${props.item.content}`;
  const hasDetails = () => operation()?.hasDetails ?? props.item.truncated;
  // A memo, so a refreshed copy of the same item does not ask again.
  const request = createMemo(() => (expanded() && hasDetails() ? `${props.item.id}:${version()}:${retry()}` : null));
  createEffect(request, (current) => {
    if (current === null) return;
    let active = true;
    // The request decides when to load; read the item as it is now.
    const expected = untrack(() => (operation() ? version() : null));
    untrack(() => props.actions.loadEventDetail({ eventId: props.item.id })).then((result) => {
      if (!active) return;
      if (expected !== null && (!result.operation || result.operation.detailRevision !== expected)) {
        setLoaded({ request: current, detail: null, error: "Activity changed while loading. Retry detail." });
      } else setLoaded({ request: current, detail: result, error: null });
    }, (reason) => {
      if (active) setLoaded({ request: current, detail: null, error: reason instanceof Error ? reason.message : "Could not load this activity." });
    });
    return () => {
      active = false;
    };
  });
  const result = () => (loaded()?.request === request() ? loaded() : null);
  const provider = () => (props.item.provider === "codex" ? "Codex" : "Claude");
  const label = () => (props.item.presentation === "failure" ? "failure" : props.item.presentation === "notice" ? "notice" : "tool activity");
  const fieldName = (field: (typeof fields)[number]) => (field === "input" ? "Command / input" : field[0].toUpperCase() + field.slice(1));
  return (
    <article class={["tool-operation", props.item.presentation, operation()?.status ?? "legacy"]} aria-label={`${provider()} ${label()}`}>
      <div class="tool-operation-summary">
        <span class="tool-operation-title" title={props.item.content}>{props.item.content}</span>
        <Show when={operation()}>{(current) => <span class="tool-operation-status">{statuses[current().status]}</span>}</Show>
        <Show when={operation()?.durationMs != null}><span class="tool-operation-meta">{Number(operation()!.durationMs) / 1000} s</span></Show>
        <Show when={operation()?.exitCode != null}><span class="tool-operation-meta">Exit {operation()!.exitCode}</span></Show>
        <Show when={!props.grouped}><span class="tool-operation-attribution">{provider()}{props.agentPath ? ` · ${props.agentPath}` : ""}</span></Show>
        <Show when={hasDetails()} fallback={<span class="tool-operation-meta">Details weren't captured</span>}>
          <button
            type="button"
            class="disclosure-link"
            data-operation-disclosure
            aria-expanded={expanded() ? "true" : "false"}
            onClick={() => (props.onToggle ? props.onToggle() : setLocalExpanded((value) => !value))}
          >
            {expanded() ? "Hide details" : "Show details"}
          </button>
        </Show>
      </div>
      <Show when={operation()?.conflicted}><p class="tool-operation-notice">Conflicting outcomes were reported.</p></Show>
      <Show when={(operation()?.truncated || props.item.truncated) && !expanded()}><p class="truncation-note">Preview truncated.</p></Show>
      <Show when={expanded() && hasDetails()}>
        <div class="tool-operation-detail">
          <Show when={!result()?.error} fallback={<><p role="alert">{result()?.error}</p><button type="button" class="disclosure-link" onClick={() => setRetry((value) => value + 1)}>Retry detail</button></>}>
            <Show when={result()?.detail} fallback={<p>Loading bounded detail…</p>}>
              {(detail) => (
                <Show
                  when={detail().operation}
                  fallback={<><pre>{detail().content}</pre><CopyButton content={detail().content} label={detail().truncated ? "Copy detail preview" : "Copy detail"} /><Show when={detail().truncated}><p class="truncation-note">Bounded detail remains truncated.</p></Show></>}
                >
                  {(captured) => (
                    <>
                      <For each={fields}>
                        {(field) => {
                          const value = () => captured()[field];
                          return (
                            <Show
                              when={value() !== null && !(field === "output" && value() === "")}
                              fallback={
                                <Show when={field === "output"}>
                                  <p>{value() === null ? "Output wasn't captured" : captured().truncated ? "No output retained; captured detail was truncated" : "Output was empty"}</p>
                                </Show>
                              }
                            >
                              <section class="tool-operation-field" aria-label={field}>
                                <div><span>{fieldName(field)}</span><CopyButton content={value()!} label={`Copy ${field}${captured().truncated ? " preview" : ""}`} /></div>
                                <pre>{value()}</pre>
                              </section>
                            </Show>
                          );
                        }}
                      </For>
                      <Show when={captured().truncated}><p class="truncation-note">Captured detail is truncated.</p></Show>
                    </>
                  )}
                </Show>
              )}
            </Show>
          </Show>
        </div>
      </Show>
    </article>
  );
}
