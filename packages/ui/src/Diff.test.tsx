import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { createSignal, onCleanup } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { DIFF_THEME, Diff, diffTheme, excerptFile, parsePatchFiles, type DiffLineAnnotation, type FileContents, type FileDiffMetadata } from "./index";
import { axeViolations, render } from "./test/render";

const oldFile: FileContents = { name: "score.go", contents: 'package score\n\n// Pass is the lowest passing score.\nconst Pass = 0.5\n\nfunc Name() string {\n\treturn "old"\n}\n' };
const newFile: FileContents = { name: "score.go", contents: 'package score\n\n// Pass is the lowest passing score.\nconst Pass = 0.6\n\nfunc Name() string {\n\treturn "new"\n}\n' };

interface Thread {
  id: string;
  text: string;
}
const nameThread: DiffLineAnnotation<Thread> = { side: "additions", lineNumber: 7, metadata: { id: "t1", text: "Say why the name changed." } };
const passThread: DiffLineAnnotation<Thread> = { side: "additions", lineNumber: 4, metadata: { id: "t0", text: "Why 0.6?" } };

function ThreadView(props: { annotation: () => DiffLineAnnotation<Thread>; onDispose?: (id: string) => void }) {
  onCleanup(() => props.onDispose?.(props.annotation().metadata.id));
  return (
    <label>
      <input type="checkbox" checked />
      Include {props.annotation().metadata.text}
    </label>
  );
}

const host = () => document.querySelector("diffs-container");
const slotOf = (text: string) => screen.getByRole("checkbox", { name: `Include ${text}` }).closest("[data-annotation-slot]");

describe("Diff", () => {
  it("draws each thread under its line, with code colored by Sigil's variables", async () => {
    render(() => (
      <Diff
        oldFile={oldFile}
        newFile={newFile}
        annotations={[nameThread]}
        annotationKey={(annotation) => annotation.metadata.id}
        renderAnnotation={(annotation) => <ThreadView annotation={annotation} />}
      />
    ));
    const checkbox = await screen.findByRole("checkbox", { name: "Include Say why the name changed." }, { timeout: 10_000 });
    expect(checkbox.closest("[data-annotation-slot]")).toHaveAttribute("slot", "annotation-additions-7");
    expect(checkbox.closest("diffs-container")).toBe(host());
    const styles = [...host()!.shadowRoot!.querySelectorAll("[style]")].map((element) => element.getAttribute("style"));
    expect(styles).toContain("color:var(--s-code-string-color)");
    expect(styles).toContain("color:var(--s-code-keyword-color)");
    expect(await axeViolations()).toEqual([]);
  });

  it("keeps a thread and its state when Pierre rebuilds the thread's element", async () => {
    const [annotations, setAnnotations] = createSignal([nameThread]);
    const disposed = vi.fn();
    render(() => (
      <Diff
        oldFile={oldFile}
        newFile={newFile}
        annotations={annotations()}
        annotationKey={(annotation) => annotation.metadata.id}
        renderAnnotation={(annotation) => <ThreadView annotation={annotation} onDispose={disposed} />}
      />
    ));
    const checkbox = await screen.findByRole("checkbox", { name: "Include Say why the name changed." }, { timeout: 10_000 });
    fireEvent.click(checkbox);
    const firstSlot = slotOf("Say why the name changed.");

    // A thread inserted above changes the first thread's index, so Pierre rebuilds its element.
    setAnnotations([passThread, nameThread]);
    await screen.findByRole("checkbox", { name: "Include Why 0.6?" });
    await waitFor(() => expect(firstSlot).not.toBeInTheDocument());
    expect(screen.getByRole("checkbox", { name: "Include Say why the name changed." })).toBe(checkbox);
    expect(checkbox).not.toBeChecked();
    expect(slotOf("Say why the name changed.")).toHaveAttribute("slot", "annotation-additions-7");
    expect(disposed).not.toHaveBeenCalled();

    setAnnotations([passThread]);
    await waitFor(() => expect(disposed).toHaveBeenCalledExactlyOnceWith("t1"));
    expect(screen.queryByRole("checkbox", { name: "Include Say why the name changed." })).not.toBeInTheDocument();
  });

  it("shows an error in place of a thread that throws and keeps the other threads", async () => {
    render(() => (
      <Diff
        oldFile={oldFile}
        newFile={newFile}
        annotations={[passThread, nameThread]}
        annotationKey={(annotation) => annotation.metadata.id}
        renderAnnotation={(annotation) => {
          if (annotation().metadata.id === "t0") throw new Error("bad draft");
          return <ThreadView annotation={annotation} />;
        }}
      />
    ));
    expect(await screen.findByText("This comment could not be shown: Error: bad draft", undefined, { timeout: 10_000 })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Include Say why the name changed." })).toBeChecked();
  });

  it("draws one file of a parsed patch, with threads under its lines", async () => {
    const patch = [
      "diff --git a/score.go b/score.go",
      "--- a/score.go",
      "+++ b/score.go",
      "@@ -3,6 +3,6 @@ package score",
      " // Pass is the lowest passing score.",
      "-const Pass = 0.5",
      "+const Pass = 0.6",
      " ",
      " func Name() string {",
      '-\treturn "old"',
      '+\treturn "new"',
      " }",
      "",
    ].join("\n");
    const [file] = parsePatchFiles(patch).flatMap((parsed) => parsed.files);
    render(() => (
      <Diff
        file={file!}
        layout="unified"
        annotations={[passThread]}
        annotationKey={(annotation) => annotation.metadata.id}
        renderAnnotation={(annotation) => <ThreadView annotation={annotation} />}
      />
    ));
    const checkbox = await screen.findByRole("checkbox", { name: "Include Why 0.6?" }, { timeout: 10_000 });
    expect(checkbox.closest("[data-annotation-slot]")).toHaveAttribute("slot", "annotation-additions-4");
    const styles = [...host()!.shadowRoot!.querySelectorAll("[style]")].map((element) => element.getAttribute("style"));
    expect(styles).toContain("color:var(--s-code-string-color)");
    expect(diffTheme.name).toBe(DIFF_THEME);
  });
});

/** An invented 30-line Go file's patch: line 4 changed near the top, and lines 24 and 25 near the end. */
const longPatch = [
  "diff --git a/limits.go b/limits.go",
  "--- a/limits.go",
  "+++ b/limits.go",
  "@@ -1,30 +1,31 @@",
  " package limits",
  " ",
  " // Max is the most one request may send.",
  "-const Max = 10",
  "+const Max = 20",
  ...Array.from({ length: 18 }, (_, index) => ` // filler ${index + 5}`),
  " func Check(n int) error {",
  " \tif n > Max {",
  "-\t\treturn errTooMany",
  "+\t\treturn fmt.Errorf(\"%d is over %d\", n, Max)",
  "+\t\t// The caller shows this.",
  " \t}",
  " \treturn nil",
  " }",
  " ",
  " // end",
  "",
].join("\n");
const longFile = () => parsePatchFiles(longPatch).flatMap((parsed) => parsed.files)[0]!;
const numbers = (file: FileDiffMetadata) => file.hunks.map((hunk) => [hunk.additionStart, hunk.additionCount]);

describe("excerptFile", () => {
  it("keeps the lines in each range with three lines around them, at their real line numbers", () => {
    const excerpt = excerptFile(longFile(), [{ start: 25, end: 25 }])!;
    // New lines 22 to 28: three before line 25, the change (one line out, two in), and three after.
    expect(numbers(excerpt)).toEqual([[22, 7]]);
    expect(excerpt.additionLines.join("")).toContain("is over");
    expect(excerpt.deletionLines.join("")).toContain("errTooMany");
  });

  it("makes one hunk per range far apart, and none for a range outside the patch", () => {
    expect(numbers(excerptFile(longFile(), [{ start: 4, end: 4 }, { start: 25, end: 26 }])!)).toEqual([[1, 7], [22, 8]]);
    expect(excerptFile(longFile(), [{ start: 200, end: 210 }])).toBeNull();
  });
});

describe("Diff's excerpts, marks, and target", () => {
  it("draws only the excerpt, marks its lines' numbers, and calls onMark for a click on one", async () => {
    const onMark = vi.fn();
    render(() => <Diff file={longFile()} layout="unified" excerpt={[{ start: 24, end: 26 }]} marks={[{ id: "place-1", label: "1", start: 25, end: 26 }]} onMark={onMark} />);
    const root = () => host()?.shadowRoot;
    await waitFor(() => expect(root()?.querySelector('[data-column-number="25"]')).not.toBeNull(), { timeout: 10_000 });
    expect(root()!.querySelector('[data-column-number="5"]')).toBeNull();
    const css = [...root()!.querySelectorAll("style")].map((style) => style.textContent).join("\n");
    expect(css).toContain('[data-column-number="25"]');
    expect(css).toContain('content:"1"');
    const cell = root()!.querySelector<HTMLElement>('[data-column-number="26"]:not([data-line-type="change-deletion"])')!;
    fireEvent.pointerDown(cell, { button: 0 });
    fireEvent.pointerUp(cell, { button: 0 });
    fireEvent.click(cell);
    expect(onMark).toHaveBeenCalledWith("place-1");
  });

  it("scrolls the target's first line into view, again for a new target object", async () => {
    const scrolled = vi.fn();
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled(this.getAttribute("data-column-number"));
    };
    const [target, setTarget] = createSignal({ start: 25, end: 26 });
    render(() => <Diff file={longFile()} layout="unified" target={target()} />);
    await waitFor(() => expect(scrolled).toHaveBeenCalledWith("25"), { timeout: 10_000 });
    setTarget({ start: 4, end: 4 });
    await waitFor(() => expect(scrolled).toHaveBeenLastCalledWith("4"));
    // @ts-expect-error jsdom has no scrollIntoView; the test put one in for itself.
    delete Element.prototype.scrollIntoView;
  });
});
