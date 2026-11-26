import { FileDiff, type DiffLineAnnotation, type FileContents, type FileDiffMetadata, type FileDiffOptions } from "@pierre/diffs";
import { createEffect, createMemo, createSignal, Errored, For, onSettled, Show, untrack } from "solid-js";
import { Portal, type JSX } from "@solidjs/web";
import { DIFF_THEME, registerDiffTheme } from "./diffTheme";
import { excerptFile, type LineRange } from "./excerpt";

/** What Diff draws: a file's old and new contents, or one file of a parsed patch. */
export type DiffSource =
  | { oldFile: FileContents; newFile: FileContents; file?: never }
  | {
      /**
       * One file of a unified diff, from parsePatchFiles, for a page that keeps only the patch,
       * such as a review's diff.patch. Only the patch's hunks show; there is no context to expand.
       */
      file: FileDiffMetadata;
      oldFile?: never;
      newFile?: never;
    };

export type DiffProps<T> = DiffSource & DiffOptions<T>;

/**
 * A mark in the line-number gutter over a range of lines, such as a place a review asks you to look
 * at: the numbers take the warning color, the first carries `label`, and a click on any of them
 * calls `onMark` with `id`. Lines count as in `LineRange`.
 */
export interface DiffMark extends LineRange {
  id: string;
  label: string;
}

interface DiffOptions<T> {
  /** Side by side ("split", the default) or in one column ("unified"). */
  layout?: "split" | "unified";
  /**
   * Comment threads, each under one line. Keep each annotation's metadata object the same while
   * its content is the same: Pierre rebuilds an annotation whose metadata is a new object.
   */
  annotations?: readonly DiffLineAnnotation<T>[];
  /** A stable key for an annotation, such as its thread id. A thread keeps its state while its key stays. */
  annotationKey?: (annotation: DiffLineAnnotation<T>) => string;
  /** Draws one thread, once per key. The accessor returns the annotation's latest version. */
  renderAnnotation?: (annotation: () => DiffLineAnnotation<T>) => JSX.Element;
  /**
   * For a file of a patch, only these lines, each with up to three unchanged lines around it: line
   * excerpts, such as the lines a walkthrough shows for one step (see excerptFile).
   */
  excerpt?: readonly LineRange[];
  /** Marks in the line-number gutter; a click on a marked line's number calls `onMark`. */
  marks?: readonly DiffMark[];
  onMark?: (id: string) => void;
  /** Lines to scroll into view and highlight. A new object scrolls again; null highlights nothing. */
  target?: LineRange | null;
}

/** The gutter cell of line `line`, in one column or split: the new file's, or the old file's for a deleted file. */
function numberCell(line: number, deleted: boolean): string {
  const cell = `[data-column-number="${line}"]`;
  return deleted
    ? `:is([data-unified], [data-deletions]) ${cell}[data-line-type="change-deletion"]`
    : `:is([data-unified] ${cell}:not([data-line-type="change-deletion"]), [data-additions] ${cell})`;
}

/** The code row of line `line`, as numberCell picks its gutter cell. */
function codeRow(line: number, deleted: boolean): string {
  const row = `[data-line="${line}"]`;
  return deleted
    ? `:is([data-unified], [data-deletions]) ${row}[data-line-type="change-deletion"]`
    : `:is([data-unified] ${row}:not([data-line-type="change-deletion"]), [data-additions] ${row})`;
}

const linesOf = (range: LineRange) => Array.from({ length: Math.max(0, range.end - range.start + 1) }, (_, index) => range.start + index);

/**
 * The styles Pierre adds inside its Shadow DOM for the marks and the target. Pierre's unsafeCSS is
 * the only way in, and it promises no compatibility, even in patch releases: these rules lean on
 * its data-column-number, data-line, and data-line-type attributes, which Diff.test checks.
 */
function markStyles(marks: readonly DiffMark[], target: LineRange | null, deleted: boolean): string {
  const rules = marks.flatMap((mark) => [
    `${linesOf(mark).map((line) => numberCell(line, deleted)).join(",")}{color:var(--s-warning);background-color:color-mix(in oklch,var(--s-warning) 16%,transparent);box-shadow:inset 2px 0 0 var(--s-warning);cursor:pointer}`,
    `${numberCell(mark.start, deleted)}::before{content:${JSON.stringify(mark.label)};margin-inline-end:auto;padding-inline:4px;font-weight:600}`,
  ]);
  if (target) rules.push(`${linesOf(target).map((line) => codeRow(line, deleted)).join(",")}{background-color:color-mix(in oklch,var(--s-text) 9%,var(--diffs-bg))}`);
  return rules.join("\n");
}

/** Where Pierre placed one annotation: the element under its line that the thread is drawn into. */
interface Placed<T> {
  annotation: DiffLineAnnotation<T>;
  element: HTMLElement;
}

/**
 * A file diff drawn by @pierre/diffs, from whole files or from one file of a patch, with comment threads drawn by this component's own Solid
 * tree. Pierre asks for an element per annotation and may rebuild it, for example when a thread is
 * inserted above it; each thread is a Portal keyed by `annotationKey`, so the same thread, with its
 * half-typed draft and checkboxes, moves into the new element. Each thread has its own error
 * boundary, because an uncaught error in Solid 2.0 stops every update in the app.
 */
export function Diff<T>(props: DiffProps<T>) {
  registerDiffTheme();
  let wrapper!: HTMLDivElement;
  let instance: FileDiff<T> | undefined;
  const [placed, setPlaced] = createSignal<ReadonlyMap<string, Placed<T>>>(new Map());
  /** The patch file Pierre draws: the excerpt's lines when there is an excerpt, or null when none of its lines are in the patch. */
  const file = createMemo(() => (props.file && props.excerpt ? excerptFile(props.file, props.excerpt) : props.file ?? null));
  const deleted = () => props.file?.type === "deleted";
  /** The target to scroll to once Pierre has drawn its lines, which may wait for Shiki. */
  let pending: LineRange | null = null;

  function scrollToPending() {
    const cell = pending && wrapper.querySelector("diffs-container")?.shadowRoot?.querySelector(numberCell(pending.start, untrack(deleted)));
    if (!cell) return;
    pending = null;
    cell.scrollIntoView?.({ block: "center" });
  }

  const options = (layout: "split" | "unified", css: string): FileDiffOptions<T, undefined> => ({
    theme: DIFF_THEME,
    themeType: "dark",
    diffStyle: layout,
    unsafeCSS: css,
    onLineNumberClick: ({ lineNumber, annotationSide }) => {
      if ((annotationSide === "deletions") !== untrack(deleted)) return;
      const mark = untrack(() => props.marks)?.find((item) => lineNumber >= item.start && lineNumber <= item.end);
      if (mark) untrack(() => props.onMark)?.(mark.id);
    },
    // Pierre calls this from the effect below or, while Shiki loads, from its own callback. The
    // element starts empty; the <For> below draws the thread into it.
    renderAnnotation: (annotation) => {
      const element = document.createElement("div");
      const key = props.annotationKey?.(annotation) ?? `${annotation.side}:${annotation.lineNumber}`;
      setPlaced((map) => new Map(map).set(key, { annotation, element }));
      return element;
    },
    // Pierre removes a stale annotation's element without saying so, so after each render drop the
    // threads whose element has left the page.
    onPostRender: () => {
      setPlaced((map) => new Map([...map].filter(([, entry]) => entry.element.isConnected)));
      scrollToPending();
    },
  });

  // Drive Pierre from an effect, not onSettled: with Shiki loaded, Pierre draws annotations
  // synchronously, and Solid's dev build throws if that happens inside onSettled.
  createEffect(
    () => ({
      // A patch file none of whose lines are in the excerpt draws nothing.
      source: props.file ? (file() ? { fileDiff: file()! } : null) : { oldFile: props.oldFile, newFile: props.newFile },
      layout: props.layout ?? "split",
      annotations: props.annotations ?? [],
      css: markStyles(props.marks ?? [], props.target ?? null, deleted()),
    }),
    ({ source, layout, annotations, css }) => {
      if (!source) return;
      // setOptions replaces every option, so the callbacks above go in each time.
      if (instance) instance.setOptions(options(layout, css));
      else instance = new FileDiff<T>(options(layout, css));
      instance.render({ ...source, lineAnnotations: [...annotations], containerWrapper: wrapper, forceRender: true });
    },
  );
  // After the render above, which the same change runs first; Pierre's onPostRender tries again once Shiki draws.
  createEffect(() => props.target, (target) => {
    pending = target ?? null;
    scrollToPending();
  });
  onSettled(() => () => instance?.cleanUp());

  return (
    <div class="pt-diff">
      <Show when={props.file && !file()}>
        <p class="pt-diff-empty">None of these lines are in the patch.</p>
      </Show>
      <div ref={wrapper} />
      <For each={[...placed()]} keyed={([key]) => key}>
        {(entry) => (
          <Portal mount={entry()[1].element}>
            <Errored fallback={(error) => <p class="pt-diff-thread-error">This comment could not be shown: {String(error())}</p>}>
              {untrack(() => props.renderAnnotation?.(() => entry()[1].annotation))}
            </Errored>
          </Portal>
        )}
      </For>
    </div>
  );
}
