import { createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import { Button, Diff, parsePatchFiles, type DiffMark, type LineRange } from "@prompting-time/ui";
import { listenToViewKeys, ViewHead, ViewLimits } from "./FullView";
import { Inline, plural, series, Series } from "./ReviewTop";
import type { Place, TestGapCounts, TestGapLine, TestGaps } from "./shown";
import { viewName } from "./views";

/**
 * The statuses of a changed line, in the format's words: no test ran it; tests ran it, but none
 * failed when the plugin changed it on purpose; or one did. A later format may add others.
 */
const NOT_RUN = "not_run";
const SURVIVED = "survived";
const CAUGHT = "caught";
const KNOWN = [NOT_RUN, SURVIVED, CAUGHT];

/** A line tests ran that the plugin changed on purpose, with that change in words. */
type ChangedLine = TestGapLine & { change: string };

/** The lines with `status`, "survived" or "caught"; readTestGaps keeps such a line only with its change. */
const changedLines = (lines: readonly TestGapLine[], status: typeof SURVIVED | typeof CAUGHT) =>
  lines.filter((line): line is ChangedLine => line.status === status && line.change !== null);

/** Runs of consecutive line numbers, in order: 39, 40, 41, and 50 make 39–41 and 50. */
function runsOf(lines: readonly number[]): LineRange[] {
  const runs: LineRange[] = [];
  for (const line of [...new Set(lines)].sort((left, right) => left - right)) {
    const last = runs.at(-1);
    if (last && line === last.end + 1) last.end = line;
    else runs.push({ start: line, end: line });
  }
  return runs;
}

/** "39, 40" or "39–42, 50": line numbers in order, with a run of three or more as a range. */
export function lineList(lines: readonly number[]): string {
  return runsOf(lines).flatMap((run) => (
    run.end - run.start >= 2 ? [`${run.start}–${run.end}`] : run.end > run.start ? [String(run.start), String(run.end)] : [String(run.start)]
  )).join(", ");
}

/** Whether a line is among a place's lines. */
export const inPlace = (line: TestGapLine, place: Place) => line.path === place.path && line.line >= place.start_line && line.line <= place.end_line;

const byLine = (left: TestGapLine, right: TestGapLine) => left.line - right.line;

/** The lines by file, the files in the order the plugin lists them. */
function byFile<T extends TestGapLine>(lines: readonly T[]): Array<{ path: string; lines: T[] }> {
  const files = new Map<string, T[]>();
  for (const line of lines) files.set(line.path, [...(files.get(line.path) ?? []), line]);
  return [...files].map(([path, inFile]) => ({ path, lines: [...inFile].sort(byLine) }));
}

/** What the plugin found at a place to look: its changed lines no test ran, and its changes no test noticed. */
export type PlaceGapLines = { notRun: number[]; survived: ChangedLine[]; tool: string };

/** What a run that measured found among a place's lines; null when it couldn't measure, or found neither kind of line there. */
export function gapsAt(gaps: TestGaps, place: Place): PlaceGapLines | null {
  if (!gaps.measured) return null;
  const here = gaps.lines.filter((line) => inPlace(line, place)).sort(byLine);
  const notRun = here.filter((line) => line.status === NOT_RUN).map((line) => line.line);
  const survived = changedLines(here, SURVIVED);
  return notRun.length > 0 || survived.length > 0 ? { notRun, survived, tool: gaps.tool } : null;
}

/**
 * "Tests ran 3 of 21 changed lines, and no test ran 4.", with the lines the run counted as neither,
 * and, when the plugin says, how many of the changes it made on purpose a test caught, naming the
 * plugin by its title: "Tests caught 5 of 7 changes Whooper made on purpose."
 */
export function countsText(counts: TestGapCounts, pluginTitle: string): string {
  const rest = counts.changed_lines - counts.run - counts.not_run;
  const changes = counts.survived !== null && counts.caught !== null ? counts.survived + counts.caught : 0;
  return `Tests ran ${counts.run} of ${plural(counts.changed_lines, "changed line")}${counts.not_run > 0 ? `, and no test ran ${counts.not_run}` : ""}.`
    + (rest > 0 ? ` The run counted the other ${rest} as neither, such as comments.` : "")
    + (changes > 0 ? ` Tests caught ${counts.caught} of ${plural(changes, "change")} ${pluginTitle} made on purpose.` : "");
}

/** The runs of the tests the plugin measured with, each in code type, "a and b"; `tool` joins several with "; ". */
function Tool(props: { tool: string }) {
  const runs = () => props.tool.split(/;\s+/).map((run) => run.trim()).filter((run) => run !== "");
  return <Series items={runs().map((run) => ({ text: run, code: true }))} />;
}

/** How a finding of lines no test ran is known: the plugin ran the tests with `tool` and measured it. */
function MeasuredNotRun(props: { count: number; tool: string }) {
  return (
    <p class="place-evidence">
      Measured: no test ran {props.count === 1 ? "this line" : "these lines"}
      <Show when={props.tool}> · <Tool tool={props.tool} /></Show>
    </p>
  );
}

/** A change made on purpose that no test noticed, naming the plugin by its title: "Tests ran line 39 but none noticed when Whooper changed `>` to `>=`". */
function Unnoticed(props: { line: ChangedLine; pluginTitle: string }) {
  return <p>Tests ran line {props.line.line} but none noticed when {props.pluginTitle} changed <Inline text={props.line.change} /></p>;
}

/** How a finding of changes no test noticed is known: the plugin changed each line on purpose and ran the tests again. */
function MeasuredUnnoticed(props: { count: number }) {
  return <p class="place-evidence">Measured: no test noticed {props.count === 1 ? "this change" : "these changes"}</p>;
}

/** At a place to look: the changed lines among its lines that no test ran, the changes there no test noticed, and how each is known. */
export function PlaceGaps(props: { gaps: PlaceGapLines; pluginTitle: string }) {
  return (
    <div class="place-gaps">
      <Show when={props.gaps.notRun.length > 0}>
        <div>
          <p>No test ran {plural(props.gaps.notRun.length, "changed line")} here: {lineList(props.gaps.notRun)}</p>
          <MeasuredNotRun count={props.gaps.notRun.length} tool={props.gaps.tool} />
        </div>
      </Show>
      <Show when={props.gaps.survived.length > 0}>
        <div>
          <For each={props.gaps.survived}>{(line) => <Unnoticed line={line} pluginTitle={props.pluginTitle} />}</For>
          <MeasuredUnnoticed count={props.gaps.survived.length} />
        </div>
      </Show>
    </div>
  );
}

/**
 * Changes no test would catch, after the places to look, each of which names its own lines: how
 * many changed lines the tests ran and how many of the changes made on purpose they caught, which
 * are only counted; the lines no test ran and the changes no test noticed outside the places; and
 * Open all lines, which shows every line by file in the walkthrough's place. When the plugin
 * couldn't measure, one quiet line says why.
 */
export function TestGapsBlock(props: { testGaps: TestGaps; places: readonly Place[]; pluginTitle: string; headingId: string; onOpen(button: HTMLButtonElement): void }) {
  const outside = (line: TestGapLine) => !props.places.some((place) => inPlace(line, place));
  const notRun = createMemo(() => props.testGaps.lines.filter((line) => line.status === NOT_RUN && outside(line)));
  const survived = createMemo(() => byFile(changedLines(props.testGaps.lines, SURVIVED).filter(outside)).flatMap((file) => file.lines));
  const hasPlaces = () => props.places.length > 0;
  return (
    <Show
      when={props.testGaps.measured}
      fallback={
        <p class="walk-view-quiet">
          <span class="walk-view-name">{viewName("test-gaps")}</span>
          {props.testGaps.reason ? `Not measured: ${props.testGaps.reason}` : "Not measured."}
        </p>
      }
    >
      <section class="walk-view-summary" aria-labelledby={props.headingId}>
        <div class="walk-view-head">
          <h4 id={props.headingId}>{viewName("test-gaps")}</h4>
          <Show when={props.testGaps.lines.length > 0}>
            <Button onClick={(event) => props.onOpen(event.currentTarget)}>Open all lines</Button>
          </Show>
        </div>
        <p>{countsText(props.testGaps.summary, props.pluginTitle)}</p>
        <Show when={notRun().length > 0}>
          <div class="walk-view-group">
            <p>{hasPlaces() ? "Outside the places to look, no test ran" : "No test ran"} {plural(notRun().length, "changed line")}:</p>
            <ul>
              <For each={byFile(notRun())}>
                {(file) => <li><code>{file.path}</code> <span class="walk-muted">{lineList(file.lines.map((line) => line.line))}</span></li>}
              </For>
            </ul>
            <MeasuredNotRun count={notRun().length} tool={props.testGaps.tool} />
          </div>
        </Show>
        <Show when={survived().length > 0}>
          <div class="walk-view-group">
            <p>{hasPlaces() ? "Outside the places to look, tests ran" : "Tests ran"} {plural(survived().length, "changed line")} but none noticed when {props.pluginTitle} changed {survived().length === 1 ? "it" : "them"}:</p>
            <ul>
              <For each={survived()}>
                {(line) => <li><code>{line.path}</code> <span class="walk-muted">{line.line}:</span> <Inline text={line.change} /></li>}
              </For>
            </ul>
            <MeasuredUnnoticed count={survived().length} />
          </div>
        </Show>
        <Show when={notRun().length === 0 && survived().length === 0}>
          <p class="place-evidence">Measured<Show when={props.testGaps.tool}> · <Tool tool={props.testGaps.tool} /></Show></p>
        </Show>
      </section>
    </Show>
  );
}

/** "line 42 (timed_out)" or "lines 42 (timed_out) and 50 (skipped)": lines whose status this version doesn't know, with each status as the plugin wrote it. */
const othersText = (lines: readonly TestGapLine[]) => `${lines.length === 1 ? "line" : "lines"} ${series(lines.map((line) => `${line.line} (${line.status})`))}`;

/**
 * Changes no test would catch, in full, in the walkthrough's place: what the plugin measured with,
 * how many changed lines the tests ran, and how many of the changes it made on purpose they caught,
 * in words; then each file with its lines by what the runs found, each kind with how it is known:
 * the changed lines no test ran, the changes no test noticed, and the changes tests caught, with
 * the test when the plugin names it; then the file's diff cut to those lines, with the lines no test
 * ran and the changes no test noticed marked in orange in the gutter. A line with a status this
 * version doesn't know says so, with the plugin's word, and gets no mark. A run that couldn't
 * measure shows why instead, and no lines. Last, what the measure may miss. j and k step through
 * the files; Escape clears the current file, then goes back. Nothing moves on a key.
 */
export function TestGapsView(props: { testGaps: TestGaps; patch: string | null; pluginTitle: string; onBack(): void }) {
  const patchFiles = createMemo(() => (props.patch ? parsePatchFiles(props.patch).flatMap((parsed) => parsed.files) : []));
  const files = createMemo(() => byFile(props.testGaps.lines).map((group) => {
    const notRun = group.lines.filter((line) => line.status === NOT_RUN).map((line) => line.line);
    const survived = changedLines(group.lines, SURVIVED);
    const marked = [...runsOf(notRun), ...survived.map((line) => ({ start: line.line, end: line.line }))];
    return {
      path: group.path,
      notRun,
      survived,
      caught: changedLines(group.lines, CAUGHT),
      others: group.lines.filter((line) => !KNOWN.includes(line.status)),
      patchFile: patchFiles().find((file) => file.name === group.path || file.prevName === group.path) ?? null,
      excerpt: runsOf(group.lines.map((line) => line.line)),
      marks: marked.map((run): DiffMark => ({ id: `${group.path}:${run.start}`, label: "", ...run })),
    };
  }));
  const [current, setCurrent] = createSignal(-1);
  let heading: HTMLHeadingElement | undefined;
  let column: HTMLDivElement | undefined;

  /** j and k: the next or previous file, around the end, its top brought into view at once. */
  function step(by: 1 | -1) {
    const count = untrack(() => props.testGaps.measured) ? untrack(files).length : 0;
    if (count === 0) return;
    const now = untrack(current);
    const next = now < 0 ? (by > 0 ? 0 : count - 1) : (now + by + count) % count;
    setCurrent(next);
    column?.querySelectorAll<HTMLElement>(".gaps-file")[next]?.scrollIntoView?.({ block: "start" });
  }

  onSettled(() => {
    heading?.focus();
    return listenToViewKeys({
      step,
      escape() {
        if (untrack(current) >= 0) setCurrent(-1);
        else untrack(() => props.onBack)();
      },
    });
  });

  return (
    <section class="full-view" aria-labelledby="test-gaps-heading">
      <ViewHead id="test-gaps" steps="step through the files" headingRef={(element) => { heading = element; }} onBack={() => props.onBack()} />
      <div class="full-view-scroll">
        <div class="full-view-column" ref={(element: HTMLDivElement) => { column = element; }}>
          <Show
            when={props.testGaps.measured}
            fallback={<p class="gaps-counts">{props.testGaps.reason ? `Not measured: ${props.testGaps.reason}` : "Not measured."}</p>}
          >
            <p class="gaps-counts">
              <Show when={props.testGaps.tool}>Measured with <Tool tool={props.testGaps.tool} />. </Show>
              {countsText(props.testGaps.summary, props.pluginTitle)}
            </p>
            <For each={files()}>
              {(file, index) => (
                <section class="gaps-file" data-current={current() === index() ? "" : undefined} aria-labelledby={`gaps-file-${index()}`}>
                  <h4 id={`gaps-file-${index()}`}><code>{file.path}</code></h4>
                  <Show when={file.notRun.length > 0}>
                    <div class="gaps-group">
                      <p>No test ran {plural(file.notRun.length, "changed line")}: {lineList(file.notRun)}</p>
                      <MeasuredNotRun count={file.notRun.length} tool={props.testGaps.tool} />
                    </div>
                  </Show>
                  <Show when={file.survived.length > 0}>
                    <div class="gaps-group">
                      <For each={file.survived}>{(line) => <Unnoticed line={line} pluginTitle={props.pluginTitle} />}</For>
                      <MeasuredUnnoticed count={file.survived.length} />
                    </div>
                  </Show>
                  <Show when={file.caught.length > 0}>
                    <div class="gaps-group gaps-caught">
                      <p>Tests caught {file.caught.length === 1 ? "this change" : "these changes"} {props.pluginTitle} made on purpose:</p>
                      <ul>
                        <For each={file.caught}>
                          {(line) => (
                            <li>
                              Line {line.line}: <Inline text={line.change} />
                              <Show when={line.test}>{(test) => <>, caught by <code>{test()}</code></>}</Show>
                            </li>
                          )}
                        </For>
                      </ul>
                    </div>
                  </Show>
                  <Show when={file.others.length > 0}>
                    <p class="walk-note">The run also marked {othersText(file.others)}, which this version of Prompting Time can't explain.</p>
                  </Show>
                  <Show when={file.patchFile} fallback={<p class="walk-note">{file.path} isn't in the review's diff.</p>}>
                    {(patchFile) => <Diff file={patchFile()} layout="unified" excerpt={file.excerpt} marks={file.marks} />}
                  </Show>
                </section>
              )}
            </For>
          </Show>
          <Show when={props.testGaps.limits.length > 0}>
            <ViewLimits id="test-gaps" heading="What this measure may miss" limits={props.testGaps.limits} />
          </Show>
        </div>
      </div>
    </section>
  );
}
