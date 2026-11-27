import { vi } from "vitest";
import type { AppApi } from "../app/store";
import type { PluginActionResult, PluginSnapshot } from "../bridge/types";

/** The plugin half of a fake bridge with no plugins installed, for tests about other things. */
export function noPlugins(): Pick<AppApi, "listPlugins" | "installPlugin" | "pluginRequest" | "savePluginSettings" | "pickPluginFolder" | "pickPluginArchive" | "stopPlugin" | "startPlugin" | "readPluginLog" | "updateMenuBar" | "listenToMenuBar"> {
  return {
    listPlugins: vi.fn().mockResolvedValue([]),
    installPlugin: vi.fn(),
    pluginRequest: vi.fn(),
    savePluginSettings: vi.fn(),
    pickPluginFolder: vi.fn().mockResolvedValue(null),
    pickPluginArchive: vi.fn().mockResolvedValue(null),
    stopPlugin: vi.fn(),
    startPlugin: vi.fn(),
    readPluginLog: vi.fn().mockResolvedValue({ path: "/synthetic/plugins/whooper.log", text: "", truncated: false }),
    updateMenuBar: vi.fn().mockResolvedValue(undefined),
    listenToMenuBar: vi.fn().mockResolvedValue(() => {}),
  };
}

/** An invented settings schema with the shapes of Whooper's: groups, numbers with units, days, choices, a list, and a nested group. */
export const fixtureSchema = JSON.stringify({
  type: "object",
  properties: {
    schedule: {
      title: "Polling",
      type: "object",
      properties: {
        interval_minutes: { title: "Poll every (minutes)", description: "Minutes between rounds.", type: "integer", minimum: 1 },
        start: { title: "Start", type: "string" },
        days: { title: "Days", type: "array", items: { enum: ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] } },
      },
    },
    poll: {
      title: "Poll",
      type: "object",
      properties: { timeout_minutes: { title: "Stop a poll after (minutes)", type: "integer", minimum: 5 } },
    },
    jobs: {
      title: "Agents",
      type: "object",
      properties: { review: { title: "Review agent", enum: ["claude", "devin"] } },
    },
    agents: {
      title: "Models",
      type: "object",
      properties: {
        claude: {
          title: "Claude",
          type: "object",
          properties: { budget_usd: { title: "Most one review may spend (USD)", type: "number", minimum: 1 } },
        },
      },
    },
    review: {
      title: "Review runs",
      type: "object",
      properties: { skills: { title: "Review skills", type: "array", items: { type: "string" } } },
    },
  },
});

/** What the fixture plugin's get-settings prints. */
export const fixtureSettings = {
  schedule: { interval_minutes: 15, start: "10:00", days: ["mon", "tue", "wed", "thu", "fri"] },
  poll: { timeout_minutes: 10 },
  jobs: { review: "devin" },
  agents: { claude: {} },
  review: { skills: ["fixture-review", "fixture-style"] },
};

/** An invented copy of Whooper as list_plugins reports it, started by Prompting Time from a release archive. */
export function fixturePlugin(overrides: Partial<PluginSnapshot> = {}): PluginSnapshot {
  return {
    name: "whooper",
    title: "Whooper",
    version: "0.1.0",
    description: "Finds the pull requests waiting on your review and reviews each.",
    source: { kind: "release", url: "https://example.invalid/whooper-0.1.0.tar.gz", sha256: "a".repeat(64) },
    folder: "/synthetic/plugins/whooper",
    stateFolder: "/synthetic/state/whooper",
    settingsSchema: fixtureSchema,
    pageUrl: "pt-plugin://whooper/index.html",
    actions: [
      { name: "status", title: "Status", description: "Prints the status as JSON.", confirm: false },
      { name: "get-settings", title: "Read settings", description: "Prints the settings as JSON.", confirm: false },
      { name: "save-settings", title: "Save settings", description: "Checks and saves settings.", confirm: false },
      { name: "submit", title: "Submit review", description: "Posts the chosen comments to GitHub as one review.", confirm: true },
    ],
    prerequisites: [
      { command: "gh", purpose: "Reads pull requests.", needed: true, path: "/synthetic/bin/gh" },
      { command: "claude", purpose: "Runs the jobs the settings give to Claude.", needed: false, path: null },
    ],
    settingsError: null,
    process: { kind: "started", pid: 4242 },
    ...overrides,
  };
}

/** `whooper-status --json` output, waiting for its next round at `nextRound`. */
export function fixtureStatus(nextRound: string, overrides: Record<string, unknown> = {}) {
  return {
    version: 1,
    loop: { running: true, pid: 4242 },
    paused: false,
    next_round: nextRound,
    round: { running: false, started_at: null, reviewing: null },
    last_round: null,
    ...overrides,
  };
}

/** A plugin action's result, exit 0 unless the overrides say otherwise. */
export function actionResult(overrides: Partial<PluginActionResult> = {}): PluginActionResult {
  return { exitCode: 0, stdout: "", stderr: "", fingerprint: null, preview: null, ...overrides };
}

/**
 * The actions Whooper's page and Prompting Time's screen for a failed review run: status, show-review,
 * draft, ask, retry, round, and submit, which asks first.
 */
export const pageActions = [
  { name: "status", title: "Status", description: "Prints the status as JSON.", confirm: false },
  { name: "show-review", title: "Show a review", description: "Prints one review as JSON.", confirm: false },
  { name: "draft", title: "Change a draft", description: "Changes a review's drafts.", confirm: false },
  { name: "ask", title: "Ask about this", description: "Answers a question about a review from its record, and writes nothing else.", confirm: false },
  { name: "retry", title: "Retry a review", description: "Queues the pull request of a failed review for the next round.", confirm: false },
  { name: "round", title: "Poll now", description: "Runs a round now. Posts nothing.", confirm: false },
  { name: "submit", title: "Submit review", description: "Posts the chosen comments to GitHub as one review.", confirm: true },
];

/**
 * Invented reviews as `whooper-status --json` lists them, newest first. Only #101's newest run is
 * safe to approve: it says approve once CI passes, read the whole change, and found only nits.
 */
export function fixtureReviews() {
  return [
    { run_id: "20261126T150000Z-101", repo: "acme/widgets", pr: 101, title: "separate cache configuration", url: "https://example.invalid/acme/widgets/pull/101",
      started_at: "2026-11-26T15:00:00Z", ended_at: "2026-11-26T15:20:00Z", outcome: "reviewed", verdict: "approve_once_ci_passes",
      skipped: false, findings: 2, submitted: null, needs_you: true, author: "sam-fixture", safe_to_approve: true },
    { run_id: "20261126T140000Z-102", repo: "acme/widgets", pr: 102, title: null, url: null,
      started_at: "2026-11-26T14:00:00Z", ended_at: null, outcome: "running", verdict: null, skipped: false, findings: 0, submitted: null, needs_you: false, safe_to_approve: false },
    { run_id: "20261125T100000Z-77", repo: "acme/gadgets", pr: 77, title: "add the tags package", url: null,
      started_at: "2026-11-25T10:00:00Z", ended_at: "2026-11-25T10:30:00Z", outcome: "reviewed", verdict: "look_first",
      skipped: false, findings: 1, submitted: { event: "COMMENT", at: "2026-11-25T11:00:00Z" }, needs_you: false, safe_to_approve: false },
    // An earlier run of #101 failed without recording why; the newer run means it no longer needs you.
    { run_id: "20261125T080000Z-101", repo: "acme/widgets", pr: 101, title: "separate cache configuration", url: "https://example.invalid/acme/widgets/pull/101",
      started_at: "2026-11-25T08:00:00Z", ended_at: "2026-11-25T08:02:00Z", outcome: "failed", verdict: null,
      skipped: false, findings: 0, submitted: null, needs_you: false, safe_to_approve: false },
    { run_id: "20261124T090000Z-55", repo: "acme/gadgets", pr: 55, title: "move the projection", url: null,
      started_at: "2026-11-24T09:00:00Z", ended_at: "2026-11-24T09:01:00Z", outcome: "failed", verdict: null,
      skipped: false, findings: 0, submitted: null, needs_you: true, safe_to_approve: false },
  ];
}

/** Why each invented failed run failed, as its failure.txt says, or null for a run that recorded no reason. */
const fixtureFailures: Record<string, string | null> = {
  "20261124T090000Z-55": "the review run exited 1 without findings; see runs/20261124T090000Z-55/stderr.log",
  "20261125T080000Z-101": null,
};

/**
 * What the show-review action prints for one invented run, with the title the status lists for it.
 * A failed run has no findings, drafts, or diff, and says why it failed if it recorded a reason.
 */
export function fixtureShown(runId: string, overrides: Record<string, unknown> = {}) {
  const review = fixtureReviews().find((entry) => entry.run_id === runId);
  const failed = review?.outcome === "failed";
  return {
    version: 1,
    run_id: runId,
    run: review ? { repo: review.repo, pr: review.pr } : null,
    title: review?.title ?? null,
    author: "fixture-author",
    outcome: failed ? "failed" : "reviewed",
    findings: failed ? null : { status: "reviewed", skip_reason: "", approval: { verdict: "approve_once_ci_passes", reason: "Invented reason." }, findings: [{ id: "1", path: "src/a.rs", line: 3, side: "RIGHT", title: "A nit", draft: "**nit:** invented", kind: "nit" }] },
    drafts: failed ? null : { event: "APPROVE", message: "", findings: { "1": { included: true } } },
    diff: failed ? null : "diff --git a/src/a.rs b/src/a.rs\n",
    failure: fixtureFailures[runId] ?? null,
    asks: [],
    submitted: null,
    ...overrides,
  };
}

/** An invented two-file patch: a new retry policy in src/retry.rs and the client that starts using it. */
export const fixturePatch = [
  "diff --git a/src/retry.rs b/src/retry.rs",
  "new file mode 100644",
  "--- /dev/null",
  "+++ b/src/retry.rs",
  "@@ -0,0 +1,8 @@",
  "+pub struct RetryPolicy {",
  "+    pub attempts: u32,",
  "+}",
  "+",
  "+impl RetryPolicy {",
  "+    pub fn delay(&self, n: u32) -> u64 {",
  "+        250 * 2u64.pow(n)",
  "+    }",
  "diff --git a/src/client.rs b/src/client.rs",
  "--- a/src/client.rs",
  "+++ b/src/client.rs",
  "@@ -1,4 +1,5 @@",
  " fn send(body: &[u8]) {",
  "-    for _ in 0..3 { try_once(body); }",
  "+    let policy = RetryPolicy { attempts: 5 };",
  "+    retry(&policy, body);",
  " }",
  " ",
  "",
].join("\n");

/**
 * What show-review prints for a run Whooper wrote an overview of: two layers (the policy, then the
 * client, to skim), two places to look, the calls before and after, two commits after the reviewed
 * one, and a file that usually changes with the client. `overrides` replace its top-level fields.
 */
export function fixtureWalkthrough(runId: string, overrides: Record<string, unknown> = {}) {
  return {
    ...fixtureShown(runId),
    findings: { status: "reviewed", skip_reason: "", approval: { verdict: "approve_once_ci_passes", reason: "Small and tested." }, findings: [{ id: "1", path: "src/retry.rs", line: 7, side: "RIGHT", title: "The delay overflows", draft: "**nit:** `2u64.pow` overflows after 64 attempts.", kind: "nit" }] },
    drafts: { event: "APPROVE", message: "", findings: { "1": { included: true } } },
    diff: fixturePatch,
    overview: {
      summary: "Uploads retry with a `RetryPolicy` instead of a fixed loop.",
      verdict_reason: "The policy is small and its tests pass.",
      before_after: [{ label: "A failed upload", before: "tries 3 times at once", after: "tries 5 times, waiting longer each time" }],
      places: [
        { id: "p1", title: "The delay doubles without a cap", reason: "After 20 attempts the delay is hours.", evidence: { kind: "read", detail: "read `delay`" }, path: "src/retry.rs", start_line: 6, end_line: 8 },
        { id: "p2", title: "Five attempts, not three", reason: "Uploads now try twice more.", evidence: { kind: "ran", detail: "ran the client tests" }, path: "src/client.rs", start_line: 2, end_line: 3 },
      ],
      layers: [
        { id: "l1", title: "The retry policy", intent: "A new type that says how often to retry.", read_hint: "read", ranges: [{ path: "src/retry.rs", start_line: 1, end_line: 8 }] },
        { id: "l2", title: "The client uses it", intent: "The loop gives way to the policy.", read_hint: "skim", ranges: [{ path: "src/client.rs", start_line: 1, end_line: 4 }] },
      ],
      flow: {
        title: "A failed upload",
        input: "send(b\"x\") while the server answers 503",
        calls: [
          { from: "client", to: "server", label: "try_once", status: "removed", old_label: null, path: null, line: null },
          { from: "client", to: "policy", label: "retry", status: "added", old_label: null, path: "src/client.rs", line: 3 },
          { from: "policy", to: "policy", label: "delay(n)", status: "changed", old_label: "sleep(0)", path: "src/retry.rs", line: 7 },
        ],
      },
      checked: ["Ran the client tests at head and base"],
      not_checked: ["Didn't run the upload integration suite"],
    },
    commits_since_review: [
      { sha: "aaaa1111bbbb", title: "client: log each retry", author: "sam-fixture", at: "2026-11-26T16:00:00Z", files: ["src/client.rs"] },
      { sha: "cccc2222dddd", title: "docs: retries", author: "sam-fixture", at: "2026-11-26T17:00:00Z", files: ["README.md"] },
    ],
    co_change: [{ path: "src/server.rs", partner: "src/client.rs", ratio: 0.8, commits: 10 }],
    ...overrides,
  };
}

/** An invented Go patch: a new name rule in internal/projects/name.go, and Create, which checks it instead of slugify, removed. */
export const fixtureGoPatch = [
  "diff --git a/internal/projects/name.go b/internal/projects/name.go",
  "new file mode 100644",
  "--- /dev/null",
  "+++ b/internal/projects/name.go",
  "@@ -0,0 +1,11 @@",
  "+package projects",
  "+",
  '+import "regexp"',
  "+",
  "+// namePattern is what a bucket name allows.",
  "+var namePattern = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$`)",
  "+",
  "+// ValidateName says why a project name can't name a bucket.",
  "+func ValidateName(name string) error {",
  "+\treturn check(namePattern.MatchString(name))",
  "+}",
  "diff --git a/internal/projects/create.go b/internal/projects/create.go",
  "--- a/internal/projects/create.go",
  "+++ b/internal/projects/create.go",
  "@@ -10,8 +10,6 @@",
  " func Create(name string) (*Project, error) {",
  "-\tbucket := slugify(name)",
  "-\treturn newProject(name, bucket)",
  "+\tif err := ValidateName(name); err != nil {",
  "+\t\treturn nil, err",
  "+\t}",
  "+\treturn newProject(name, name)",
  " }",
  "-",
  "-func slugify(name string) string {",
  "-\treturn strings.ToLower(name)",
  "-}",
  "",
].join("\n");

/**
 * How the changed code of fixtureGoPatch connects, as show-review prints it: ValidateName and
 * namePattern added, Create changed, slugify removed, and two unchanged callers, one of which only
 * a search for ValidateName's name found, so it may call a different ValidateName.
 */
export function fixtureConnections() {
  return {
    nodes: [
      { id: "validate", name: "projects.ValidateName", kind: "func", path: "internal/projects/name.go", line: 9, change: "added", lines_changed: 3 },
      { id: "pattern", name: "projects.namePattern", kind: "var", path: "internal/projects/name.go", line: 6, change: "added", lines_changed: 1 },
      { id: "create", name: "projects.Create", kind: "func", path: "internal/projects/create.go", line: 10, change: "changed", lines_changed: 6 },
      { id: "slugify", name: "projects.slugify", kind: "func", path: "internal/projects/create.go", line: 15, change: "removed", lines_changed: 4 },
      { id: "handle", name: "api.handleCreate", kind: "func", path: "internal/api/projects.go", line: 41, change: "unchanged", lines_changed: 0 },
      { id: "signup", name: "api.signup", kind: "func", path: "internal/api/users.go", line: 30, change: "unchanged", lines_changed: 0 },
    ],
    edges: [
      { from: "handle", to: "create", kind: "calls", found_by: "gopls" },
      { from: "create", to: "validate", kind: "calls", found_by: "gopls" },
      { from: "validate", to: "pattern", kind: "uses", found_by: "gopls" },
      { from: "signup", to: "validate", kind: "calls", found_by: "name_search" },
    ],
    limits: ["Callers outside acme/gadgets aren't searched.", "gopls couldn't load internal/legacy, so its callers come from name search."],
  };
}

/**
 * How a TypeScript change connects, as show-review prints it: the trace sidebar shows token counts
 * from a new formatTokens. TypeScript's language service found every link but exportReport's, in
 * plain JavaScript outside TypeScript's project, which only a search for formatTokens found.
 */
export function fixtureTypeScriptConnections() {
  return {
    nodes: [
      { id: "format", name: "formatTokens", kind: "func", path: "web/src/trace/tokens.ts", line: 9, change: "added", lines_changed: 3 },
      { id: "sidebar", name: "TraceSidebar", kind: "func", path: "web/src/trace/TraceSidebar.tsx", line: 8, change: "changed", lines_changed: 2 },
      { id: "page", name: "TracePage", kind: "func", path: "web/src/trace/TracePage.tsx", line: 31, change: "unchanged", lines_changed: 0 },
      { id: "report", name: "exportReport", kind: "func", path: "web/scripts/report.js", line: 14, change: "unchanged", lines_changed: 0 },
    ],
    edges: [
      { from: "page", to: "sidebar", kind: "calls", found_by: "typescript" },
      { from: "sidebar", to: "format", kind: "calls", found_by: "typescript" },
      { from: "report", to: "format", kind: "calls", found_by: "name_search" },
    ],
    limits: ["TypeScript's project leaves out web/scripts, which is plain JavaScript, so its callers come from name search."],
  };
}

/**
 * Which changed lines of fixtureGoPatch the tests ran, as show-review prints it: no test ran lines
 * 11 and 12 of create.go, in the first place to look, nor line 14 after it. TestValidateName ran
 * name.go's lines 6 and 10: it caught the change Whooper made to line 10 on purpose, but no test
 * noticed the one to line 6, the second place to look.
 */
export function fixtureTestGaps() {
  return {
    measured: true,
    reason: null,
    tool: "go test -coverprofile",
    lines: [
      { path: "internal/projects/create.go", line: 11, status: "not_run" },
      { path: "internal/projects/create.go", line: 12, status: "not_run" },
      { path: "internal/projects/create.go", line: 14, status: "not_run" },
      { path: "internal/projects/name.go", line: 6, status: "survived", change: "`{1,61}` to `{1,62}`" },
      { path: "internal/projects/name.go", line: 10, status: "caught", change: "`namePattern.MatchString(name)` to `!namePattern.MatchString(name)`", test: "TestValidateName" },
    ],
    summary: { changed_lines: 15, run: 2, not_run: 3, survived: 1, caught: 1 },
    limits: ["Whooper changed only lines a test ran, one change on each.", "Tests of other packages that call Create didn't run."],
  };
}

/**
 * Example inputs run through fixtureGoPatch's functions, as show-review prints them: Create at base
 * and head, where "a b" now fails, and ValidateName, which only the head has.
 */
export function fixtureExamples() {
  return {
    functions: [
      { name: "projects.Create", path: "internal/projects/create.go", line: 10 },
      { name: "projects.ValidateName", path: "internal/projects/name.go", line: 9 },
    ],
    rows: [
      { function: "projects.Create", input: "\"abc\"", before: "&Project{Name: \"abc\", Bucket: \"abc\"}", after: "&Project{Name: \"abc\", Bucket: \"abc\"}", status: "same", label: "ran_base_and_head", note: null },
      { function: "projects.Create", input: "\"a b\"", before: "&Project{Name: \"a b\", Bucket: \"a b\"}", after: "error: name \"a b\" can't name a bucket", status: "different", label: "ran_base_and_head", note: null },
      { function: "projects.Create", input: "\"x\"", before: "&Project{Name: \"x\"}", after: "&Project{Name: \"x\"}", status: "flaky", label: "ran_base_and_head", note: "the two runs at base gave different results" },
      { function: "projects.Create", input: "\"\"", before: null, after: null, status: "error", label: "ran_base_and_head", note: "the run at base timed out" },
      { function: "projects.ValidateName", input: "\"a--b\"", before: null, after: "nil", status: "different", label: "ran_head_only", note: "new in this PR" },
      { function: "projects.ValidateName", input: "\"Q3\"", before: null, after: null, status: "same", label: "not_compared", note: "the test didn't build" },
    ],
    limits: ["Create ran with an in-memory store, not a real bucket."],
  };
}

/** What show-review prints for a run of fixtureGoPatch with an overview, how its changed code connects, its test gaps, and example inputs. */
export function fixtureConnectionsWalkthrough(runId: string, overrides: Record<string, unknown> = {}) {
  return fixtureWalkthrough(runId, {
    findings: { status: "reviewed", skip_reason: "", approval: { verdict: "approve_once_ci_passes", reason: "Small and tested." }, findings: [{ id: "1", path: "internal/projects/name.go", line: 6, side: "RIGHT", title: "Two dashes pass", draft: "**nit:** `namePattern` lets `a--b` through.", kind: "nit" }] },
    diff: fixtureGoPatch,
    overview: {
      summary: "Projects check their names before they get a bucket, instead of `slugify` turning any name into one.",
      verdict_reason: "Small and tested.",
      before_after: [{ label: "A name with a space", before: "gets a bucket with a dash", after: "fails with a message" }],
      places: [
        { id: "p1", title: "Names with spaces now fail", reason: "`slugify` used to fix them.", evidence: { kind: "read", detail: "read `Create`" }, path: "internal/projects/create.go", start_line: 11, end_line: 13 },
        { id: "p2", title: "Two dashes pass", reason: "`namePattern` lets `a--b` through.", evidence: { kind: "ran", detail: "ran `TestValidateName`" }, path: "internal/projects/name.go", start_line: 6, end_line: 6 },
      ],
      layers: [
        { id: "rule", title: "The name rule", intent: "What a bucket name allows.", read_hint: "read", ranges: [{ path: "internal/projects/name.go", start_line: 1, end_line: 11 }] },
        { id: "create", title: "Create checks names", intent: "The check replaces slugify.", read_hint: "read", ranges: [{ path: "internal/projects/create.go", start_line: 10, end_line: 15 }] },
      ],
      flow: null,
      checked: ["Ran go test ./internal/projects/..."],
      not_checked: ["Didn't create a real bucket"],
    },
    commits_since_review: [],
    co_change: [],
    connections: fixtureConnections(),
    test_gaps: fixtureTestGaps(),
    examples: fixtureExamples(),
    ...overrides,
  });
}
