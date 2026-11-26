import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { createSignal, onCleanup } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { Diff, type DiffLineAnnotation, type FileContents } from "./index";
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
});
