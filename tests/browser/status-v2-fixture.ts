import type { PluginActionRequest, PluginActionResult, PluginSnapshot } from "../../src/bridge/types";

/**
 * `?agents=v2`: an invented plugin, beside the invented Whooper, whose status action prints status
 * format 2, laid out like Monopole's. Its manifest declares four groups (Run failed and Review
 * needed under Needs you, Running, and Done folded), Rerun and Delete run on rows (Delete asks
 * first), and Check providers, which its one blocker names. Everything answers in browser memory.
 */
export const V2_PLUGIN = "fixture-runs";

export const v2Plugin: PluginSnapshot = {
  name: V2_PLUGIN,
  title: "Fixture runs",
  version: "0.1.0",
  description: "Runs invented evaluations of agent sessions and prints status format 2.",
  source: { kind: "folder", path: "/synthetic/fixture-runs" },
  folder: "/synthetic/fixture-runs",
  stateFolder: "/synthetic/state/fixture-runs",
  settingsSchema: JSON.stringify({ title: "Fixture runs settings", type: "object", properties: {} }),
  pageUrl: "/plugin-page/index.html",
  actions: [
    { name: "status", title: "Status", description: "Prints status format 2.", confirm: false },
    { name: "get-settings", title: "Read settings", description: "Prints the settings as JSON.", confirm: false },
    { name: "save-settings", title: "Save settings", description: "Checks and saves settings.", confirm: false },
    { name: "rerun", title: "Rerun", description: "Runs the evaluation again with its pinned configuration.", confirm: false },
    { name: "delete", title: "Delete run", description: "Deletes the run's records from this Mac.", confirm: true },
    { name: "check-providers", title: "Check providers", description: "Checks each provider's sign-in again.", confirm: false },
  ],
  prerequisites: [],
  settingsError: null,
  process: { kind: "started", pid: 5151 },
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
};

const minutesAgo = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

type Item = { id: string; group: string; title: string; detail: string | null; dot: string; project: string | null; updated_at: string; route: string; actions: string[] };

let codexSignedOut = true;
let items: Item[] = [
  { id: "run-74c91b1d", group: "failed", title: "Review this stack of PRs · acme/widgets#60", detail: "Verification Discipline: the model could not run",
    dot: "error", project: null, updated_at: minutesAgo(12), route: "/runs/run-74c91b1d", actions: ["rerun", "delete"] },
  { id: "run-5e02aa10", group: "failed", title: "Score last week's sessions", detail: "Codex is signed out",
    dot: "error", project: null, updated_at: minutesAgo(40), route: "/runs/run-5e02aa10", actions: ["rerun", "delete"] },
  { id: "run-31b7c0de", group: "review", title: "Tighten upload retries", detail: "2 rubrics below threshold",
    dot: "attention", project: "/synthetic/projects/widgets", updated_at: minutesAgo(25), route: "/runs/run-31b7c0de", actions: ["delete"] },
  { id: "run-90fe11aa", group: "running", title: "Compare judge panels on the auth refactor", detail: "3 of 8 sessions",
    dot: "running", project: "/synthetic/projects/widgets", updated_at: minutesAgo(1), route: "/runs/run-90fe11aa", actions: [] },
  ...Array.from({ length: 6 }, (_, index): Item => ({
    id: `run-done-${index}`, group: "done", title: `Nightly evaluation ${index + 1}`, detail: "All rubrics passed",
    dot: "success", project: null, updated_at: minutesAgo(60 * 24 * (index + 1)), route: `/runs/run-done-${index}`, actions: ["rerun", "delete"],
  })),
];

const result = (overrides: Partial<PluginActionResult>): PluginActionResult => ({ exitCode: 0, stdout: "", stderr: "", fingerprint: null, preview: null, ...overrides });
const fingerprint = "5d".repeat(32);

/** The `--item <id>` the request names, or null. */
function itemOf(sent: PluginActionRequest): string | null {
  const index = sent.args.indexOf("--item");
  return index >= 0 ? sent.args[index + 1] ?? null : null;
}

/** Answers the invented format-2 plugin's actions in browser memory. */
export function answerV2Action(sent: PluginActionRequest): PluginActionResult {
  const id = itemOf(sent);
  switch (sent.action) {
    case "status":
      return result({ stdout: JSON.stringify({
        format: 2,
        agent: items.some(({ group }) => group === "running") ? { words: "Running 1 evaluation", dot: "running" } : { words: "Idle · last run 2 min ago", dot: "idle" },
        blockers: codexSignedOut ? [{ id: "codex-signed-out", title: "Codex is signed out", dot: "error", route: "/providers", action: "check-providers" }] : [],
        items,
        truncated: false,
      }) });
    case "get-settings":
      return result({ stdout: "{}" });
    case "check-providers":
      codexSignedOut = false;
      return result({ stdout: "codex: signed in\n" });
    case "rerun":
      items = items.map((item) => (item.id === id ? { ...item, group: "running", dot: "running", detail: "Queued again", updated_at: new Date().toISOString(), actions: [] } : item));
      return result({ stdout: `queued ${id} again\n` });
    case "delete":
      if (sent.confirmedFingerprint === null) {
        return result({ stdout: `printed the request to /synthetic/state/fixture-runs/delete-request.json; nothing deleted\nfingerprint: ${fingerprint}\n`, fingerprint,
          preview: JSON.stringify({ delete: id, records: ["run.json", "scores.jsonl", "reviews/"] }, null, 2) });
      }
      items = items.filter((item) => item.id !== id);
      return result({ stdout: `deleted ${id}\n` });
    default:
      return result({ exitCode: 2, stderr: `fixture-runs: this synthetic fixture does not run ${sent.action}` });
  }
}
