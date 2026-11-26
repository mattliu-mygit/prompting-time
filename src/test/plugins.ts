import { vi } from "vitest";
import type { AppApi } from "../app/store";
import type { PluginActionResult, PluginSnapshot } from "../bridge/types";

/** The plugin half of a fake bridge with no plugins installed, for tests about other things. */
export function noPlugins(): Pick<AppApi, "listPlugins" | "installPlugin" | "pluginRequest" | "savePluginSettings" | "pickPluginFolder" | "pickPluginArchive"> {
  return {
    listPlugins: vi.fn().mockResolvedValue([]),
    installPlugin: vi.fn(),
    pluginRequest: vi.fn(),
    savePluginSettings: vi.fn(),
    pickPluginFolder: vi.fn().mockResolvedValue(null),
    pickPluginArchive: vi.fn().mockResolvedValue(null),
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
