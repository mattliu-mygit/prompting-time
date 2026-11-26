import { parsePatchFiles, type FileDiffMetadata } from "@pierre/diffs";

/**
 * A run of lines in a file, first and last inclusive: the new file's line numbers, or the old file's
 * for a deleted file, which has no new lines.
 */
export interface LineRange {
  start: number;
  end: number;
}

/** How many unchanged lines an excerpt keeps around each range, as `git diff -U3` does around a change. */
const CONTEXT = 3;

type PatchLine = { mark: " " | "-" | "+"; old: number | null; new: number | null; text: string };

/** One hunk's lines in order, each with its old and new line number. */
function hunkLines(file: FileDiffMetadata, hunk: FileDiffMetadata["hunks"][number]): PatchLine[] {
  const lines: PatchLine[] = [];
  const text = (line: string | undefined) => (line ?? "").replace(/\r?\n$/, "");
  let oldNumber = hunk.deletionStart;
  let newNumber = hunk.additionStart;
  for (const content of hunk.hunkContent) {
    if (content.type === "context") {
      for (let index = 0; index < content.lines; index += 1) {
        lines.push({ mark: " ", old: oldNumber++, new: newNumber++, text: text(file.additionLines[content.additionLineIndex + index]) });
      }
    } else {
      for (let index = 0; index < content.deletions; index += 1) {
        lines.push({ mark: "-", old: oldNumber++, new: null, text: text(file.deletionLines[content.deletionLineIndex + index]) });
      }
      for (let index = 0; index < content.additions; index += 1) {
        lines.push({ mark: "+", old: null, new: newNumber++, text: text(file.additionLines[content.additionLineIndex + index]) });
      }
    }
  }
  return lines;
}

/** "@@ -3,2 +3,4 @@": where a hunk's lines start in each file and how many each has; a side with none starts at the line before. */
function hunkHeader(lines: readonly PatchLine[]): string {
  const side = (key: "old" | "new", mark: "-" | "+") => {
    const numbers = lines.filter((line) => line.mark !== (mark === "-" ? "+" : "-")).map((line) => line[key]!);
    if (numbers.length > 0) return `${numbers[0]},${numbers.length}`;
    // A side with no lines in the hunk starts at the line before the hunk's position, as git writes it.
    const after = lines.find((line) => line[key] !== null)?.[key];
    return `${Math.max(0, (after ?? 1) - 1)},0`;
  };
  return `@@ -${side("old", "-")} +${side("new", "+")} @@`;
}

/**
 * Line excerpts of one file of a parsed patch: only the patch's lines inside `ranges`, each with up
 * to three unchanged lines around it, as a file of its own that Pierre draws with the file's real
 * line numbers. A removed line belongs to the excerpt when the new line after it, or the one before
 * it, does, so a change shows whole. Returns null when no line of the patch falls in the ranges.
 *
 * It writes the kept lines as a unified diff and parses that with Pierre's own parser, so the result
 * is exactly what Pierre would make of a patch holding only those lines.
 */
export function excerptFile(file: FileDiffMetadata, ranges: readonly LineRange[]): FileDiffMetadata | null {
  // A deleted file has only old lines, so its ranges count old lines.
  const key: "old" | "new" = file.type === "deleted" ? "old" : "new";
  const near = (number: number | null) => number !== null && ranges.some((range) => number >= range.start - CONTEXT && number <= range.end + CONTEXT);
  const hunks: PatchLine[][] = [];
  for (const hunk of file.hunks) {
    const lines = hunkLines(file, hunk);
    let run: PatchLine[] = [];
    lines.forEach((line, index) => {
      // A line of the other side has no number on this one: it goes with its neighbors on this side.
      const keep = line[key] !== null
        ? near(line[key])
        : near(lines.slice(index + 1).find((next) => next[key] !== null)?.[key] ?? null) || near(lines.slice(0, index).reverse().find((previous) => previous[key] !== null)?.[key] ?? null);
      if (keep) run.push(line);
      else if (run.length > 0) {
        hunks.push(run);
        run = [];
      }
    });
    if (run.length > 0) hunks.push(run);
  }
  if (hunks.length === 0) return null;
  const oldName = file.type === "new" ? "/dev/null" : `a/${file.prevName ?? file.name}`;
  const newName = file.type === "deleted" ? "/dev/null" : `b/${file.name}`;
  const patch = [
    `diff --git a/${file.prevName ?? file.name} b/${file.name}`,
    `--- ${oldName}`,
    `+++ ${newName}`,
    ...hunks.flatMap((lines) => [hunkHeader(lines), ...lines.map((line) => `${line.mark}${line.text}`)]),
    "",
  ].join("\n");
  return parsePatchFiles(patch).flatMap((parsed) => parsed.files)[0] ?? null;
}
