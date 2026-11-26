import type { MenuItem } from "@prompting-time/ui";
import type { PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { ACTIONS, hasAction } from "./actions";
import { newestRuns, readReviews, type ReviewEntry } from "./reviews";
import { missingPrograms, readAgentStatus } from "./status";

/** What a background agent's row menu and screen can do: run its round, pause or resume it, stop or start it, or show a tab. */
export type AgentAction = "poll" | "pause" | "resume" | "stop" | "start" | "settings" | "logs";
export type AgentTab = "runs" | "logs" | "settings";

type Control = { action: AgentAction; label: string; disabled: boolean };

/**
 * The agent's controls as they stand: Poll now (its round action) while it runs and no round does;
 * Pause or Resume (its pause and resume actions) while it runs; Stop for the copy Prompting Time
 * started, else Start once nothing keeps it from starting. `pending` is the control that runs now,
 * which disables the others. A control the plugin has no action for is null.
 */
export function agentControls(plugin: PluginSnapshot, read: PluginStatusRead | undefined, pending: AgentAction | null) {
  const status = readAgentStatus(read);
  const running = plugin.process.kind !== "notRunning";
  const polling = pending === "poll" || status?.round.running === true;
  const paused = status?.paused === true;
  const poll: Control | null = hasAction(plugin, ACTIONS.round)
    ? { action: "poll", label: polling ? "Polling…" : "Poll now", disabled: !running || polling || pending !== null }
    : null;
  const pause: Control | null = hasAction(plugin, ACTIONS.pause) && hasAction(plugin, ACTIONS.resume)
    ? { action: paused ? "resume" : "pause", label: paused ? "Resume" : "Pause", disabled: !running || pending !== null }
    : null;
  const power: Control = running
    // A copy started elsewhere, such as in a terminal, is stopped where it was started.
    ? { action: "stop", label: "Stop", disabled: plugin.process.kind !== "started" || pending !== null }
    : { action: "start", label: pending === "start" ? "Starting…" : "Start", disabled: pending !== null || plugin.settingsError !== null || missingPrograms(plugin).length > 0 };
  return { poll, pause, power };
}

/** Menu items for the agent's controls, in this order, followed by Settings and the log under `logsLabel`. */
export function agentMenuItems(plugin: PluginSnapshot, read: PluginStatusRead | undefined, pending: AgentAction | null, include: { poll: boolean; pause: boolean; logsLabel: string }): MenuItem[] {
  const { poll, pause, power } = agentControls(plugin, read, pending);
  const controls = [include.poll ? poll : null, include.pause ? pause : null, power].filter((control): control is Control => control !== null);
  return [
    ...controls.map((control) => ({ value: control.action, label: control.label, disabled: control.disabled })),
    { value: "settings", label: "Settings" },
    { value: "logs", label: include.logsLabel },
  ];
}

/** The reviews that run now, which stopping the agent ends. */
export function reviewsInProgress(read: PluginStatusRead | undefined): ReviewEntry[] {
  return newestRuns(readReviews(read?.value)).filter((review) => review.outcome === "running");
}
