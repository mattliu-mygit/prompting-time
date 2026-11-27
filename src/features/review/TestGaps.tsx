import { createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import { Button, Diff, parsePatchFiles, type DiffMark, type LineRange } from "@prompting-time/ui";
import { listenToViewKeys, ViewHead, ViewLimits } from "./FullView";
import { plural, series } from "./ReviewTop";
import type { Place, TestGapCounts, TestGapLine, TestGaps } from "./shown";
import { viewName } from "./views";

/** The status of a changed line no test ran, in the format's words; a later format may add others. */
const NOT_RUN = "not_run";

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

/** The changed lines no test ran. */
export const notRunLines = (gaps: TestGaps) => gaps.lines.filter((line) => line.status === NOT_RUN);

/** Whether a line is among a place's lines. */
export const inPlace = (line: TestGapLine, place: Place) => line.path === place.path && line.line >= place.start_line && line.line <= place.end_line;

/** The lines by file, the files in the order the plugin lists them. */
function byFile(lines: readonly TestGapLine[]): Array<{ path: string; lines: TestGapLine[] }> {
  const files = new Map<string, TestGapLine[]>();
  for (const line of lines) files.set(line.path, [...(files.get(line.path) ?? []), line]);
  return [...files].map(([path, inFile]) => ({ path, lines: [...inFile].sort((left, right) => left.line - right.line) }));
}

/** "Tests ran 3 of 21 changed lines, and no test ran 4.", with the lines the run counted as neither. */
export function countsText(counts: TestGapCounts): string {
  const rest = counts.changed_lines - counts.run - counts.not_run;
  return `Tests ran ${counts.run} of ${plural(counts.changed_lines, "changed line")}${counts.not_run > 0 ? `, and no test ran ${counts.not_run}` : ""}.`
    + (rest > 0 ? ` The run counted the other ${rest} as neither, such as comments.` : "");
}

/** How a finding of lines no test ran is known: the plugin ran the tests with `tool` and measured it. */
function Measured(props: { count: number; tool: string }) {
  return (
    <p class="place-evidence">
      Measured: no test ran {props.count === 1 ? "this line" : "these lines"}
      <Show when={props.tool}> · <code>{props.tool}</code></Show>
    </p>
  );
}

/** At a place to look: the changed lines among its lines that no test ran, and how that is known. */
export function PlaceGaps(props: { lines: readonly number[]; tool: string }) {
  return (
    <div class="place-gaps">
      <p>No test ran {plural(props.lines.length, "changed line")} here: {lineList(props.lines)}</p>
      <Measured count={props.lines.length} tool={props.tool} />
    </div>
  );
}

/**
 * Changes no test would catch, after the places to look, each of which names its own lines: how
 * many changed lines the tests ran, the lines no test ran outside the places, and Open all lines,
 * which shows every line by file in the walkthrough's place. When the plugin couldn't measure, one
 * quiet line says why.
 */
export function TestGapsBlock(props: { testGaps: TestGaps; elsewhere: readonly TestGapLine[]; hasPlaces: boolean; headingId: string; onOpen(button: HTMLButtonElement): void }) {
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
        <p>{countsText(props.testGaps.summary)}</p>
        <Show
          when={props.elsewhere.length > 0}
          fallback={<p class="place-evidence">Measured<Show when={props.testGaps.tool}> · <code>{props.testGaps.tool}</code></Show></p>}
        >
          <p>{props.hasPlaces ? "Outside the places to look, no test ran" : "No test ran"} {plural(props.elsewhere.length, "changed line")}:</p>
          <ul>
            <For each={byFile(props.elsewhere)}>
              {(file) => <li><code>{file.path}</code> <span class="walk-muted">{lineList(file.lines.map((line) => line.line))}</span></li>}
            </For>
          </ul>
          <Measured count={props.elsewhere.length} tool={props.testGaps.tool} />
        </Show>
      </section>
    </Show>
  );
}

/** "line 42 (survived)" or "lines 42 (survived) and 50 (caught)": lines whose status this version doesn't know, with each status as the plugin wrote it. */
const othersText = (lines: readonly TestGapLine[]) => `${lines.length === 1 ? "line" : "lines"} ${series(lines.map((line) => `${line.line} (${line.status})`))}`;

/**
 * Changes no test would catch, in full, in the walkthrough's place: what the plugin measured with,
 * and how many changed lines the tests ran, in words; then each file with its changed lines no test
 * ran, how that is known, and its diff cut to those lines, marked in orange in the gutter. A line
 * with a status this version doesn't know says so, with the plugin's word, and gets no mark. A run
 * that couldn't measure shows why instead, and no lines. Last, what the measure may miss. j and k step through the files; Escape clears the current file, then
 * goes back. Nothing moves on a key.
 */
export function TestGapsView(props: { testGaps: TestGaps; patch: string | null; onBack(): void }) {
  const patchFiles = createMemo(() => (props.patch ? parsePatchFiles(props.patch).flatMap((parsed) => parsed.files) : []));
  const files = createMemo(() => byFile(props.testGaps.lines).map((group) => {
    const notRun = group.lines.filter((line) => line.status === NOT_RUN);
    const others = group.lines.filter((line) => line.status !== NOT_RUN);
    const runs = runsOf(notRun.map((line) => line.line));
    return {
      path: group.path,
      notRun: notRun.map((line) => line.line),
      others,
      patchFile: patchFiles().find((file) => file.name === group.path || file.prevName === group.path) ?? null,
      excerpt: [...runs, ...others.map((line) => ({ start: line.line, end: line.line }))],
      marks: runs.map((run): DiffMark => ({ id: `${group.path}:${run.start}`, label: "", ...run })),
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
              <Show when={props.testGaps.tool}>Measured with <code>{props.testGaps.tool}</code>. </Show>
              {countsText(props.testGaps.summary)}
            </p>
            <For each={files()}>
              {(file, index) => (
                <section class="gaps-file" data-current={current() === index() ? "" : undefined} aria-labelledby={`gaps-file-${index()}`}>
                  <h4 id={`gaps-file-${index()}`}><code>{file.path}</code></h4>
                  <Show when={file.notRun.length > 0}>
                    <p>No test ran {plural(file.notRun.length, "changed line")}: {lineList(file.notRun)}</p>
                    <Measured count={file.notRun.length} tool={props.testGaps.tool} />
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
