import type { MenuItem } from "@prompting-time/ui";
import type { PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { ACTIONS, hasAction } from "./actions";
import { missingPrograms } from "./status";
import { readStatusV2, type StatusV2Item } from "./statusV2";

/** What a background agent's row menu and screen can do: run its round, pause or resume it, stop or start it, or show a tab. */
export type AgentAction = "poll" | "pause" | "resume" | "stop" | "start" | "settings" | "logs";
export type AgentTab = "logs" | "settings";

export type Control = { action: AgentAction; label: string; disabled: boolean };

/**
 * The agent's controls as they stand: Poll now (its round action) while it runs and Poll now isn't
 * already running; Pause or Resume (its pause and resume actions) while it runs, Resume while its
 * status format 2 says `paused`; Stop for the copy Prompting Time started, else Start once nothing
 * keeps it from starting. `pending` is the control that runs now, which disables the others. A
 * control the plugin has no action for is null.
 */
export function agentControls(plugin: PluginSnapshot, read: PluginStatusRead | undefined, pending: AgentAction | null) {
  const running = plugin.process.kind !== "notRunning";
  const polling = pending === "poll";
  const paused = readStatusV2(plugin, read)?.paused === true;
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

/**
 * The buttons the menu bar extra shows for the agent: Pause or Resume, or Start, which takes
 * Pause's place while the agent is stopped, then Poll now. Either is null when
 * the plugin has no action for it.
 */
export function menuBarButtons(plugin: PluginSnapshot, read: PluginStatusRead | undefined, pending: AgentAction | null): [Control | null, Control | null] {
  const { poll, pause, power } = agentControls(plugin, read, pending);
  return [power.action === "start" ? power : pause, poll];
}

/**
 * The ⋯ menu of a plugin, on its sidebar row and in the header while its page or screen shows:
 * Poll now, Pause or Resume, Stop or Start, each when the plugin has it, then Settings and Logs.
 */
export function agentMenuItems(plugin: PluginSnapshot, read: PluginStatusRead | undefined, pending: AgentAction | null): MenuItem[] {
  const { poll, pause, power } = agentControls(plugin, read, pending);
  const controls = [poll, pause, power].filter((control): control is Control => control !== null);
  return [
    ...controls.map((control) => ({ value: control.action, label: control.label, disabled: control.disabled })),
    { value: "settings", label: "Settings" },
    { value: "logs", label: "Logs" },
  ];
}

/** The items whose status format 2 dot says they run now, which stopping the agent may end. */
export function itemsInProgress(plugin: PluginSnapshot, read: PluginStatusRead | undefined): StatusV2Item[] {
  return readStatusV2(plugin, read)?.items.filter((item) => item.dot === "running") ?? [];
}
