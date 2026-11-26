import type { PluginsSnapshot } from "../../app/store";
import { readPreference, savePreference } from "../../app/preferences";
import type { MenuBarAction, MenuBarControl, UpdateMenuBarRequest } from "../../bridge/types";
import { rowButtons, type AgentAction } from "./agentMenu";
import { describeAgent } from "./status";

/** Whether the user keeps the background agents in the macOS menu bar, which this device remembers. */
const MENU_BAR_PREFERENCE = "prompting-time.menu-bar";

/** On unless the user turned it off. */
export function readMenuBarEnabled(): boolean {
  return readPreference(MENU_BAR_PREFERENCE) !== "off";
}

export function saveMenuBarEnabled(enabled: boolean): void {
  savePreference(MENU_BAR_PREFERENCE, enabled ? "on" : "off");
}

const isMenuBarAction = (action: AgentAction): action is MenuBarAction => action === "pause" || action === "resume" || action === "start" || action === "poll";

/**
 * What the menu bar extra shows: each background agent as its pinned sidebar row shows it, its
 * status words, the cause they name, and its row's buttons with what runs now (`pending`), and how
 * many reviews need you. An agent whose row is red, because it fails, is blocked, or can't start,
 * puts a dot on the icon. The extra shows while the user keeps it on and at least one background
 * agent is installed.
 */
export function menuBarRequest(plugins: PluginsSnapshot, pending: Readonly<Record<string, AgentAction>>, reviewsNeedingYou: number, enabled: boolean, now = new Date()): UpdateMenuBarRequest {
  const agents = plugins.items.map((plugin) => {
    const status = plugins.statuses[plugin.name];
    const words = describeAgent(plugin, status, now);
    return {
      plugin: plugin.name,
      title: plugin.title,
      status: words.meta,
      cause: words.cause ?? null,
      alert: words.status === "error",
      controls: rowButtons(plugin, status, pending[plugin.name] ?? null).flatMap((control): MenuBarControl[] => (
        control && isMenuBarAction(control.action) ? [{ action: control.action, label: control.label, enabled: !control.disabled }] : []
      )),
    };
  });
  return { visible: enabled && agents.length > 0, agents, reviewsNeedingYou };
}
