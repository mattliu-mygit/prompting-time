import type { Status } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginStatusRead } from "../../app/store";

/**
 * What a plugin's status action prints, in the format of `whooper-status --json` version 1, which
 * Whooper's header describes field by field. Times are local ISO 8601 with an offset.
 */
export type AgentStatus = {
  version: 1;
  loop: { running: boolean; pid: number | null };
  paused: boolean;
  next_round: string | null;
  round: { running: boolean; started_at: string | null; reviewing: string | null };
  last_round: {
    started_at: string;
    ended_at: string;
    result: "done" | "poll_failed" | "interrupted";
    selected: number | null;
    reviews: { pr: string; outcome: "reviewed" | "failed" | "interrupted" }[];
  } | null;
};

/** The status, when the plugin printed one in a format this version of Prompting Time reads. */
export function readAgentStatus(read: PluginStatusRead | undefined): AgentStatus | null {
  const value = read?.value;
  if (typeof value !== "object" || value === null) return null;
  const status = value as Partial<AgentStatus>;
  const lastRound = status.last_round;
  const fits = status.version === 1
    && typeof status.paused === "boolean"
    && (status.next_round === null || typeof status.next_round === "string")
    && typeof status.round === "object" && status.round !== null && typeof status.round.running === "boolean"
    && (lastRound === null || (typeof lastRound === "object" && typeof lastRound.result === "string" && typeof lastRound.ended_at === "string"));
  return fits ? (status as AgentStatus) : null;
}

/** The programs the plugin needs with its current settings that Prompting Time did not find. */
export function missingPrograms(plugin: PluginSnapshot): string[] {
  return plugin.prerequisites.filter((prerequisite) => prerequisite.needed && prerequisite.path === null).map(({ command }) => command);
}

/** A time from the status as the row shows it: "3:45 PM" today, "Mon 10:00 AM" on another day. */
export function shortTime(iso: string, now: Date): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const time = date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return date.toDateString() === now.toDateString() ? time : `${date.toLocaleDateString([], { weekday: "short" })} ${time}`;
}

/**
 * The one gray line and the dot of a background agent's row, such as "running · next poll 3:45 PM"
 * with a green dot. What stops the plugin from running comes first, then what its status says.
 */
export function describeAgent(plugin: PluginSnapshot, read: PluginStatusRead | undefined, now = new Date()): { meta: string; status: Status } {
  const process = plugin.process;
  if (process.kind === "notRunning") {
    const missing = missingPrograms(plugin);
    if (plugin.settingsError) return { meta: "can't read its settings", status: "error" };
    if (missing.length > 0) return { meta: `missing ${missing.join(", ")}`, status: "attention" };
    if (process.exit && process.exit.code !== 0) {
      return { meta: process.exit.code === null ? "stopped by a signal" : `exited with code ${process.exit.code}`, status: "error" };
    }
    return { meta: "not running", status: "idle" };
  }
  const status = readAgentStatus(read);
  if (!status) return { meta: "running", status: "success" };
  if (status.round.running) {
    return { meta: status.round.reviewing ? `reviewing ${status.round.reviewing}` : "polling now", status: "running" };
  }
  if (status.paused) return { meta: "paused", status: "idle" };
  const next = status.next_round ? shortTime(status.next_round, now) : null;
  if (status.last_round?.result === "poll_failed") {
    return { meta: `poll failed ${shortTime(status.last_round.ended_at, now)}${next ? ` · next try ${next}` : ""}`, status: "error" };
  }
  return { meta: next ? `running · next poll ${next}` : "running", status: "success" };
}
