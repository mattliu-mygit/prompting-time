import { FileDiff, type DiffLineAnnotation, type FileContents, type FileDiffMetadata, type FileDiffOptions } from "@pierre/diffs";
import { createEffect, createSignal, Errored, For, onSettled, untrack } from "solid-js";
import { Portal, type JSX } from "@solidjs/web";
import { DIFF_THEME, registerDiffTheme } from "./diffTheme";

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

  const options = (layout: "split" | "unified"): FileDiffOptions<T, undefined> => ({
    theme: DIFF_THEME,
    themeType: "dark",
    diffStyle: layout,
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
    onPostRender: () => setPlaced((map) => new Map([...map].filter(([, entry]) => entry.element.isConnected))),
  });

  // Drive Pierre from an effect, not onSettled: with Shiki loaded, Pierre draws annotations
  // synchronously, and Solid's dev build throws if that happens inside onSettled.
  createEffect(
    () => ({
      source: props.file ? { fileDiff: props.file } : { oldFile: props.oldFile, newFile: props.newFile },
      layout: props.layout ?? "split",
      annotations: props.annotations ?? [],
    }),
    ({ source, layout, annotations }) => {
      // setOptions replaces every option, so the callbacks above go in each time.
      if (instance) instance.setOptions(options(layout));
      else instance = new FileDiff<T>(options(layout));
      instance.render({ ...source, lineAnnotations: [...annotations], containerWrapper: wrapper, forceRender: true });
    },
  );
  onSettled(() => () => instance?.cleanUp());

  return (
    <div class="pt-diff">
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
