import { createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import { Button, ChevronDownIcon, ChevronRightIcon, CloseIcon, Diff, IconButton, parsePatchFiles, type DiffTarget, type FileDiffMetadata } from "@prompting-time/ui";
import { listenToViewKeys, ViewHead, ViewLimits } from "./FullView";
import { plural, series, Series } from "./ReviewTop";
import type { ConnectionEdge, ConnectionNode, Connections } from "./shown";
import { viewName } from "./views";

/** Each kind of node in words, one and many. */
const KIND: Record<ConnectionNode["kind"], readonly [string, string]> = {
  func: ["function", "functions"],
  method: ["method", "methods"],
  type: ["type", "types"],
  var: ["variable", "variables"],
  const: ["constant", "constants"],
};
const COLUMN = 210;
const ROW = 88;
const MARGIN = 32;
/** Names longer than this are cut with an ellipsis on the map; the node's accessible name and its panel keep the whole name. */
const NAME_CHARACTERS = 24;
/** How many unchanged callers the summary names before it says how many more there are. */
const NAMED_CALLERS = 4;

const cut = (text: string) => (text.length > NAME_CHARACTERS ? `${text.slice(0, NAME_CHARACTERS - 1)}…` : text);
/** A circle's radius: larger for more lines changed, with unchanged callers small and quiet. */
const radius = (node: ConnectionNode) => (node.change === "unchanged" ? 6 : Math.min(24, 8 + 2 * Math.sqrt(node.lines_changed)));
/** "function", or "functions and types" for several nodes of those kinds. */
function kindsOf(nodes: readonly ConnectionNode[]): string {
  const kinds = (Object.keys(KIND) as ConnectionNode["kind"][]).filter((kind) => nodes.some((node) => node.kind === kind));
  return series(kinds.map((kind) => KIND[kind][nodes.length === 1 ? 0 : 1]));
}
/** "changed · 19 lines" under a node's name, or "unchanged". */
const changeText = (node: ConnectionNode) => (node.change === "unchanged" ? "unchanged" : `${node.change} · ${plural(node.lines_changed, "line")}`);
const location = (node: ConnectionNode) => `${node.path}:${node.line}`;

export type PlacedNode = ConnectionNode & { x: number; y: number; r: number };
export type ConnectionsLayout = { nodes: PlacedNode[]; width: number; height: number };

/**
 * Places the nodes in columns, callers left of what they call or use: a node's column is the longest
 * chain of callers before it, leaving out the edge that closes each cycle. In each column the nodes
 * sit near the average row of their neighbors in the column before, then the column after, which
 * keeps the lines short and their crossings few. Returns the nodes in reading order, column by
 * column and top to bottom, with each circle's center and radius.
 */
export function layoutConnections(connections: Connections): ConnectionsLayout {
  const ids = connections.nodes.map((node) => node.id);
  const calls = connections.edges.filter((edge) => edge.from !== edge.to);
  const out = new Map(ids.map((id) => [id, calls.filter((edge) => edge.from === id).map((edge) => edge.to)]));
  // Depth first from the nodes nothing calls, then from the rest: an edge back to a node still
  // being visited closes a cycle, and only the other edges set the columns.
  const kept: Array<{ from: string; to: string }> = [];
  const visiting = new Set<string>();
  const visited = new Set<string>();
  const visit = (id: string) => {
    visiting.add(id);
    visited.add(id);
    for (const to of out.get(id)!) {
      if (visiting.has(to)) continue;
      kept.push({ from: id, to });
      if (!visited.has(to)) visit(to);
    }
    visiting.delete(id);
  };
  const called = new Set(calls.map((edge) => edge.to));
  for (const id of [...ids.filter((id) => !called.has(id)), ...ids]) if (!visited.has(id)) visit(id);

  // Each node's column: one more than its furthest caller's, in an order where callers come first.
  const column = new Map(ids.map((id) => [id, 0]));
  const waiting = new Map(ids.map((id) => [id, kept.filter((edge) => edge.to === id).length]));
  const ready = ids.filter((id) => waiting.get(id) === 0);
  for (let index = 0; index < ready.length; index += 1) {
    const id = ready[index]!;
    for (const edge of kept.filter((item) => item.from === id)) {
      column.set(edge.to, Math.max(column.get(edge.to)!, column.get(id)! + 1));
      waiting.set(edge.to, waiting.get(edge.to)! - 1);
      if (waiting.get(edge.to) === 0) ready.push(edge.to);
    }
  }
  const columns: string[][] = [];
  for (const id of ids) (columns[column.get(id)!] ??= []).push(id);
  const tallest = Math.max(...columns.map((list) => list.length));

  // Rows, counted from the top of the tallest column, with shorter columns centered beside it.
  const row = new Map<string, number>();
  const place = () => columns.forEach((list) => list.forEach((id, index) => row.set(id, (tallest - list.length) / 2 + index)));
  const order = (list: string[], before: boolean) => {
    const center = new Map(list.map((id) => {
      const near = kept.filter((edge) => (before ? edge.to === id : edge.from === id)).map((edge) => row.get(before ? edge.from : edge.to)!);
      return [id, near.length > 0 ? near.reduce((sum, value) => sum + value, 0) / near.length : row.get(id)!];
    }));
    list.sort((left, right) => center.get(left)! - center.get(right)!);
    place();
  };
  place();
  for (let round = 0; round < 2; round += 1) {
    for (let index = 1; index < columns.length; index += 1) order(columns[index]!, true);
    for (let index = columns.length - 2; index >= 0; index -= 1) order(columns[index]!, false);
  }
  const byId = new Map(connections.nodes.map((node) => [node.id, node]));
  return {
    nodes: columns.flatMap((list, index) => list.map((id): PlacedNode => {
      const node = byId.get(id)!;
      return { ...node, r: radius(node), x: MARGIN + index * COLUMN + COLUMN / 2, y: MARGIN + row.get(id)! * ROW + 24 };
    })),
    width: columns.length * COLUMN + 2 * MARGIN,
    height: tallest * ROW + 2 * MARGIN,
  };
}

/** An edge's line: right from a caller into what it calls, around a column, under the rows back to the left, or a loop over a node that calls itself. */
function edgePath(from: PlacedNode, to: PlacedNode): string {
  if (from.id === to.id) {
    const { x, y, r } = from;
    return `M${x - r * 0.6},${y - r * 0.8} C${x - r - 14},${y - r - 30} ${x + r + 14},${y - r - 30} ${x + r * 0.6 + 2},${y - r * 0.8 - 2}`;
  }
  if (to.x > from.x) {
    const start = from.x + from.r;
    const end = to.x - to.r - 2;
    const bend = (end - start) / 2;
    return `M${start},${from.y} C${start + bend},${from.y} ${end - bend},${to.y} ${end},${to.y}`;
  }
  if (to.x < from.x) {
    const start = from.x - from.r;
    const end = to.x + to.r + 2;
    const below = Math.max(from.y, to.y) + ROW / 2;
    return `M${start},${from.y} C${start - 30},${below} ${end + 30},${below} ${end},${to.y}`;
  }
  const start = from.x + from.r;
  const end = to.x + to.r + 2;
  const out = from.x + Math.max(from.r, to.r) + 40;
  return `M${start},${from.y} C${out},${from.y} ${out},${to.y} ${end},${to.y}`;
}

/**
 * Where the diff opens for a node: at its own line when the patch shows it, else at the first line
 * after it that the patch shows, since a change inside a function usually comes after its first
 * line; null when the patch shows neither. A removed node's line counts in the old file.
 */
function openAt(file: FileDiffMetadata, node: ConnectionNode): DiffTarget | null {
  const old = node.change === "removed";
  const runs = file.hunks
    .map((hunk): [number, number] => (old ? [hunk.deletionStart, hunk.deletionStart + hunk.deletionCount - 1] : [hunk.additionStart, hunk.additionStart + hunk.additionCount - 1]))
    .filter(([start, end]) => end >= start);
  const line = runs.some(([start, end]) => node.line >= start && node.line <= end) ? node.line : runs.find(([start]) => start > node.line)?.[0];
  return line === undefined ? null : { start: line, end: line, ...(old ? { old: true } : {}) };
}

/**
 * How the changed code connects, in the walkthrough where the layers start: how many functions and
 * types the change touches, the unchanged code that calls them, how many links only a search for a
 * name found, and Open the map, which shows the whole map in the walkthrough's place.
 */
export function ConnectionsSummary(props: { connections: Connections; headingId: string; onOpen(button: HTMLButtonElement): void }) {
  const changed = () => props.connections.nodes.filter((node) => node.change !== "unchanged");
  const callers = () => props.connections.nodes.filter((node) => node.change === "unchanged");
  const searched = () => props.connections.edges.filter((edge) => edge.found_by === "name_search").length;
  const changes = () => {
    const counts = (["added", "changed", "removed"] as const).map((change) => ({ change, count: changed().filter((node) => node.change === change).length })).filter((item) => item.count > 0);
    if (counts.length === 0) return "None of these functions or types change.";
    if (counts.length === 1) return `${changed().length} ${counts[0]!.change} ${kindsOf(changed())}.`;
    return `${changed().length} ${kindsOf(changed())} change here: ${series(counts.map((item) => `${item.count} ${item.change}`))}.`;
  };
  /** The first few callers by name, then how many more. */
  const named = () => [
    ...callers().slice(0, NAMED_CALLERS).map((node) => ({ text: node.name, code: true })),
    ...(callers().length > NAMED_CALLERS ? [{ text: `${callers().length - NAMED_CALLERS} more`, code: false }] : []),
  ];
  return (
    <section class="walk-view-summary walk-prose walk-connections" data-part="connections" aria-labelledby={props.headingId}>
      <div class="walk-view-head">
        <h3 id={props.headingId}>{viewName("connections")}</h3>
        <Button onClick={(event) => props.onOpen(event.currentTarget)}>Open the map</Button>
      </div>
      <p>{changes()}</p>
      <Show when={callers().length > 0} fallback={<p>No unchanged code in the map calls or uses them.</p>}>
        <p>{plural(callers().length, "unchanged caller")}: <Series items={named()} />.</p>
      </Show>
      <Show when={searched() > 0}>
        <p class="walk-muted">
          {searched()} of {plural(props.connections.edges.length, "link")} {searched() === 1 ? "was" : "were"} found by name search, so {searched() === 1 ? "it" : "each"} may point at a different function or type with the same name.
        </p>
      </Show>
    </section>
  );
}

type ViewProps = {
  connections: Connections;
  /** The pull request's change as the review read it, a unified diff, for each node's code. */
  patch: string | null;
  onBack(): void;
};

/**
 * How the changed code connects, in full, in the walkthrough's place: a map of the functions and
 * types the change touches and the code that calls or uses them, callers on the left. A circle's
 * size is its lines changed; added is green and removed red, each with its word under the name,
 * changed is plain, and unchanged callers are small and gray. An arrow points from a caller to
 * what it calls or uses, dashed when only a search for the name found it. A click on a node, Enter
 * on it, or j and k through the changed ones selects it: its links stay bright and its panel shows
 * where it is, which opens the diff at that line, and its links, each a way to select the other
 * end. Under the map: what its marks mean, and what it may miss. Escape clears the selection, then
 * goes back. Nothing moves or opens on hover.
 */
export function ConnectionsView(props: ViewProps) {
  const map = createMemo(() => layoutConnections(props.connections));
  const files = createMemo(() => (props.patch ? parsePatchFiles(props.patch).flatMap((parsed) => parsed.files) : []));
  const [selected, setSelected] = createSignal<string | null>(null);
  // Whether the panel shows the diff at the selected node's line; it stays open as the selection moves.
  const [codeOpen, setCodeOpen] = createSignal(false);
  const node = (id: string) => map().nodes.find((item) => item.id === id)!;
  const chosen = () => {
    const id = selected();
    return id === null ? null : node(id);
  };
  const near = createMemo(() => {
    const id = selected();
    if (id === null) return null;
    return new Set([id, ...props.connections.edges.flatMap((edge) => (edge.from === id ? [edge.to] : edge.to === id ? [edge.from] : []))]);
  });
  const lit = (edge: ConnectionEdge) => selected() !== null && (edge.from === selected() || edge.to === selected());
  let heading: HTMLHeadingElement | undefined;
  let canvas: HTMLDivElement | undefined;
  let panelName: HTMLHeadingElement | undefined;

  /** Selects a node and brings it into view, at once. */
  function select(id: string | null) {
    setSelected(id);
    if (id === null) return;
    requestAnimationFrame(() => [...(canvas?.querySelectorAll("[data-node]") ?? [])].find((element) => element.getAttribute("data-node") === id)?.scrollIntoView?.({ block: "nearest", inline: "nearest" }));
  }
  /** j and k: the next or previous changed node in reading order, around the end; from nothing selected, the first or the last. */
  function step(by: 1 | -1) {
    const nodes = untrack(map).nodes;
    const from = nodes.findIndex((item) => item.id === untrack(selected));
    for (let offset = 1; offset <= nodes.length; offset += 1) {
      const index = from < 0 ? (by > 0 ? offset - 1 : nodes.length - offset) : (((from + by * offset) % nodes.length) + nodes.length) % nodes.length;
      if (nodes[index]!.change !== "unchanged") return select(nodes[index]!.id);
    }
  }

  onSettled(() => {
    heading?.focus();
    return listenToViewKeys({
      step,
      escape() {
        if (untrack(selected) !== null) setSelected(null);
        else untrack(() => props.onBack)();
      },
    });
  });

  const groups = (id: string) => [
    { name: "Called or used by", incoming: true, edges: props.connections.edges.filter((edge) => edge.to === id && edge.from !== id) },
    { name: "Calls or uses", incoming: false, edges: props.connections.edges.filter((edge) => edge.from === id) },
  ].filter((group) => group.edges.length > 0);

  return (
    <section class="full-view map-view" aria-labelledby="connections-heading">
      <ViewHead id="connections" detail="Callers are on the left." steps="step through the changed code" headingRef={(element) => { heading = element; }} onBack={() => props.onBack()} />
      <div class="map-body" data-open={chosen() ? "" : undefined}>
        <div class="map-main">
          <div
            class="map-canvas"
            ref={(element: HTMLDivElement) => { canvas = element; }}
            onClick={(event) => {
              if (!(event.target as Element).closest("[data-node]")) setSelected(null);
            }}
          >
            <svg class="map" viewBox={`0 0 ${map().width} ${map().height}`} width={map().width} height={map().height} role="group" aria-label="Map of the changed code">
              <defs>
                <marker id="map-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,0 L8,4 L0,8 z" fill="context-stroke" /></marker>
              </defs>
              <For each={props.connections.edges}>
                {(edge) => (
                  <path
                    class="map-edge"
                    data-found-by={edge.found_by}
                    data-lit={lit(edge) ? "" : undefined}
                    data-dim={selected() !== null && !lit(edge) ? "" : undefined}
                    d={edgePath(node(edge.from), node(edge.to))}
                    marker-end="url(#map-arrow)"
                    aria-hidden="true"
                  />
                )}
              </For>
              <For each={map().nodes}>
                {(item) => {
                  const toggle = () => select(selected() === item.id ? null : item.id);
                  return (
                    <g
                      class="map-node"
                      data-node={item.id}
                      data-change={item.change}
                      data-dim={near() && !near()!.has(item.id) ? "" : undefined}
                      role="button"
                      tabindex="0"
                      aria-pressed={selected() === item.id ? "true" : "false"}
                      aria-label={`${item.name}, ${KIND[item.kind][0]}, ${changeText(item).replace(" · ", ", ")}`}
                      onClick={toggle}
                      onKeyDown={(event: KeyboardEvent) => {
                        if (event.key !== "Enter" && event.key !== " ") return;
                        event.preventDefault();
                        toggle();
                      }}
                    >
                      <circle cx={item.x} cy={item.y} r={item.r} />
                      <text class="map-node-name" x={item.x} y={item.y + item.r + 15} text-anchor="middle">{cut(item.name)}</text>
                      <text class="map-node-change" x={item.x} y={item.y + item.r + 30} text-anchor="middle">{changeText(item)}</text>
                    </g>
                  );
                }}
              </For>
            </svg>
          </div>
          <ul class="map-legend" aria-label="What the map's marks mean">
            <li><i class="map-key" data-change="added" />added</li>
            <li><i class="map-key" data-change="changed" />changed</li>
            <li><i class="map-key" data-change="removed" />removed</li>
            <li><i class="map-key" data-change="unchanged" />unchanged caller</li>
            <li>larger circle: more lines changed</li>
            <li><i class="map-line" />calls or uses, found by gopls</li>
            <li><i class="map-line" data-found-by="name_search" />found by name search: may be a different function or type with the same name</li>
          </ul>
          <Show when={props.connections.limits.length > 0}>
            <ViewLimits id="connections" heading="What this map may miss" limits={props.connections.limits} />
          </Show>
        </div>
        <Show when={chosen()}>
          {(item) => (
            <aside class="map-panel" aria-label={`${item().name}: where it is and its links`}>
              <header class="map-panel-head">
                <h4 class="map-panel-name" tabindex="-1" ref={(element: HTMLHeadingElement) => { panelName = element; }}><code>{item().name}</code></h4>
                <IconButton label="Close" onClick={() => setSelected(null)}><CloseIcon /></IconButton>
              </header>
              <p class="map-panel-what">
                {KIND[item().kind][0]} · <span class="map-change" data-change={item().change}>{item().change}</span>
                <Show when={item().change !== "unchanged"}> · {plural(item().lines_changed, "line")} changed</Show>
              </p>
              <Show
                when={item().change !== "unchanged"}
                fallback={<p class="map-location"><code>{location(item())}</code> · unchanged, so the diff has none of its lines</p>}
              >
                <button type="button" class="map-location map-location-open" aria-expanded={codeOpen() ? "true" : "false"} onClick={() => setCodeOpen(!codeOpen())}>
                  {codeOpen() ? <ChevronDownIcon /> : <ChevronRightIcon />}
                  <code>{location(item())}</code>
                  <span class="walk-muted">{codeOpen() ? "Hide the diff" : "Open the diff at this line"}</span>
                </button>
                <Show when={codeOpen()}>
                  <CodeAt node={item()} files={files()} />
                </Show>
              </Show>
              <For each={groups(item().id)}>
                {(group) => (
                  <section class="map-links" aria-label={group.name}>
                    <h5>{group.name}</h5>
                    <ul>
                      <For each={group.edges}>
                        {(edge) => {
                          const other = () => node(group.incoming ? edge.from : edge.to);
                          const verb = edge.kind === "calls" ? "calls" : "uses";
                          const go = () => {
                            select(other().id);
                            requestAnimationFrame(() => panelName?.focus());
                          };
                          return (
                            <li data-found-by={edge.found_by}>
                              <p>
                                <Show
                                  when={group.incoming}
                                  fallback={<><code>{item().name}</code> {verb} <button type="button" class="walk-link" onClick={go}>{other().name}</button></>}
                                >
                                  <button type="button" class="walk-link" onClick={go}>{other().name}</button> {verb} <code>{item().name}</code>
                                </Show>
                              </p>
                              <p class="map-found">
                                <i class="map-line" data-found-by={edge.found_by} aria-hidden="true" />
                                {edge.found_by === "gopls"
                                  ? "found by gopls"
                                  : `found by name search, so ${node(edge.from).name} may ${verb === "calls" ? "call" : "use"} a different ${KIND[node(edge.to).kind][0]} named ${node(edge.to).name}`}
                              </p>
                            </li>
                          );
                        }}
                      </For>
                    </ul>
                  </section>
                )}
              </For>
            </aside>
          )}
        </Show>
      </div>
    </section>
  );
}

/** The diff of a node's file, opened at its line, or why it can't be. */
function CodeAt(props: { node: ConnectionNode; files: readonly FileDiffMetadata[] }) {
  const file = createMemo(() => props.files.find((item) => item.name === props.node.path || item.prevName === props.node.path) ?? null);
  const target = createMemo(() => {
    const found = file();
    return found ? openAt(found, props.node) : null;
  });
  return (
    <div class="map-code">
      <Show when={file()} fallback={<p class="walk-note">{props.node.path} isn't in the review's diff.</p>}>
        {(found) => (
          <>
            <Show when={target()?.start !== props.node.line}>
              <p class="walk-note">
                {target() ? `The diff doesn't show line ${props.node.line}; it opens at line ${target()!.start}, the next line it shows.` : `The diff doesn't show line ${props.node.line} or any line after it.`}
              </p>
            </Show>
            <Diff file={found()} layout="unified" target={target()} />
          </>
        )}
      </Show>
    </div>
  );
}
