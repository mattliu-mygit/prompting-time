import type { PluginActions } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";

/** The actions a background agent's row and screen run as Poll now, Pause, and Resume, when its manifest declares them. */
export const ACTIONS = {
  round: "round",
  pause: "pause",
  resume: "resume",
} as const;

export function hasAction(plugin: PluginSnapshot, name: string): boolean {
  return plugin.actions.some((action) => action.name === name && !action.confirm);
}

/**
 * Runs one of the plugin's actions to its end and returns what it printed, trimmed. A failure
 * throws the plugin's own words: its stderr, else its stdout. These callers offer no confirmation,
 * so an action the manifest marks confirm is refused.
 */
export async function runPluginAction(actions: PluginActions, plugin: PluginSnapshot, name: string, args: string[] = []): Promise<string> {
  const action = plugin.actions.find((item) => item.name === name);
  if (!action) throw new Error(`${plugin.title} has no ${name} action.`);
  if (action.confirm) throw new Error(`${plugin.title} asks to confirm ${name} first, which this screen cannot do.`);
  const result = await actions.pluginRequest({ plugin: plugin.name, action: name, args, confirmedFingerprint: null });
  if (result.exitCode !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `${name} exited with ${result.exitCode ?? "a signal"}.`);
  return result.stdout.trim();
}

export function messageOf(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Prompting Time could not run the action.";
}
