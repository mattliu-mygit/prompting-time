import { createSignal, Show } from "solid-js";
import type { AppApi, AppStore } from "../../src/app/store";
import type { PluginActionRequest, PluginActionResult, PluginSnapshot } from "../../src/bridge/types";
import { ConfirmActionDialog } from "../../src/features/agents/ConfirmActionDialog";

/**
 * `?agents=1`: an invented Whooper under Background agents, running with its next poll at 3:45 PM.
 * `?agents=missing` shows it stopped for a missing program; `?agents=confirm` opens the submit
 * confirmation over the app, and `?agents=mentions` opens it on a refusal for an @-mention first.
 */
export const agentsScenario = new URLSearchParams(location.search).get("agents");

const schema = {
  title: "Whooper settings",
  type: "object",
  properties: {
    schedule: {
      title: "Polling",
      type: "object",
      properties: {
        interval_minutes: { title: "Poll every (minutes)", description: "Minutes from the end of one round to the start of the next.", type: "integer", minimum: 1 },
        start: { title: "Start", description: "The local time rounds start from, such as 10:00.", type: "string" },
        end: { title: "End", description: "The local time from which no round starts, such as 18:00.", type: "string" },
        days: { title: "Days", type: "array", items: { enum: ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] } },
      },
    },
    poll: {
      title: "Poll",
      type: "object",
      properties: {
        timeout_minutes: { title: "Stop a poll after (minutes)", description: "At least 5: a poll takes 2 to 3 minutes.", type: "integer", minimum: 5 },
      },
    },
    jobs: {
      title: "Agents",
      type: "object",
      properties: {
        review: { title: "Review agent", enum: ["claude", "devin"] },
        poll: { title: "Poll agent", enum: ["claude", "devin"] },
      },
    },
    agents: {
      title: "Models",
      type: "object",
      properties: {
        claude: {
          title: "Claude",
          type: "object",
          properties: {
            review_model: { title: "Review model", type: "string" },
            budget_usd: { title: "Most one review may spend (USD)", description: "Leave out for no limit.", type: "number", minimum: 1 },
          },
        },
      },
    },
    review: {
      title: "Review runs",
      type: "object",
      properties: {
        timeout_minutes: { title: "Stop a review after (minutes)", type: "integer", minimum: 1 },
        skills: { title: "Review skills", description: "The skills each review run loads.", type: "array", items: { type: "string" } },
      },
    },
  },
};

let settings: Record<string, Record<string, unknown>> = {
  schedule: { interval_minutes: 15, start: "10:00", end: "18:00", days: ["mon", "tue", "wed", "thu", "fri"] },
  poll: { timeout_minutes: 10 },
  jobs: { review: "devin", poll: "claude" },
  agents: { claude: { review_model: "claude-fixture-large" } },
  review: { timeout_minutes: 45, skills: ["fixture-review", "fixture-style"] },
};

const plugin: PluginSnapshot = {
  name: "whooper",
  title: "Whooper",
  version: "0.1.0",
  description: "Finds the pull requests waiting on your review, reviews each with a coding-agent run, and submits the review you choose as one GitHub review.",
  source: { kind: "release", url: "https://example.invalid/whooper/releases/v0.1.0/whooper-0.1.0.tar.gz", sha256: "4f".repeat(32) },
  folder: "/synthetic/application-support/plugins/whooper",
  stateFolder: "/synthetic/state/whooper",
  settingsSchema: JSON.stringify(schema),
  actions: [
    { name: "status", title: "Status", description: "Prints the status as JSON.", confirm: false },
    { name: "round", title: "Poll now", description: "Runs a round now. Posts nothing.", confirm: false },
    { name: "get-settings", title: "Read settings", description: "Prints the settings as JSON.", confirm: false },
    { name: "save-settings", title: "Save settings", description: "Checks settings against the schema and saves them.", confirm: false },
    { name: "submit", title: "Submit review", description: "Posts the chosen comments and the approval or comment to GitHub as one review, with your gh login.", confirm: true },
  ],
  prerequisites: [
    { command: "gh", purpose: "Reads your review requests and submits your reviews.", needed: true, path: "/synthetic/bin/gh" },
    { command: "slackdump", purpose: "Reads the review asks in your Slack channel.", needed: true, path: agentsScenario === "missing" ? null : "/synthetic/bin/slackdump" },
    { command: "uv", purpose: "Supplies the Python Whooper's scripts run with.", needed: true, path: "/synthetic/bin/uv" },
    { command: "claude", purpose: "Runs the jobs the settings give to Claude.", needed: true, path: "/synthetic/bin/claude" },
    { command: "devin", purpose: "Runs the jobs the settings give to Devin.", needed: true, path: "/synthetic/bin/devin" },
  ],
  settingsError: null,
  process: agentsScenario === "missing" ? { kind: "notRunning", exit: null } : { kind: "started", pid: 4242 },
};

const nextPoll = new Date();
nextPoll.setHours(15, 45, 0, 0);
const request = JSON.stringify({
  commit_id: "0123456789abcdef0123456789abcdef01234567",
  event: "APPROVE",
  body: "Pre-approving to unblock",
  comments: [
    { path: "src/upload/retry.rs", line: 42, side: "RIGHT", body: "AI comment (vetted)\n\n**nit:** The retry budget reads clearer as a named constant." },
    { path: "src/upload/retry.rs", line: 57, side: "RIGHT", body: "AI comment (vetted)\n\n**question:** Should a cancelled upload count against the budget?" },
  ],
}, null, 2);
const fingerprint = "9c".repeat(32);
let mentionsRefused = agentsScenario !== "mentions";

const result = (overrides: Partial<PluginActionResult>): PluginActionResult => ({ exitCode: 0, stdout: "", stderr: "", fingerprint: null, preview: null, ...overrides });

async function pluginRequest(sent: PluginActionRequest): Promise<PluginActionResult> {
  switch (sent.action) {
    case "status":
      return result({ stdout: JSON.stringify({ version: 1, loop: { running: true, pid: 4242 }, paused: false, next_round: nextPoll.toISOString(), round: { running: false, started_at: null, reviewing: null }, last_round: null }) });
    case "get-settings":
      return result({ stdout: JSON.stringify(settings) });
    case "submit":
      if (!mentionsRefused && !sent.args.includes("--allow-mentions")) {
        mentionsRefused = true;
        return result({ exitCode: 12, stderr: "whooper-submit: nothing posted: the review @-mentions someone, and posting would notify them:\n  @fixture-teammate in the comment on src/upload/retry.rs:57 (finding 2)\nPass --allow-mentions to post it anyway." });
      }
      return sent.confirmedFingerprint === null
        ? result({ stdout: `printed the request to /synthetic/state/whooper/runs/fixture-run/submit-request.json; nothing posted\nfingerprint: ${fingerprint}\n`, fingerprint, preview: request })
        : result({ stdout: "posted the review: https://example.invalid/review/1\n" });
    default:
      throw new Error("This synthetic fixture does not run that action.");
  }
}

/** The fixture's bridge with the invented Whooper installed. */
export function withAgentsFixture(base: AppApi): AppApi {
  return {
    ...base,
    listPlugins: async () => [plugin],
    pluginRequest,
    savePluginSettings: async ({ values }) => {
      const changes = JSON.parse(values) as Record<string, Record<string, unknown>>;
      const timeout = changes.poll?.timeout_minutes;
      if (typeof timeout === "number" && timeout < 5) return result({ exitCode: 1, stderr: "whooper-settings: settings.poll.timeout_minutes is below 5" });
      settings = Object.fromEntries(Object.entries(settings).map(([key, group]) => [key, { ...group, ...changes[key] }]));
      return result({});
    },
    pickPluginArchive: async () => "/synthetic/release/whooper-0.1.0.tar.gz",
    pickPluginFolder: async () => "/synthetic/whooper",
    installPlugin: async ({ source }) => {
      throw new Error(source.kind === "archive"
        ? `The archive's SHA-256 is ${"0f".repeat(32)}, not the expected ${source.sha256}, so nothing was installed.`
        : "This synthetic fixture installs nothing.");
    },
  };
}

/** `?agents=confirm` and `?agents=mentions`: the submit confirmation for an invented review, over the app. */
export function AgentsConfirmation(props: { store: AppStore }) {
  const [open, setOpen] = createSignal(agentsScenario === "confirm" || agentsScenario === "mentions");
  return (
    <Show when={open()}>
      <ConfirmActionDialog plugin={plugin} action="submit" args={["fixture-run"]} actions={props.store.pluginActions} onClose={() => setOpen(false)} />
    </Show>
  );
}
