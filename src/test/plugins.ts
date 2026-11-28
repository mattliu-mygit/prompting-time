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

/** Whooper's manifest `status` key: its six groups, three of them under Needs you, and its row actions. */
export const whooperStatusConfig: NonNullable<PluginSnapshot["status"]> = {
  format: 2,
  groups: [
    { id: "ready", title: "Ready to approve", needsYou: true, order: 1 },
    { id: "look", title: "Needs a look", needsYou: true, order: 2 },
    { id: "failed", title: "Review failed", needsYou: true, order: 3 },
    { id: "reviewing", title: "Reviewing", needsYou: false, order: 4 },
    { id: "waiting", title: "Waiting", needsYou: false, order: 5 },
    { id: "done", title: "Done", needsYou: false, order: 6 },
  ],
  itemActions: ["retry", "skip", "remove"],
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
    status: whooperStatusConfig,
    ...overrides,
  };
}

/**
 * An invented plugin whose status action prints status format 2, laid out like Monopole's: four
 * declared groups, two of them under Needs you; Rerun and Delete on rows, Delete asking
 * first; Check providers, which a blocker names; and a page that speaks page protocol 2.
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
        { id: "failed", title: "Run failed", needsYou: true, order: 1 },
        { id: "review", title: "Review needed", needsYou: true, order: 2 },
        { id: "running", title: "Running", needsYou: false, order: 3 },
        { id: "done", title: "Done", needsYou: false, order: 4 },
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

/**
 * Whooper's status in format 2 with the agent row's `words`, no blockers, and no items, plus the
 * overrides, such as `paused` or `items`.
 */
export function fixtureStatus(words = "Running · next poll 3:45 PM", overrides: Record<string, unknown> = {}) {
  return { format: 2, agent: { words, dot: "idle" }, blockers: [], items: [], truncated: false, ...overrides };
}

/**
 * Invented reviews as Whooper's status format 2 lists them, one in each group but Waiting: #101
 * ready to approve, #77 needing a look, #55 failed, #102 reviewing, and #88 done.
 */
export function fixtureWhooperItems() {
  const item = (pr: number, repo: string, group: string, title: string, detail: string, dot: string, updated: string, actions: string[]) => ({
    id: `run-${pr}`, group, title, detail: `${repo.split("/")[1]}#${pr} · ${detail}`, dot, project: null, updated_at: updated, route: `/reviews/run-${pr}`, actions,
  });
  return [
    item(101, "acme/widgets", "ready", "separate cache configuration", "@sam-fixture · 2 findings", "success", "2026-11-26T15:20:00Z", ["retry", "skip", "remove"]),
    item(77, "acme/gadgets", "look", "add the tags package", "@lin-fixture · 1 finding", "attention", "2026-11-26T14:30:00Z", ["retry", "skip", "remove"]),
    item(55, "acme/gadgets", "failed", "move the projection", "The review agent left no usable findings", "error", "2026-11-24T09:01:00Z", ["retry", "skip", "remove"]),
    item(102, "acme/widgets", "reviewing", "stream uploads in chunks", "reviewing since 2:00 PM", "running", "2026-11-26T14:00:00Z", []),
    item(88, "acme/gadgets", "done", "rename the queue settings", "approved Nov 25", "idle", "2026-11-25T11:00:00Z", ["retry", "remove"]),
  ];
}

/** A plugin action's result, exit 0 unless the overrides say otherwise. */
export function actionResult(overrides: Partial<PluginActionResult> = {}): PluginActionResult {
  return { exitCode: 0, stdout: "", stdoutTruncated: false, stderr: "", fingerprint: null, preview: null, ...overrides };
}
