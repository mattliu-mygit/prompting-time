import { createEffect, createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import { CheckIcon, ChevronsLeftIcon, ChevronsRightIcon, CloseIcon, Diff, IconButton, Markdown, parsePatchFiles, type DiffLineAnnotation, type DiffMark, type FileDiffMetadata, type LineRange } from "@prompting-time/ui";
import type { PluginActions } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import type { ReviewEntry } from "../agents/reviews";
import { DecideBar } from "./DecideBar";
import { FlowDiagram } from "./FlowDiagram";
import { plural } from "./ReviewTop";
import type { CodeRange, Finding, Layer, Overview, Place, ShownReview } from "./shown";
import { viewHasData, viewName, type ReviewViews } from "./views";
import type { JSX } from "@solidjs/web";

type Props = {
  plugin: PluginSnapshot;
  review: ReviewEntry;
  shown: ShownReview & { overview: Overview };
  views: ReviewViews;
  actions: PluginActions;
  /** The verdict banner and the since notice, which head the walkthrough. */
  top: JSX.Element;
  /** Whether the walkthrough shows, so j and k step only then. */
  active: boolean;
  /** Whether the user may decide here: the newest version, not yet submitted. */
  canDecide: boolean;
  onShowDiff(): void;
  onSubmitted(runId: string): void;
};

/** A place to look with its number, as the list, the navigator, and the gutter call it. */
type PlaceEntry = { place: Place; number: number };
/** One file of a layer: its patch file, the lines the layer shows, and the places to look in them. */
type LayerFile = { path: string; file: FileDiffMetadata | null; ranges: LineRange[]; places: PlaceEntry[] };
type LayerEntry = { layer: Layer; number: number; files: LayerFile[] };
type Note = { kind: "place"; entry: PlaceEntry } | { kind: "draft"; finding: Finding; included: boolean; text: string };

const HINT: Record<Layer["read_hint"], string> = { read: "read closely", skim: "skim", skip: "skip" };
const EVIDENCE: Record<Place["evidence"]["kind"], string> = { ran: "Ran it", read: "Read, not run", searched: "Searched the repo" };
const rangeText = (range: CodeRange) => `${range.path.split("/").at(-1)}:${range.start_line}${range.end_line > range.start_line ? `–${range.end_line}` : ""}`;
const overlaps = (range: CodeRange, other: CodeRange) => range.path === other.path && range.start_line <= other.end_line && range.end_line >= other.start_line;

// Kept for the session, per run, so leaving a review and coming back keeps them: the layers you
// marked read, whether the decision bar opened, and whether the navigator is collapsed.
const readLayers = new Map<string, ReadonlySet<string>>();
const decided = new Set<string>();
const [navCollapsed, setNavCollapsed] = createSignal(false);

/** Text with `code` spans, as the plugin writes identifiers in its prose. */
function Inline(props: { text: string }) {
  const parts = () => props.text.split("`").map((text, index) => ({ text, code: index % 2 === 1 }));
  return <For each={parts()}>{(part) => (part.code ? <code class="walk-code">{part.text}</code> : part.text)}</For>;
}

/** A place's reason and evidence, under its lines in the code. */
function PlaceCard(props: { entry: PlaceEntry; onClose?: () => void }) {
  return (
    <div class="place-card">
      <p class="place-card-head">
        <span class="place-number">{props.entry.number}</span>
        <span class="place-title">{props.entry.place.title}</span>
        <Show when={props.onClose}>{(close) => <IconButton label="Close" onClick={() => close()()}><CloseIcon /></IconButton>}</Show>
      </p>
      <p class="place-reason"><Inline text={props.entry.place.reason} /></p>
      <p class="place-evidence">{EVIDENCE[props.entry.place.evidence.kind]} · <Inline text={props.entry.place.evidence.detail} /></p>
    </div>
  );
}

/** What sits under a line of the walkthrough's code: an open place's reason, or one of Whooper's draft comments. */
function NoteCard(props: { note: Note; onClose(): void }) {
  return (
    <Show
      when={props.note.kind === "place" ? props.note : null}
      fallback={
        <div class="draft-card" data-included={(props.note as Extract<Note, { kind: "draft" }>).included ? "" : undefined}>
          <p class="draft-card-head">Whooper's draft comment · {(props.note as Extract<Note, { kind: "draft" }>).included ? "included" : "left out"}</p>
          <Markdown content={(props.note as Extract<Note, { kind: "draft" }>).text} />
        </div>
      }
    >
      {(note) => <PlaceCard entry={(note() as Extract<Note, { kind: "place" }>).entry} onClose={props.onClose} />}
    </Show>
  );
}

/**
 * A review's walkthrough, the plugin's overview of the change read top to bottom: what the pull
 * request does, before and after; the places to look, each a numbered mark in the code below whose
 * reason and evidence open on a click or with j and k; the calls before and after; what the plugin
 * checked and didn't; then the change layer by layer, each with its line excerpts and a read check
 * that folds it. A navigator on the left lists the parts, layers, and places, and folds to a rail.
 * Each view's summary sits where its question comes up, when the view is on and has data. The
 * decision waits in the corner until you click it or reach the end.
 */
export function Walkthrough(props: Props) {
  const runId = untrack(() => props.review.run_id);
  const overview = () => props.shown.overview;
  const patchFiles = createMemo(() => (props.shown.patch ? parsePatchFiles(props.shown.patch).flatMap((parsed) => parsed.files) : []));
  const places = createMemo((): PlaceEntry[] => overview().places.map((place, index) => ({ place, number: index + 1 })));

  /** The layers with their files, and a last "Other places to look" for places outside every layer's lines. */
  const layers = createMemo((): LayerEntry[] => {
    const covered = (entry: PlaceEntry) => overview().layers.some((layer) => layer.ranges.some((range) => overlaps(range, entry.place)));
    const others = places().filter((entry) => !covered(entry));
    const all: Layer[] = [
      ...overview().layers,
      ...(others.length > 0 ? [{ id: "other-places", title: "Other places to look", intent: "The places to look outside the layers above.", read_hint: "read" as const, ranges: others.map((entry) => entry.place) }] : []),
    ];
    return all.map((layer, index) => ({
      layer,
      number: index + 1,
      files: [...new Set(layer.ranges.map((range) => range.path))].map((path) => {
        const ranges = layer.ranges.filter((range) => range.path === path);
        return {
          path,
          file: patchFiles().find((file) => file.name === path || file.prevName === path) ?? null,
          ranges: ranges.map((range) => ({ start: range.start_line, end: range.end_line })),
          places: places().filter((entry) => ranges.some((range) => overlaps(range, entry.place))),
        };
      }),
    }));
  });
  /** Where each place's lines show: the first layer file that holds them. */
  const homeOf = (id: string) => layers().flatMap((entry) => entry.files.map((file) => ({ entry, file }))).find(({ file }) => file.places.some((item) => item.place.id === id));

  const [read, setReadValue] = createSignal<ReadonlySet<string>>(readLayers.get(runId) ?? new Set());
  const [reopened, setReopened] = createSignal<ReadonlySet<string>>(new Set());
  const [current, setCurrent] = createSignal(-1);
  const [openPlace, setOpenPlace] = createSignal<string | null>(null);
  const [target, setTarget] = createSignal<{ path: string; range: LineRange } | null>(null);
  const [dimmed, setDimmed] = createSignal(false);
  const [decideOpen, setDecideOpen] = createSignal(decided.has(runId));
  let scroller: HTMLDivElement | undefined;

  const isRead = (id: string) => read().has(id);
  const folded = (layer: Layer) => (isRead(layer.id) || layer.read_hint === "skip") && !reopened().has(layer.id);
  const reopen = (id: string, open: boolean) => setReopened((now) => (open ? new Set([...now, id]) : new Set([...now].filter((item) => item !== id))));
  function toggleRead(id: string) {
    setReadValue((now) => {
      const next = new Set(now);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      readLayers.set(runId, next);
      return next;
    });
    // A layer you finish folds; its one line stays where you are reading.
    reopen(id, false);
    requestAnimationFrame(() => scroller?.querySelector<HTMLElement>(`[data-layer="${id}"]`)?.scrollIntoView?.({ block: "nearest" }));
  }
  const openDecide = () => {
    decided.add(runId);
    setDecideOpen(true);
  };

  /** Opens place `index`: its layer unfolds, its lines scroll into view, and its reason opens under them. */
  function look(index: number) {
    const entry = untrack(places)[index];
    if (!entry) return;
    const home = untrack(() => homeOf(entry.place.id));
    if (home) reopen(home.entry.layer.id, true);
    setCurrent(index);
    setOpenPlace(entry.place.id);
    setTarget({ path: entry.place.path, range: { start: entry.place.start_line, end: entry.place.end_line } });
  }
  const step = (by: number) => {
    const count = untrack(places).length;
    if (count === 0) return;
    const now = untrack(current);
    look(now < 0 ? (by > 0 ? 0 : count - 1) : (now + by + count) % count);
  };
  /** A click on a mark in the gutter opens its reason there, or closes it. */
  const toggleMark = (id: string) => {
    const index = untrack(places).findIndex((entry) => entry.place.id === id);
    if (untrack(openPlace) === id) setOpenPlace(null);
    else {
      setCurrent(index);
      setOpenPlace(id);
    }
  };
  /** A call in the flow diagram opens its line where a layer shows it. */
  function openLine(path: string, line: number) {
    const home = untrack(layers).flatMap((entry) => entry.files.map((file) => ({ entry, file }))).find(({ file }) => file.path === path && file.ranges.some((range) => line >= range.start - 3 && line <= range.end + 3));
    if (home) reopen(home.entry.layer.id, true);
    setTarget({ path, range: { start: line, end: line } });
  }

  // Dimming while stepping also dims the window around the review.
  createEffect(() => dimmed() && props.active, (on) => {
    document.body.classList.toggle("review-dimming", on);
    return () => document.body.classList.remove("review-dimming");
  });

  onSettled(() => {
    // Where the pointer was when j or k dimmed the rest, so the scroll a step makes doesn't undim it.
    let pointer = { x: -1, y: -1 };
    let dimmedAt = { x: -1, y: -1 };
    const typing = (event: KeyboardEvent) => event.target instanceof HTMLElement && (event.target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName));
    const keys = (event: KeyboardEvent) => {
      if (!untrack(() => props.active) || event.defaultPrevented) return;
      if (event.key === "Escape" && (untrack(dimmed) || untrack(openPlace))) {
        setDimmed(false);
        setOpenPlace(null);
        return;
      }
      if (typing(event) || event.metaKey || event.ctrlKey || event.altKey || (event.key !== "j" && event.key !== "k")) return;
      if (document.querySelector('[role="dialog"], [role="menu"]:not([hidden])')?.checkVisibility?.()) return;
      event.preventDefault();
      step(event.key === "j" ? 1 : -1);
      dimmedAt = pointer;
      setDimmed(true);
    };
    const moved = (event: MouseEvent) => {
      pointer = { x: event.clientX, y: event.clientY };
      if (untrack(dimmed) && Math.abs(pointer.x - dimmedAt.x) + Math.abs(pointer.y - dimmedAt.y) > 3) setDimmed(false);
    };
    const undim = () => {
      if (untrack(dimmed)) setDimmed(false);
    };
    document.addEventListener("keydown", keys);
    window.addEventListener("mousemove", moved);
    window.addEventListener("wheel", undim, { passive: true });
    window.addEventListener("pointerdown", undim);
    return () => {
      document.removeEventListener("keydown", keys);
      window.removeEventListener("mousemove", moved);
      window.removeEventListener("wheel", undim);
      window.removeEventListener("pointerdown", undim);
    };
  });

  const scrollTo = (selector: string) => scroller?.querySelector<HTMLElement>(selector)?.scrollIntoView?.({ block: "start" });
  const showCalls = () => props.views.isOn("calls") && viewHasData("calls", props.shown);
  const showCoChange = () => props.views.isOn("co-change") && viewHasData("co-change", props.shown);
  const parts = () => [
    { id: "verdict", name: "Verdict" },
    { id: "what", name: "What this PR does" },
    { id: "places", name: "Places to look", count: places().length },
    ...(showCalls() ? [{ id: "calls", name: viewName("calls") }] : []),
    { id: "checked", name: "What Whooper checked" },
  ];
  const fileCount = () => new Set(layers().flatMap((entry) => entry.files.map((file) => file.path))).size;
  /** One file of a layer: its excerpts, with the places' marks, the open place's reason, and Whooper's draft comments. */
  function LayerFileView(fileProps: { file: LayerFile }) {
    const side = () => (fileProps.file.file?.type === "deleted" ? "deletions" : "additions");
    const marks = createMemo((): DiffMark[] => fileProps.file.places.map((entry) => ({ id: entry.place.id, label: String(entry.number), start: entry.place.start_line, end: entry.place.end_line })));
    const placeNotes = createMemo(() => new Map(fileProps.file.places.map((entry): [string, DiffLineAnnotation<Note>] => [entry.place.id, { side: side(), lineNumber: entry.place.end_line, metadata: { kind: "place", entry } }])));
    const draftNotes = createMemo(() => props.shown.findings
      // Only in the layer whose lines hold it, not in another's context lines, so each shows once.
      .filter((finding) => finding.path === fileProps.file.path && fileProps.file.ranges.some((range) => finding.line >= range.start && finding.line <= range.end))
      .map((finding): DiffLineAnnotation<Note> => {
        const choice = props.shown.drafts?.findings[finding.id];
        return { side: finding.side === "LEFT" ? "deletions" : "additions", lineNumber: finding.line, metadata: { kind: "draft", finding, included: choice?.included ?? false, text: choice?.text ?? finding.draft } };
      }));
    const annotations = () => {
      const open = openPlace();
      const note = open ? placeNotes().get(open) : undefined;
      return note ? [...draftNotes(), note] : draftNotes();
    };
    const targeted = createMemo(() => {
      const now = target();
      return now && now.path === fileProps.file.path && fileProps.file.ranges.some((range) => now.range.start <= range.end + 3 && now.range.end >= range.start - 3) ? now : null;
    });
    return (
      <div class="walk-file" data-target={targeted() ? "" : undefined}>
        <Show when={fileProps.file.file} fallback={<p class="walk-note">{fileProps.file.path} isn't in the review's diff.</p>}>
          {(file) => (
            <Diff<Note>
              file={file()}
              layout="unified"
              excerpt={fileProps.file.ranges}
              marks={marks()}
              onMark={toggleMark}
              target={targeted()?.range ?? null}
              annotations={annotations()}
              annotationKey={(annotation) => (annotation.metadata.kind === "place" ? `place:${annotation.metadata.entry.place.id}` : `draft:${annotation.metadata.finding.id}`)}
              renderAnnotation={(annotation) => <NoteCard note={annotation().metadata} onClose={() => setOpenPlace(null)} />}
            />
          )}
        </Show>
      </div>
    );
  }

  return (
    <div class="walkthrough" data-rail={navCollapsed() ? "" : undefined} data-dim={dimmed() ? "" : undefined}>
      <nav class="walk-nav" aria-label="Walkthrough">
        <Show
          when={!navCollapsed()}
          fallback={
            <button type="button" class="walk-rail" aria-expanded="false" title="Open the walkthrough's navigator" onClick={() => setNavCollapsed(false)}>
              <ChevronsRightIcon />
              <span class="walk-rail-label">Walkthrough</span>
              <span class="walk-ticks" aria-hidden="true">
                <For each={layers()}>{(entry) => <span class="walk-tick" data-read={isRead(entry.layer.id) ? "" : undefined} />}</For>
                <span class="walk-ticks-gap" />
                <For each={places()}>{(entry) => <span class="walk-tick" data-kind="place" data-current={current() === entry.number - 1 ? "" : undefined} />}</For>
              </span>
            </button>
          }
        >
          <p class="walk-nav-heading">
            Walkthrough
            <IconButton class="walk-nav-collapse" label="Collapse the navigator" onClick={() => setNavCollapsed(true)}><ChevronsLeftIcon /></IconButton>
          </p>
          <ul class="walk-nav-parts">
            <For each={parts()}>
              {(part) => (
                <li>
                  <button type="button" onClick={() => scrollTo(`[data-part="${part.id}"]`)}>
                    <span>{part.name}</span>
                    <Show when={part.count !== undefined}><span class="walk-count">{part.count}</span></Show>
                  </button>
                </li>
              )}
            </For>
          </ul>
          <p class="walk-nav-heading">Layers <span class="walk-count">{layers().filter((entry) => isRead(entry.layer.id)).length} of {layers().length} read</span></p>
          <ul class="walk-nav-layers">
            <For each={layers()}>
              {(entry) => (
                <li>
                  <button type="button" class="walk-check" aria-pressed={isRead(entry.layer.id) ? "true" : "false"} aria-label={`Layer ${entry.number} read`} onClick={() => toggleRead(entry.layer.id)}><CheckIcon /></button>
                  <button
                    type="button"
                    class="walk-nav-layer"
                    onClick={() => {
                      reopen(entry.layer.id, true);
                      requestAnimationFrame(() => scrollTo(`[data-layer="${entry.layer.id}"]`));
                    }}
                  >
                    <span>{entry.number} · {entry.layer.title}</span>
                    <span class="walk-count">{plural(entry.files.length, "file")}{entry.layer.read_hint === "read" ? "" : ` · ${HINT[entry.layer.read_hint]}`}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
          <p class="walk-nav-heading">
            Places to look <span class="walk-count">{current() < 0 ? places().length : `${current() + 1} of ${places().length}`}</span>
            <span class="walk-keys" title="Next and previous place to look"><kbd>j</kbd><kbd>k</kbd></span>
          </p>
          <ol class="walk-nav-places">
            <For each={places()}>
              {(entry) => (
                <li>
                  <button type="button" aria-current={current() === entry.number - 1 ? "true" : undefined} onClick={() => look(entry.number - 1)}>
                    <span class="place-number">{entry.number}</span>
                    <span class="walk-nav-place-text">{entry.place.title}<span class="walk-count">{rangeText(entry.place)}</span></span>
                  </button>
                </li>
              )}
            </For>
          </ol>
        </Show>
      </nav>
      <div class="walk-main">
        <div
          class="walk-scroll"
          ref={(element: HTMLDivElement) => { scroller = element; }}
          onScroll={(event) => {
            const box = event.currentTarget;
            if (props.canDecide && !untrack(decideOpen) && box.scrollTop + box.clientHeight >= box.scrollHeight - 4) openDecide();
          }}
        >
          <div class="walk-column">
            <div data-part="verdict">{props.top}</div>
            <section class="walk-section walk-prose" data-part="what" aria-labelledby={`what-${runId}`}>
              <h3 id={`what-${runId}`}>What this PR does</h3>
              <p><Inline text={overview().summary} /></p>
              <Show when={overview().before_after.length > 0}>
                <table class="walk-before-after">
                  <thead><tr><th scope="col"><span class="pt-visually-hidden">What</span></th><th scope="col">Before</th><th scope="col">After</th></tr></thead>
                  <tbody>
                    <For each={overview().before_after}>
                      {(row) => <tr><th scope="row">{row.label}</th><td><Inline text={row.before} /></td><td><Inline text={row.after} /></td></tr>}
                    </For>
                  </tbody>
                </table>
              </Show>
            </section>
            <section class="walk-section walk-prose" data-part="places" aria-labelledby={`places-${runId}`}>
              <h3 id={`places-${runId}`}>Places to look <span class="walk-count">{places().length} · each is a numbered mark in the code below · j and k step through them</span></h3>
              <ol class="walk-places">
                <For each={places()}>
                  {(entry) => (
                    <li data-current={current() === entry.number - 1 ? "" : undefined}>
                      <span class="place-number">{entry.number}</span>
                      <div>
                        <p class="place-title">{entry.place.title}</p>
                        <p class="place-reason"><Inline text={entry.place.reason} /></p>
                        <p class="place-evidence">{EVIDENCE[entry.place.evidence.kind]} · <Inline text={entry.place.evidence.detail} /></p>
                      </div>
                      <button type="button" class="walk-link" aria-label={`Show place ${entry.number} in the code, ${rangeText(entry.place)}`} onClick={() => look(entry.number - 1)}>{rangeText(entry.place)}</button>
                    </li>
                  )}
                </For>
              </ol>
            </section>
            <Show when={showCalls() ? overview().flow : null}>
              {(flow) => (
                <section class="walk-section walk-view" data-part="calls" aria-labelledby={`calls-${runId}`}>
                  <h3 id={`calls-${runId}`}>{viewName("calls")} <span class="walk-count">{flow().title} · click a call to see its line below</span></h3>
                  <FlowDiagram flow={flow()} onOpen={openLine} />
                </section>
              )}
            </Show>
            <section class="walk-section walk-prose" data-part="checked" aria-labelledby={`checked-${runId}`}>
              <h3 id={`checked-${runId}`}>What Whooper checked and didn't</h3>
              <div class="walk-checks">
                <div>
                  <h4>Checked</h4>
                  <ul><For each={overview().checked}>{(item) => <li><CheckIcon /><span><Inline text={item} /></span></li>}</For></ul>
                </div>
                <div>
                  <h4>Didn't check</h4>
                  <ul data-kind="not"><For each={overview().not_checked}>{(item) => <li><span aria-hidden="true">–</span><span><Inline text={item} /></span></li>}</For></ul>
                </div>
              </div>
              <Show when={showCoChange()}>
                <div class="walk-view-summary" aria-label={viewName("co-change")}>
                  <h4>{viewName("co-change")}</h4>
                  <p>In git history, {plural(props.shown.coChange.length, "file")} usually {props.shown.coChange.length === 1 ? "changes" : "change"} with this PR's files, but this PR leaves {props.shown.coChange.length === 1 ? "it" : "them"} alone:</p>
                  <ul>
                    <For each={props.shown.coChange}>
                      {(item) => <li><code>{item.path}</code> <span class="walk-muted">changed with {item.partner.split("/").at(-1)} in {Math.round(item.ratio * 100)}% of {plural(item.commits, "commit")}</span></li>}
                    </For>
                  </ul>
                </div>
              </Show>
            </section>
            <p class="walk-divider"><span>The change, layer by layer</span><span class="walk-muted">{plural(fileCount(), "file")} in {plural(layers().length, "layer")}</span></p>
            <For each={layers()}>
              {(entry) => (
                <section class="walk-layer" data-layer={entry.layer.id} data-folded={folded(entry.layer) ? "" : undefined} aria-label={`Layer ${entry.number}: ${entry.layer.title}`}>
                  <Show
                    when={!folded(entry.layer)}
                    fallback={
                      <button type="button" class="walk-folded" aria-expanded="false" onClick={() => reopen(entry.layer.id, true)}>
                        <Show when={isRead(entry.layer.id)}><span class="walk-folded-check"><CheckIcon /></span></Show>
                        <span class="walk-eyebrow">Layer {entry.number} of {layers().length}</span>
                        <span class="walk-folded-title">{entry.layer.title}</span>
                        <span class="walk-muted">{plural(entry.files.length, "file")} · {isRead(entry.layer.id) ? "read" : HINT[entry.layer.read_hint]}</span>
                      </button>
                    }
                  >
                    <header class="walk-layer-head walk-prose">
                      <p class="walk-eyebrow">Layer {entry.number} of {layers().length} · {HINT[entry.layer.read_hint]}</p>
                      <h3>{entry.layer.title}</h3>
                      <p><Inline text={entry.layer.intent} /></p>
                    </header>
                    <For each={entry.files}>{(file) => <LayerFileView file={file} />}</For>
                    <div class="walk-layer-end walk-prose">
                      <button type="button" class="pt-button" onClick={() => toggleRead(entry.layer.id)}>
                        {isRead(entry.layer.id) ? `Layer ${entry.number} read · undo` : `Mark layer ${entry.number} read`}
                      </button>
                    </div>
                  </Show>
                </section>
              )}
            </For>
          </div>
        </div>
        <Show when={props.canDecide}>
          <DecideBar
            plugin={props.plugin}
            review={props.review}
            shown={props.shown}
            actions={props.actions}
            open={decideOpen()}
            onOpen={openDecide}
            onEditDrafts={() => props.onShowDiff()}
            onSubmitted={props.onSubmitted}
          />
        </Show>
      </div>
    </div>
  );
}
