import type { Status } from "@prompting-time/ui";
import type { PluginSnapshot, PluginStatusConfig, PluginStatusGroup } from "../../bridge/types";
import type { PluginsSnapshot, PluginStatusRead } from "../../app/store";

/**
 * Status format 2: what a plugin's status action prints when its manifest declares
 * `"status": {"format": 2, ...}`. `docs/plugin-status-v2.schema.json` is the format's schema. The
 * manifest names the groups and the row actions; the output fills them in.
 */
export type StatusV2 = {
  format: 2;
  /** The plugin's row: a few status words and a dot; null when the output's `agent` does not fit. */
  agent: { words: string; dot: Status } | null;
  /** Problems that stop every job, drawn above Needs you and in the menu bar badge. */
  blockers: StatusV2Blocker[];
  /** At most 50, each in a group the manifest declares. */
  items: StatusV2Item[];
  /** Whether the plugin left items out to stay within 50. */
  truncated: boolean;
  /** Whether the plugin says it is paused, which offers its resume action in place of pause. */
  paused: boolean;
};

export type StatusV2Blocker = {
  id: string;
  title: string;
  dot: Status;
  route: string;
  /** One of the manifest's actions that fixes or rechecks the blocker, run with no extra arguments. */
  action: string | null;
};

export type StatusV2Item = {
  id: string;
  /** One of the manifest's `status.groups` ids. */
  group: string;
  title: string;
  /** One line of muted text. */
  detail: string | null;
  dot: Status;
  /** The project's path, if the item belongs to one. */
  project: string | null;
  updated_at: string;
  route: string;
  /** The row actions that apply to it, each among the manifest's `status.item_actions`. */
  actions: string[];
};

/** The most items Prompting Time reads from one status; the plugin's page keeps the full history. */
export const STATUS_V2_ITEM_LIMIT = 50;

const DOTS = new Set<string>(["error", "attention", "success", "running", "idle"]);

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const isText = (value: unknown): value is string => typeof value === "string" && value !== "";
const isDot = (value: unknown): value is Status => typeof value === "string" && DOTS.has(value);
const isRoute = (value: unknown): value is string => typeof value === "string" && value.startsWith("/");
const textOrNull = (value: unknown): string | null => (typeof value === "string" && value !== "" ? value : null);
const fitsOptional = (value: unknown) => value === undefined || value === null || typeof value === "string";

/** The manifest's `status` key, when the plugin's status action prints status format 2. */
export function statusConfig(plugin: PluginSnapshot): PluginStatusConfig | null {
  return plugin.status?.format === 2 ? plugin.status : null;
}

/**
 * The plugin's status in format 2, or null when its manifest does not declare format 2 or the
 * output is not format 2. It reads defensively: a part that does not fit is left out and the rest
 * kept. An item in a group the manifest does not declare, past the 50th, or with an id already
 * listed is left out; row actions the manifest does not allow are dropped from their item, and a
 * blocker's action the manifest does not declare is dropped from its blocker.
 */
export function readStatusV2(plugin: PluginSnapshot, read: PluginStatusRead | undefined): StatusV2 | null {
  const config = statusConfig(plugin);
  const value = read?.value;
  if (!config || !isRecord(value) || value.format !== 2) return null;
  const agent = isRecord(value.agent) && typeof value.agent.words === "string" && isDot(value.agent.dot)
    ? { words: value.agent.words, dot: value.agent.dot }
    : null;
  const declared = new Set(plugin.actions.map((action) => action.name));
  const groups = new Set(config.groups.map((group) => group.id));
  const allowed = new Set(config.itemActions.filter((name) => declared.has(name)));
  const blockerIds = new Set<string>();
  const blockers = (Array.isArray(value.blockers) ? value.blockers : []).flatMap((blocker: unknown): StatusV2Blocker[] => {
    if (!isRecord(blocker) || !isText(blocker.id) || !isText(blocker.title) || !isDot(blocker.dot) || !isRoute(blocker.route)
      || !fitsOptional(blocker.action) || blockerIds.has(blocker.id)) return [];
    blockerIds.add(blocker.id);
    const action = textOrNull(blocker.action);
    return [{ id: blocker.id, title: blocker.title, dot: blocker.dot, route: blocker.route, action: action && declared.has(action) ? action : null }];
  });
  const itemIds = new Set<string>();
  const items: StatusV2Item[] = [];
  for (const item of Array.isArray(value.items) ? value.items : []) {
    if (items.length === STATUS_V2_ITEM_LIMIT) break;
    if (!isRecord(item) || !isText(item.id) || !isText(item.group) || !groups.has(item.group) || !isText(item.title) || !isDot(item.dot)
      || typeof item.updated_at !== "string" || Number.isNaN(Date.parse(item.updated_at)) || !isRoute(item.route)
      || !fitsOptional(item.detail) || !fitsOptional(item.project) || !(item.actions === undefined || Array.isArray(item.actions))
      || itemIds.has(item.id)) continue;
    itemIds.add(item.id);
    const actions: unknown[] = Array.isArray(item.actions) ? item.actions : [];
    items.push({
      id: item.id, group: item.group, title: item.title, detail: textOrNull(item.detail), dot: item.dot, project: textOrNull(item.project),
      updated_at: item.updated_at, route: item.route,
      actions: [...new Set(actions.filter((name): name is string => typeof name === "string" && allowed.has(name)))],
    });
  }
  return { format: 2, agent, blockers, items, truncated: value.truncated === true, paused: value.paused === true };
}

/** A blocker and the plugin that reports it. */
export type PluginBlocker = { plugin: string; pluginTitle: string; blocker: StatusV2Blocker };
/** One of a plugin's declared groups with its items, newest first. `label` adds the plugin's title when more than one plugin prints format 2. */
export type PluginGroup = { plugin: string; pluginTitle: string; group: PluginStatusGroup; label: string; items: StatusV2Item[] };
/** What the plugins that print status format 2 add to All work. */
export type StatusV2Work = {
  blockers: PluginBlocker[];
  /** The groups whose items wait on the user, each with items, by `order`: parts of Needs you. */
  needsYou: PluginGroup[];
  /** The other groups with items, by `order`: sections of their own. */
  groups: PluginGroup[];
};

export const NO_STATUS_V2_WORK: StatusV2Work = { blockers: [], needsYou: [], groups: [] };

/** The key of a plugin's format-2 item among every plugin's, such as "item:monopole:run-74c91b1d". */
export function itemKey(plugin: string, id: string): string {
  return `item:${plugin}:${id}`;
}

/** The key of a plugin's format-2 blocker, such as "blocker:monopole:codex-signed-out". */
export function blockerKey(plugin: string, id: string): string {
  return `blocker:${plugin}:${id}`;
}

/** The key of a plugin's declared group, which names its part or section. */
export function groupKey(plugin: string, id: string): string {
  return `group:${plugin}:${id}`;
}

const newestFirst = (left: StatusV2Item, right: StatusV2Item) => Date.parse(right.updated_at) - Date.parse(left.updated_at);

/**
 * The blockers and grouped items of every plugin that prints status format 2, in plugin order.
 * Groups list by their `order`, and a tie keeps the plugins' and the manifest's order; a group with
 * no items is left out. Items in a group list newest first.
 */
export function statusV2Work(plugins: PluginsSnapshot): StatusV2Work {
  const read = plugins.items.flatMap((plugin) => {
    const status = readStatusV2(plugin, plugins.statuses[plugin.name]);
    return status ? [{ plugin, status }] : [];
  });
  const several = read.length > 1;
  const groups = read.flatMap(({ plugin, status }) => statusConfig(plugin)!.groups.map((group): PluginGroup => ({
    plugin: plugin.name,
    pluginTitle: plugin.title,
    group,
    label: several ? `${plugin.title} · ${group.title}` : group.title,
    items: status.items.filter((item) => item.group === group.id).sort(newestFirst),
  }))).filter((group) => group.items.length > 0).sort((left, right) => left.group.order - right.group.order);
  return {
    blockers: read.flatMap(({ plugin, status }) => status.blockers.map((blocker) => ({ plugin: plugin.name, pluginTitle: plugin.title, blocker }))),
    needsYou: groups.filter(({ group }) => group.needsYou),
    groups: groups.filter(({ group }) => !group.needsYou),
  };
}
