import { createSignal, Show } from "solid-js";
import type { AppApi, AppStore } from "../../src/app/store";
import type { AppEvent, PluginActionRequest, PluginActionResult, PluginSnapshot } from "../../src/bridge/types";
import { ConfirmActionDialog } from "../../src/features/agents/ConfirmActionDialog";
import { answerReviewAction, dockerCause, fixtureReviewEntries, fixtureRounds, fixtureStatusV2Parts, fixtureSubmitRequest, markFixtureSubmitted, reviewActions } from "./reviews-fixture";
import { answerV2Action, V2_PLUGIN, v2Plugin } from "./status-v2-fixture";

/**
 * `?agents=1`: an invented Whooper in the sidebar's Plugins section, running with its next poll at 3:45 PM,
 * whose status prints status format 2 as whooper-status --json does. `?agents=missing` shows it
 * stopped for a missing program; `?agents=confirm` opens the submit confirmation over the app, and
 * `?agents=mentions` opens it on Whooper's refusal for an @-mention first, which shows Whooper's own
 * words. `?agents=reviews` lists invented reviews as items in Whooper's groups, which open in the
 * plugin page the dev server serves.
 * `?agents=v2` adds an invented plugin that prints status format 2 (status-v2-fixture.ts), whose
 * actions send pluginChanged events. `?agents=none` installs no plugin, but its pickers answer, so
 * Add plugin reaches the install dialog.
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
  pageUrl: "/plugin-page/index.html",
  actions: [
    { name: "status", title: "Status", description: "Prints the status as JSON.", confirm: false },
    ...(agentsScenario === "reviews" ? [] : [{ name: "round", title: "Poll now", description: "Runs a round now. Posts nothing.", confirm: false }]),
    { name: "pause", title: "Pause", description: "Skips scheduled rounds until resume; reviews in progress finish.", confirm: false },
    { name: "resume", title: "Resume", description: "Runs scheduled rounds again.", confirm: false },
    { name: "get-settings", title: "Read settings", description: "Prints the settings as JSON.", confirm: false },
    { name: "save-settings", title: "Save settings", description: "Checks settings against the schema and saves them.", confirm: false },
    { name: "submit", title: "Submit review", description: "Posts the chosen comments and the approval or comment to GitHub as one review, with your gh login.", confirm: true },
    ...(agentsScenario === "reviews" ? reviewActions : []),
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
  // Whooper's manifest `status` key.
  status: {
    format: 2,
    groups: [
      { id: "ready", title: "Ready to approve", needsYou: true, order: 1, folded: false },
      { id: "look", title: "Needs a look", needsYou: true, order: 2, folded: false },
      { id: "failed", title: "Review failed", needsYou: true, order: 3, folded: false },
      { id: "reviewing", title: "Reviewing", needsYou: false, order: 4, folded: false },
      { id: "waiting", title: "Waiting", needsYou: false, order: 5, folded: false },
      { id: "done", title: "Done", needsYou: false, order: 6, folded: true },
    ],
    itemActions: ["retry", "skip", "remove"],
  },
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
/** Whether the invented Whooper runs and is paused; Stop, Start, Pause, and Resume change them in browser memory. */
let running = agentsScenario !== "missing";
let paused = false;
const logLines = Array.from({ length: 40 }, (_, index) => `15:${String(index).padStart(2, "0")}:0${index % 10} ${index % 9 === 4 ? "review acme/widgets#110: setup: Cannot connect to the Docker daemon. Is the docker daemon running?" : `poll: ${index % 3} review requests, ${index % 2} from Slack`}`);

const result = (overrides: Partial<PluginActionResult>): PluginActionResult => ({ exitCode: 0, stdout: "", stderr: "", fingerprint: null, preview: null, ...overrides });

// The app's event listeners, each numbering its events in order, so the fixture can add pluginChanged.
const appListeners = new Set<(event: AppEvent) => void>();
let appSequence = 0;

async function pluginRequest(sent: PluginActionRequest): Promise<PluginActionResult> {
  if (sent.plugin === V2_PLUGIN) {
    return answerV2Action(sent, (files) => appListeners.forEach((listener) => listener({ kind: "pluginChanged", sequence: "", plugin: V2_PLUGIN, files })));
  }
  const answered = agentsScenario === "reviews" ? answerReviewAction(sent) : null;
  if (answered) return answered;
  if (agentsScenario === "reviews" && sent.action === "submit") {
    const runId = sent.args[0]!;
    if (sent.confirmedFingerprint === null) {
      return result({ stdout: `printed the request to /synthetic/state/whooper/runs/${runId}/submit-request.json; nothing posted\nfingerprint: ${fingerprint}\n`, fingerprint, preview: fixtureSubmitRequest(runId) });
    }
    markFixtureSubmitted(runId);
    return result({ stdout: "posted the review: https://example.invalid/acme/widgets/pull/101#pullrequestreview-1\n" });
  }
  switch (sent.action) {
    case "status": {
      const reviews = agentsScenario === "reviews" ? fixtureReviewEntries() : [];
      const blockedSince = new Date();
      blockedSince.setHours(14, 30, 0, 0);
      const waiting = reviews.filter((review) => review.outcome === "waiting").length;
      const blocked = agentsScenario === "reviews" && waiting > 0;
      const time = nextPoll.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
      // The agent row's words and dot, as whooper-status's status_v2 words them.
      const agent = blocked ? { words: `Can't review · ${dockerCause.summary}`, dot: "error" }
        : paused ? { words: "Paused", dot: "idle" } : { words: `Running · next poll ${time}`, dot: "idle" };
      return result({ stdout: JSON.stringify({
        version: 1, loop: { running: running, pid: running ? 4242 : null }, paused, next_round: running && !paused ? nextPoll.toISOString() : null,
        round: { running: false, started_at: null, reviewing: null }, last_round: null,
        ...(agentsScenario === "reviews" ? { reviews, rounds: fixtureRounds(), blocked: blocked ? { ...dockerCause, since: blockedSince.toISOString(), waiting } : null } : {}),
        format: 2, agent,
        ...(agentsScenario === "reviews" ? fixtureStatusV2Parts() : { blockers: [], items: [], truncated: false }),
      }) });
    }
    case "round":
      return result({ stdout: "polled: no new review requests\nround done\n" });
    case "pause":
    case "resume":
      paused = sent.action === "pause";
      return result({ stdout: `${paused ? "paused" : "resumed"}\n` });
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
    listPlugins: async () => (agentsScenario === "none" ? [] : [
      { ...plugin, process: running ? plugin.process : { kind: "notRunning" as const, exit: null } },
      ...(agentsScenario === "v2" ? [v2Plugin] : []),
    ]),
    stopPlugin: async () => {
      running = false;
      return { ...plugin, process: { kind: "notRunning" as const, exit: null } };
    },
    startPlugin: async () => {
      running = true;
      paused = false;
      return plugin;
    },
    ...(agentsScenario === "v2" ? {
      listenToAppEvents: async (listener: (event: AppEvent) => void) => {
        const forward = (event: AppEvent) => listener({ ...event, sequence: String(++appSequence) });
        appListeners.add(forward);
        const unsubscribe = await base.listenToAppEvents(forward);
        return () => {
          appListeners.delete(forward);
          unsubscribe();
        };
      },
    } : {}),
    readPluginLog: async () => ({ path: "/synthetic/application-support/plugins/whooper.log", text: running ? logLines.join("\n") : "", truncated: false }),
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
