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
    status: null,
    ...overrides,
  };
}

/**
 * An invented plugin whose status action prints status format 2, laid out like Monopole's: four
 * declared groups, two of them under Needs you, Done folded; Rerun and Delete on rows, Delete asking
 * first; and Check providers, which a blocker names.
 */
export function fixtureV2Plugin(overrides: Partial<PluginSnapshot> = {}): PluginSnapshot {
  return fixturePlugin({
    name: "fixture-runs",
    title: "Fixture runs",
    description: "Runs invented evaluations and prints status format 2.",
    folder: "/synthetic/plugins/fixture-runs",
    stateFolder: "/synthetic/state/fixture-runs",
    pageUrl: "pt-plugin://fixture-runs/index.html",
    actions: [
      { name: "status", title: "Status", description: "Prints status format 2.", confirm: false },
      { name: "rerun", title: "Rerun", description: "Runs the evaluation again.", confirm: false },
      { name: "delete", title: "Delete run", description: "Deletes the run's records.", confirm: true },
      { name: "check-providers", title: "Check providers", description: "Checks each provider's sign-in again.", confirm: false },
    ],
    prerequisites: [],
    status: {
      format: 2,
      groups: [
        { id: "failed", title: "Run failed", needsYou: true, order: 1, folded: false },
        { id: "review", title: "Review needed", needsYou: true, order: 2, folded: false },
        { id: "running", title: "Running", needsYou: false, order: 3, folded: false },
        { id: "done", title: "Done", needsYou: false, order: 4, folded: true },
      ],
      itemActions: ["rerun", "delete"],
    },
    ...overrides,
  });
}

/** What fixtureV2Plugin's status action prints: one blocker and an item in each group. */
export function fixtureV2Status(overrides: Record<string, unknown> = {}) {
  return {
    format: 2,
    agent: { words: "Idle · last run 2 min ago", dot: "idle" },
    blockers: [{ id: "codex-signed-out", title: "Codex is signed out", dot: "error", route: "/providers", action: "check-providers" }],
    items: [
      { id: "run-74c91b1d", group: "failed", title: "Review this stack of PRs · acme/widgets#60", detail: "Verification Discipline: the model could not run",
        dot: "error", project: null, updated_at: "2026-11-26T18:43:07Z", route: "/runs/run-74c91b1d", actions: ["rerun", "delete"] },
      { id: "run-review-2", group: "review", title: "Tighten upload retries", detail: "2 rubrics below threshold",
        dot: "attention", project: "/work/alpha", updated_at: "2026-11-26T17:00:00Z", route: "/runs/run-review-2", actions: ["delete"] },
      { id: "run-running-3", group: "running", title: "Score yesterday's sessions", detail: null,
        dot: "running", project: null, updated_at: "2026-11-26T18:50:00Z", route: "/runs/run-running-3" },
      { id: "run-done-4", group: "done", title: "Compare judge panels",
        dot: "success", project: null, updated_at: "2026-11-25T10:00:00Z", route: "/runs/run-done-4", actions: ["rerun", "delete"] },
    ],
    truncated: false,
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
