import type { Status } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginStatusRead } from "../../app/store";
import { readStatusV2 } from "./statusV2";

/** The programs the plugin needs with its current settings that Prompting Time did not find. */
export function missingPrograms(plugin: PluginSnapshot): string[] {
  return plugin.prerequisites.filter((prerequisite) => prerequisite.needed && prerequisite.path === null).map(({ command }) => command);
}

/**
 * The status words and the dot of a plugin's row in the sidebar, which the window's header and
 * the menu bar extra repeat. What stops the plugin from running comes first, in red,
 * such as "Missing gh"; then the words and dot its status format 2 gives (`agent`), such as
 * "Running · next poll 3:45 PM"; and "Running" with no dot for a plugin that prints no status
 * Prompting Time reads.
 */
export function describeAgent(plugin: PluginSnapshot, read: PluginStatusRead | undefined): { meta: string; status: Status } {
  const process = plugin.process;
  if (process.kind === "notRunning") {
    const missing = missingPrograms(plugin);
    if (plugin.settingsError) return { meta: "Can't read its settings", status: "error" };
    if (missing.length > 0) return { meta: `Missing ${missing.join(", ")}`, status: "error" };
    if (process.exit && process.exit.code !== 0) {
      return { meta: process.exit.code === null ? "Stopped by a signal" : `Exited with code ${process.exit.code}`, status: "error" };
    }
    return { meta: "Stopped", status: "idle" };
  }
  const agent = readStatusV2(plugin, read)?.agent;
  return agent ? { meta: agent.words, status: agent.dot } : { meta: "Running", status: "idle" };
}
