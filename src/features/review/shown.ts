import { readShown } from "../agents/reviews";

/**
 * What a review's walkthrough reads from the show-review action, beyond what the page reads: the
 * plugin's overview of the change, the commits after the one the review read, and the files that
 * usually change with the changed ones. Each part that does not fit the format reads as missing, so
 * a plugin that prints none of it gets the page as before.
 */

/** How the plugin knows a place to look matters: it ran something, read the code, or searched the repo. */
export type Evidence = { kind: "ran" | "read" | "searched"; detail: string };

/** A run of lines in one file of the patch: the new file's lines, or the old file's for a deleted file. */
export type CodeRange = { path: string; start_line: number; end_line: number };

/** A place the plugin asks you to look at, with why, how it knows, and its lines. */
export type Place = CodeRange & { id: string; title: string; reason: string; evidence: Evidence };

/** One layer of the change, read in order, with how closely to read it and its lines. */
export type Layer = { id: string; title: string; intent: string; read_hint: "read" | "skim" | "skip"; ranges: CodeRange[] };

/** One call of the flow: made before and after the change ("same"), only after ("added"), only before ("removed"), or with a new label ("changed", old_label says what it was). */
export type FlowCall = { from: string; to: string; label: string; status: "same" | "added" | "removed" | "changed"; old_label: string | null; path: string | null; line: number | null };

/** The calls one input makes, before and after the change, overlaid. */
export type Flow = { title: string; input: string; calls: FlowCall[] };

export type Overview = {
  summary: string;
  verdict_reason: string;
  before_after: Array<{ label: string; before: string; after: string }>;
  places: Place[];
  layers: Layer[];
  flow: Flow | null;
  checked: string[];
  not_checked: string[];
};

/** A commit on the pull request after the one the review read. */
export type Commit = { sha: string; title: string; author: string | null; at: string; files: string[] };

/** A file that usually changes with one of the changed files (`partner`) but that this change leaves alone. */
export type CoChange = { path: string; partner: string; ratio: number; commits: number };

/** One of the review's draft comments, as the plugin's findings list it. */
export type Finding = { id: string; path: string; line: number; side: "LEFT" | "RIGHT"; title: string; draft: string; kind: string };

/** What the user chose so far: approve or comment, and which comments go out, with any edited text. */
export type Drafts = { event: "APPROVE" | "COMMENT"; message: string; findings: Record<string, { included: boolean; text?: string }> };

export type ShownReview = {
  overview: Overview | null;
  /** null when the plugin doesn't say; empty when the pull request has no commits after the reviewed one. */
  commitsSinceReview: Commit[] | null;
  coChange: CoChange[];
  findings: Finding[];
  /** The plugin's own reason for its verdict, from its findings. */
  reason: string | null;
  drafts: Drafts | null;
  /** The pull request's change as the review read it, a unified diff. */
  patch: string | null;
};

type Json = Record<string, unknown>;
const isRecord = (value: unknown): value is Json => typeof value === "object" && value !== null && !Array.isArray(value);
const isString = (value: unknown): value is string => typeof value === "string";
const isLine = (value: unknown): value is number => typeof value === "number" && Number.isInteger(value) && value > 0;
/** The entries of an array that fit, each read by `read`; anything else reads as no entries. */
const listOf = <T>(value: unknown, read: (item: unknown) => T | null): T[] => (Array.isArray(value) ? value.flatMap((item) => read(item) ?? []) : []);

function readRange(value: unknown): CodeRange | null {
  if (!isRecord(value) || !isString(value.path) || !isLine(value.start_line) || !isLine(value.end_line) || value.end_line < value.start_line) return null;
  return { path: value.path, start_line: value.start_line, end_line: value.end_line };
}

function readPlace(value: unknown): Place | null {
  const range = readRange(value);
  if (!range || !isRecord(value) || !isString(value.id) || !isString(value.title) || !isString(value.reason)) return null;
  const evidence = value.evidence;
  if (!isRecord(evidence) || !(evidence.kind === "ran" || evidence.kind === "read" || evidence.kind === "searched") || !isString(evidence.detail)) return null;
  return { ...range, id: value.id, title: value.title, reason: value.reason, evidence: { kind: evidence.kind, detail: evidence.detail } };
}

function readLayer(value: unknown): Layer | null {
  if (!isRecord(value) || !isString(value.id) || !isString(value.title) || !isString(value.intent)) return null;
  if (!(value.read_hint === "read" || value.read_hint === "skim" || value.read_hint === "skip")) return null;
  return { id: value.id, title: value.title, intent: value.intent, read_hint: value.read_hint, ranges: listOf(value.ranges, readRange) };
}

function readCall(value: unknown): FlowCall | null {
  if (!isRecord(value) || !isString(value.from) || !isString(value.to) || !isString(value.label)) return null;
  if (!(value.status === "same" || value.status === "added" || value.status === "removed" || value.status === "changed")) return null;
  const located = isString(value.path) && isLine(value.line);
  return {
    from: value.from, to: value.to, label: value.label, status: value.status,
    old_label: isString(value.old_label) ? value.old_label : null,
    path: located ? (value.path as string) : null,
    line: located ? (value.line as number) : null,
  };
}

function readFlow(value: unknown): Flow | null {
  if (!isRecord(value) || !isString(value.title) || !isString(value.input)) return null;
  const calls = listOf(value.calls, readCall);
  return calls.length > 0 ? { title: value.title, input: value.input, calls } : null;
}

/** The overview the plugin printed, or null when it printed none or one without its summary and reason. */
export function readOverview(value: unknown): Overview | null {
  if (!isRecord(value) || !isString(value.summary) || !isString(value.verdict_reason)) return null;
  return {
    summary: value.summary,
    verdict_reason: value.verdict_reason,
    before_after: listOf(value.before_after, (row) => (isRecord(row) && isString(row.label) && isString(row.before) && isString(row.after) ? { label: row.label, before: row.before, after: row.after } : null)),
    places: listOf(value.places, readPlace),
    layers: listOf(value.layers, readLayer),
    flow: readFlow(value.flow),
    checked: listOf(value.checked, (item) => (isString(item) ? item : null)),
    not_checked: listOf(value.not_checked, (item) => (isString(item) ? item : null)),
  };
}

function readCommit(value: unknown): Commit | null {
  if (!isRecord(value) || !isString(value.sha) || !isString(value.title) || !isString(value.at)) return null;
  return { sha: value.sha, title: value.title, author: isString(value.author) ? value.author : null, at: value.at, files: listOf(value.files, (file) => (isString(file) ? file : null)) };
}

function readFinding(value: unknown): Finding | null {
  if (!isRecord(value) || !isString(value.id) || !isString(value.path) || !isLine(value.line) || !isString(value.title) || !isString(value.draft)) return null;
  return { id: value.id, path: value.path, line: value.line, side: value.side === "LEFT" ? "LEFT" : "RIGHT", title: value.title, draft: value.draft, kind: isString(value.kind) ? value.kind : "other" };
}

function readDrafts(value: unknown): Drafts | null {
  if (!isRecord(value) || !(value.event === "APPROVE" || value.event === "COMMENT") || !isRecord(value.findings)) return null;
  const findings = Object.fromEntries(Object.entries(value.findings).flatMap(([id, choice]) => (
    isRecord(choice) && typeof choice.included === "boolean" ? [[id, { included: choice.included, ...(isString(choice.text) ? { text: choice.text } : {}) }]] : []
  )));
  return { event: value.event, message: isString(value.message) ? value.message : "", findings };
}

/** What show-review printed for run `runId` that the walkthrough reads; throws when it isn't that run in format version 1. */
export function readShownReview(stdout: string, runId: string): ShownReview {
  const shown = readShown(stdout, runId);
  const findings = isRecord(shown.findings) ? shown.findings : null;
  const approval = findings && isRecord(findings.approval) ? findings.approval : null;
  return {
    overview: readOverview(shown.overview),
    commitsSinceReview: Array.isArray(shown.commits_since_review) ? listOf(shown.commits_since_review, readCommit) : null,
    coChange: listOf(shown.co_change, (item) => (
      isRecord(item) && isString(item.path) && isString(item.partner) && typeof item.ratio === "number" && typeof item.commits === "number"
        ? { path: item.path, partner: item.partner, ratio: item.ratio, commits: item.commits } : null
    )),
    findings: findings ? listOf(findings.findings, readFinding) : [],
    reason: approval && isString(approval.reason) ? approval.reason : null,
    drafts: readDrafts(shown.drafts),
    patch: isString(shown.diff) ? shown.diff : null,
  };
}
