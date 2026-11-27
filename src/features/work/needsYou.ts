import type { ConversationSummary } from "../../bridge/types";
import { readPreference, savePreference } from "../../app/preferences";
import { blockerKey, itemKey, type StatusV2Blocker, type StatusV2Item, type StatusV2Work } from "../agents/statusV2";

/**
 * One entry of Needs you: a plugin's blocker or item, or a chat waiting on you. `key` is the list's
 * key for it (blockerKey, itemKey, or the conversation's id) and `plugin` its plugin's name, null for
 * a chat. `version` changes when the entry does: an item's `updated_at`, a blocker's title and dot,
 * and nothing for a chat, which changes by no longer waiting.
 */
export type NeedsYouEntry = {
  kind: "blocker" | "item" | "chat";
  key: string;
  plugin: string | null;
  pluginTitle: string | null;
  version: string;
  title: string;
  /** Where its plugin's page shows it; null for a chat. */
  route: string | null;
};

/** An entry the user hid, which stays out of Needs you while it is there with the same `version`. */
export type HiddenEntry = Pick<NeedsYouEntry, "key" | "plugin" | "version">;

/** Which entries Needs you lists: all, the chats, or one plugin's, as `plugin:<name>`. */
export type NeedsYouFilter = "all" | "chats" | `plugin:${string}`;

/** The entries this device hides, until each changes or leaves Needs you. */
const HIDDEN_PREFERENCE = "prompting-time.needs-you-hidden";
/** The filter this device chose for Needs you. */
const FILTER_PREFERENCE = "prompting-time.needs-you-filter";

export const itemVersion = (item: StatusV2Item) => item.updated_at;
export const blockerVersion = (blocker: StatusV2Blocker) => `${blocker.dot} ${blocker.title}`;

/** Everything under Needs you, in Command-J's order: the blockers, the Needs you groups' items, then the chats waiting on you. */
export function needsYouEntries(work: StatusV2Work, chats: readonly ConversationSummary[]): NeedsYouEntry[] {
  return [
    ...work.blockers.map(({ plugin, pluginTitle, blocker }): NeedsYouEntry => ({
      kind: "blocker", key: blockerKey(plugin, blocker.id), plugin, pluginTitle, version: blockerVersion(blocker), title: blocker.title, route: blocker.route,
    })),
    ...work.needsYou.flatMap(({ plugin, pluginTitle, items }) => items.map((item): NeedsYouEntry => ({
      kind: "item", key: itemKey(plugin, item.id), plugin, pluginTitle, version: itemVersion(item), title: item.title, route: item.route,
    }))),
    ...chats.map((chat): NeedsYouEntry => ({ kind: "chat", key: chat.id, plugin: null, pluginTitle: null, version: "", title: chat.title, route: null })),
  ];
}

export function isHidden(hidden: readonly HiddenEntry[], key: string, version: string): boolean {
  return hidden.some((entry) => entry.key === key && entry.version === version);
}

/** Whether `filter` lists the entries of `plugin`, null for the chats. */
export function inFilter(filter: NeedsYouFilter, plugin: string | null): boolean {
  return filter === "all" || filter === (plugin === null ? "chats" : `plugin:${plugin}`);
}

/** The plugins' part of Needs you that `filter` lists, without what the user hid; the groups outside Needs you stay. */
export function narrowWork(work: StatusV2Work, hidden: readonly HiddenEntry[], filter: NeedsYouFilter): StatusV2Work {
  return {
    blockers: work.blockers.filter(({ plugin, blocker }) => inFilter(filter, plugin) && !isHidden(hidden, blockerKey(plugin, blocker.id), blockerVersion(blocker))),
    needsYou: work.needsYou
      .filter(({ plugin }) => inFilter(filter, plugin))
      .map((group) => ({ ...group, items: group.items.filter((item) => !isHidden(hidden, itemKey(group.plugin, item.id), itemVersion(item))) }))
      .filter((group) => group.items.length > 0),
    groups: work.groups,
  };
}

/**
 * The hidden entries to keep: those still in Needs you unchanged, and those whose plugin's status
 * or chat Prompting Time can't read now (`known` is false), which it can't check. An entry that
 * changed or left Needs you shows again the next time it is there.
 */
export function keepHidden(hidden: readonly HiddenEntry[], entries: readonly NeedsYouEntry[], known: (entry: HiddenEntry) => boolean): HiddenEntry[] {
  return hidden.filter((entry) => !known(entry) || entries.some(({ key, version }) => key === entry.key && version === entry.version));
}

const isHiddenEntry = (value: unknown): value is HiddenEntry => typeof value === "object" && value !== null
  && typeof (value as HiddenEntry).key === "string" && typeof (value as HiddenEntry).version === "string"
  && ((value as HiddenEntry).plugin === null || typeof (value as HiddenEntry).plugin === "string");

export function readHidden(): HiddenEntry[] {
  try {
    const value: unknown = JSON.parse(readPreference(HIDDEN_PREFERENCE) ?? "[]");
    return Array.isArray(value) ? value.filter(isHiddenEntry).map(({ key, plugin, version }) => ({ key, plugin, version })) : [];
  } catch {
    return [];
  }
}

export function saveHidden(hidden: readonly HiddenEntry[]): void {
  savePreference(HIDDEN_PREFERENCE, JSON.stringify(hidden));
}

export function readFilter(): NeedsYouFilter {
  const value = readPreference(FILTER_PREFERENCE);
  return value === "chats" || (value?.startsWith("plugin:") && value.length > "plugin:".length) ? value as NeedsYouFilter : "all";
}

export function saveFilter(filter: NeedsYouFilter): void {
  savePreference(FILTER_PREFERENCE, filter);
}
