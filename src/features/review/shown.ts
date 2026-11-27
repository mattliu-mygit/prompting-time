import { readShown } from "../agents/reviews";

/**
 * What a review's walkthrough reads from the show-review action, beyond what the page reads: the
 * plugin's overview of the change, the commits after the one the review read, the files that
 * usually change with the changed ones, how the changed code connects, the changed lines no test
 * ran and the changes made on purpose that no test noticed, and example inputs run through the old
 * and new code. Each part that does not fit the format reads as missing, so a plugin that prints
 * none of it gets the page as before.
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

/** A function, method, type, variable, or constant in the map of the changed code, and what this change did to it. */
export type ConnectionNode = {
  id: string;
  name: string;
  kind: "func" | "method" | "type" | "var" | "const";
  path: string;
  /** Where it starts: the new file's line, or the old file's for one the change removed, since the new file has none. */
  line: number;
  change: "added" | "changed" | "removed" | "unchanged";
  /** How many of its lines changed, as the plugin counts them. */
  lines_changed: number;
};

/**
 * One node calls or uses another. gopls, Go's language server, found it, or a search for the
 * name did, which may have matched a different function or type with the same name.
 */
export type ConnectionEdge = { from: string; to: string; kind: "calls" | "uses"; found_by: "gopls" | "name_search" };

/** The functions and types the change touches, the code that calls or uses them, and what the plugin's map may miss. */
export type Connections = { nodes: ConnectionNode[]; edges: ConnectionEdge[]; limits: string[] };

/**
 * A changed line and what the plugin's runs of the tests found for it: "not_run" when no test ran
 * it; "survived" when tests ran it but none failed when the plugin changed it on purpose, and
 * "caught" when one did, each with that change in words (`change`, such as "`>` to `>=`") and, for
 * "caught", the test that failed when the plugin names it (`test`). A later format may add other
 * statuses, which keep their own words here.
 */
export type TestGapLine = { path: string; line: number; status: string; change: string | null; test: string | null };

/**
 * How many changed lines the run counted, how many of them some test ran, and how many none did;
 * and, when the plugin also changed lines on purpose and ran the tests again, how many of those
 * changes no test noticed (`survived`) and how many a test caught, each null when it doesn't say.
 */
export type TestGapCounts = { changed_lines: number; run: number; not_run: number; survived: number | null; caught: number | null };

/**
 * Which changed lines the tests ran, and which changes made on purpose they noticed, as the plugin
 * measured them by running the tests with `tool`, which may name several runs, joined by "; ";
 * or, when `measured` is false, why it couldn't; and what the measure may miss.
 */
export type TestGaps = { measured: boolean; reason: string | null; tool: string; lines: TestGapLine[]; summary: TestGapCounts; limits: string[] };

/** A function the plugin ran example inputs through, and where it starts. */
export type ExampleFunction = { name: string; path: string; line: number };

/**
 * One example input run through a function: what it gave at the pull request's base (`before`) and
 * at its head (`after`); whether the two are the same, differ, vary between runs, or a run failed;
 * and which code ran it, both, the head only, or neither (`label`), with the plugin's note.
 */
export type ExampleRow = {
  function: string;
  input: string;
  before: string | null;
  after: string | null;
  status: "same" | "different" | "flaky" | "error";
  label: "ran_base_and_head" | "ran_head_only" | "not_compared";
  note: string | null;
};

/** Example inputs the plugin ran through the old and new code of some changed functions, and what that may miss. */
export type Examples = { functions: ExampleFunction[]; rows: ExampleRow[]; limits: string[] };

/** What the user chose so far: approve or comment, and which comments go out, with any edited text. */
export type Drafts = { event: "APPROVE" | "COMMENT"; message: string; findings: Record<string, { included: boolean; text?: string }> };

export type ShownReview = {
  overview: Overview | null;
  /** null when the plugin doesn't say; empty when the pull request has no commits after the reviewed one. */
  commitsSinceReview: Commit[] | null;
  coChange: CoChange[];
  /** null when the plugin printed no map of the changed code, or none with a node that fits. */
  connections: Connections | null;
  /** null when the plugin printed no run of the tests over the changed lines, or one that doesn't fit. */
  testGaps: TestGaps | null;
  /** null when the plugin printed no example inputs, or none with a row that fits. */
  examples: Examples | null;
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
const isCount = (value: unknown): value is number => typeof value === "number" && Number.isInteger(value) && value >= 0;
/** The entries of an array that fit, each read by `read`; anything else reads as no entries. */
const listOf = <T>(value: unknown, read: (item: unknown) => T | null): T[] => (Array.isArray(value) ? value.flatMap((item) => read(item) ?? []) : []);
/** The strings of an array; anything else reads as none. */
const strings = (value: unknown) => listOf(value, (item) => (isString(item) ? item : null));
const isOneOf = <T extends string>(options: readonly T[], value: unknown): value is T => options.includes(value as T);

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
    checked: strings(value.checked),
    not_checked: strings(value.not_checked),
  };
}

function readNode(value: unknown): ConnectionNode | null {
  if (!isRecord(value) || !isString(value.id) || !isString(value.name) || !isString(value.path) || !isLine(value.line)) return null;
  if (!isOneOf(["func", "method", "type", "var", "const"], value.kind) || !isOneOf(["added", "changed", "removed", "unchanged"], value.change)) return null;
  if (!isCount(value.lines_changed)) return null;
  return { id: value.id, name: value.name, kind: value.kind, path: value.path, line: value.line, change: value.change, lines_changed: value.lines_changed };
}

/**
 * The map of the changed code the plugin printed, or null when it printed none or none with a node
 * that fits. A node whose id an earlier node has, and an edge to or from a node it doesn't list,
 * are left out.
 */
export function readConnections(value: unknown): Connections | null {
  if (!isRecord(value)) return null;
  const ids = new Set<string>();
  const nodes = listOf(value.nodes, readNode).filter((node) => !ids.has(node.id) && ids.add(node.id));
  if (nodes.length === 0) return null;
  const edges = listOf(value.edges, (edge): ConnectionEdge | null => (
    isRecord(edge) && isString(edge.from) && isString(edge.to) && ids.has(edge.from) && ids.has(edge.to) && isOneOf(["calls", "uses"], edge.kind) && isOneOf(["gopls", "name_search"], edge.found_by)
      ? { from: edge.from, to: edge.to, kind: edge.kind, found_by: edge.found_by } : null
  ));
  return { nodes, edges, limits: strings(value.limits) };
}

/**
 * The plugin's runs of the tests over the changed lines, or null when it printed none or one without
 * `measured`, `tool`, or its counts. A line whose path and number an earlier line has is left out,
 * and so is a "survived" or "caught" line without its change, which is what says what the tests
 * missed or caught; a line with a status this version doesn't know stays, with that status.
 */
export function readTestGaps(value: unknown): TestGaps | null {
  if (!isRecord(value) || typeof value.measured !== "boolean" || !isString(value.tool)) return null;
  const summary = value.summary;
  if (!isRecord(summary) || !isCount(summary.changed_lines) || !isCount(summary.run) || !isCount(summary.not_run)) return null;
  const seen = new Set<string>();
  const lines = listOf(value.lines, (line): TestGapLine | null => {
    if (!isRecord(line) || !isString(line.path) || !isLine(line.line) || !isString(line.status) || line.status === "") return null;
    const change = isString(line.change) && line.change !== "" ? line.change : null;
    if ((line.status === "survived" || line.status === "caught") && change === null) return null;
    return { path: line.path, line: line.line, status: line.status, change, test: isString(line.test) && line.test !== "" ? line.test : null };
  }).filter((line) => !seen.has(`${line.path}:${line.line}`) && seen.add(`${line.path}:${line.line}`));
  return {
    measured: value.measured,
    reason: isString(value.reason) ? value.reason : null,
    tool: value.tool,
    lines,
    summary: {
      changed_lines: summary.changed_lines, run: summary.run, not_run: summary.not_run,
      survived: isCount(summary.survived) ? summary.survived : null,
      caught: isCount(summary.caught) ? summary.caught : null,
    },
    limits: strings(value.limits),
  };
}

/**
 * The example inputs the plugin ran, or null when it printed none or none with a row that fits. A
 * row with a status or label this version doesn't know is left out, since only a row that says how
 * it ran may show: nothing predicted.
 */
export function readExamples(value: unknown): Examples | null {
  if (!isRecord(value)) return null;
  const rows = listOf(value.rows, (row): ExampleRow | null => {
    if (!isRecord(row) || !isString(row.function) || !isString(row.input)) return null;
    if (!isOneOf(["same", "different", "flaky", "error"], row.status) || !isOneOf(["ran_base_and_head", "ran_head_only", "not_compared"], row.label)) return null;
    return {
      function: row.function, input: row.input,
      before: isString(row.before) ? row.before : null,
      after: isString(row.after) ? row.after : null,
      status: row.status, label: row.label,
      note: isString(row.note) ? row.note : null,
    };
  });
  if (rows.length === 0) return null;
  return {
    functions: listOf(value.functions, (item) => (isRecord(item) && isString(item.name) && isString(item.path) && isLine(item.line) ? { name: item.name, path: item.path, line: item.line } : null)),
    rows,
    limits: strings(value.limits),
  };
}

function readCommit(value: unknown): Commit | null {
  if (!isRecord(value) || !isString(value.sha) || !isString(value.title) || !isString(value.at)) return null;
  return { sha: value.sha, title: value.title, author: isString(value.author) ? value.author : null, at: value.at, files: strings(value.files) };
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
    connections: readConnections(shown.connections),
    testGaps: readTestGaps(shown.test_gaps),
    examples: readExamples(shown.examples),
    findings: findings ? listOf(findings.findings, readFinding) : [],
    reason: approval && isString(approval.reason) ? approval.reason : null,
    drafts: readDrafts(shown.drafts),
    patch: isString(shown.diff) ? shown.diff : null,
  };
}
