import { For, Show } from "solid-js";
import type { Flow, FlowCall } from "./shown";

/** Words beside each call, so its color is never the only cue. */
const STATUS_WORDS: Record<FlowCall["status"], string> = { same: "", added: "new", removed: "removed", changed: "changed" };
const LANE = 168;
const TOP = 52;
const ROW = 40;
/** Labels longer than this are cut with an ellipsis; the call's accessible name keeps the whole label. */
const LABEL_CHARACTERS = 30;
const cut = (text: string) => (text.length > LABEL_CHARACTERS ? `${text.slice(0, LABEL_CHARACTERS - 1)}…` : text);

/**
 * The calls one input makes before and after the change, overlaid on one set of lifelines: a call
 * only the old code makes is struck through in red and stays drawn, a new one is green, and a
 * changed one shows its old label struck before its new one. A call with lines opens them in the
 * walkthrough's code on a click or Enter.
 */
export function FlowDiagram(props: { flow: Flow; onOpen(path: string, line: number): void }) {
  const actors = () => [...new Set(props.flow.calls.flatMap((call) => [call.from, call.to]))];
  const x = (name: string) => LANE / 2 + actors().indexOf(name) * LANE;
  const width = () => actors().length * LANE;
  const height = () => TOP + props.flow.calls.length * ROW + 12;
  const name = (call: FlowCall) => `${call.from} to ${call.to}: ${call.status === "changed" && call.old_label ? `${call.old_label}, now ` : ""}${call.label}${STATUS_WORDS[call.status] ? `, ${STATUS_WORDS[call.status]}` : ""}`;
  return (
    <div class="flow">
      <p class="flow-input"><span class="flow-input-label">Input</span> <code>{props.flow.input}</code></p>
      <div class="flow-canvas">
        <svg class="flow-diagram" viewBox={`0 0 ${width()} ${height()}`} width={width()} height={height()} role="group" aria-label={`${props.flow.title}: calls before and after this change`}>
          <defs>
            <marker id="flow-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,0 L8,4 L0,8 z" fill="context-stroke" /></marker>
          </defs>
          <For each={actors()}>
            {(actor) => (
              <g class="flow-actor" aria-hidden="true">
                <line x1={x(actor)} y1={34} x2={x(actor)} y2={height() - 4} />
                <rect x={x(actor) - LANE / 2 + 8} y={6} width={LANE - 16} height={28} />
                <text x={x(actor)} y={24} text-anchor="middle">{cut(actor)}</text>
              </g>
            )}
          </For>
          <For each={props.flow.calls}>
            {(call, index) => {
              const y = () => TOP + index() * ROW + 16;
              const self = () => call.from === call.to;
              const open = () => {
                if (call.path && call.line) props.onOpen(call.path, call.line);
              };
              return (
                <g
                  class="flow-call"
                  data-status={call.status}
                  role={call.path ? "button" : "img"}
                  tabindex={call.path ? 0 : undefined}
                  aria-label={name(call)}
                  onClick={open}
                  onKeyDown={(event: KeyboardEvent) => {
                    if (event.key !== "Enter" && event.key !== " ") return;
                    event.preventDefault();
                    open();
                  }}
                >
                  <title>{name(call)}{call.path ? ` (${call.path}:${call.line})` : ""}</title>
                  <rect class="flow-hit" x={Math.min(x(call.from), x(call.to)) - 6} y={y() - 24} width={Math.abs(x(call.to) - x(call.from)) + (self() ? LANE : 12)} height={ROW - 4} />
                  <Show
                    when={!self()}
                    fallback={<path d={`M${x(call.from)},${y() - 6} h26 v12 h-23`} fill="none" marker-end="url(#flow-arrow)" />}
                  >
                    <line x1={x(call.from)} y1={y()} x2={x(call.to) + (x(call.to) > x(call.from) ? -3 : 3)} y2={y()} marker-end="url(#flow-arrow)" />
                  </Show>
                  <text x={self() ? x(call.from) + 32 : (x(call.from) + x(call.to)) / 2} y={self() ? y() + 4 : y() - 8} text-anchor={self() ? "start" : "middle"}>
                    <Show when={call.status === "changed" && call.old_label}>{(old) => <><tspan class="flow-old">{cut(old())}</tspan><tspan> → </tspan></>}</Show>
                    <tspan class="flow-label">{cut(call.label)}</tspan>
                    <Show when={STATUS_WORDS[call.status]}><tspan class="flow-tag"> {STATUS_WORDS[call.status]}</tspan></Show>
                  </text>
                </g>
              );
            }}
          </For>
        </svg>
      </div>
      <p class="flow-legend">
        <span><i data-status="same" />before and after</span>
        <span><i data-status="added" />new</span>
        <span><i data-status="changed" />changed: <s>was</s> → now</span>
        <span><i data-status="removed" />removed: struck, not hidden</span>
      </p>
    </div>
  );
}
