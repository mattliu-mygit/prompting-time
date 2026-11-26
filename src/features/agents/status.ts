import type { Status } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginStatusRead } from "../../app/store";
import { readStatusV2 } from "./statusV2";

/**
 * What a plugin's status action prints, in the format of `whooper-status --json` version 1, which
 * Whooper's header describes field by field. Times are local ISO 8601 with an offset. `blocked`
 * and `rounds` came later in version 1, so a status without them reads as having neither.
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
  /** The cause that stops every review now, such as Docker not running: since when, and how many reviews wait on it. */
  blocked?: { id: string; summary: string; scope: "all"; since: string | null; waiting: number } | null;
  /** The last 50 rounds, newest first. */
  rounds?: Round[];
};

/** One round: a poll and the reviews it started. `reason` says why it failed or was skipped. */
export type Round = {
  started_at: string;
  ended_at: string | null;
  result: "done" | "poll_failed" | "interrupted" | "skipped";
  selected: number | null;
  reason: string | null;
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

const ROUND_RESULTS = new Set(["done", "poll_failed", "interrupted", "skipped"]);

/** The cause that blocks every review now, or null when none does or it does not fit. */
export function readBlocked(status: AgentStatus | null): NonNullable<AgentStatus["blocked"]> | null {
  const blocked = status?.blocked;
  return blocked && typeof blocked.id === "string" && typeof blocked.summary === "string" && typeof blocked.waiting === "number"
    ? { ...blocked, since: typeof blocked.since === "string" ? blocked.since : null }
    : null;
}

/** The rounds the status lists, newest first, leaving out any that does not fit. */
export function readRounds(status: AgentStatus | null): Round[] {
  const rounds: unknown = status?.rounds;
  if (!Array.isArray(rounds)) return [];
  return rounds.flatMap((round: Partial<Round> | null): Round[] => (
    round && typeof round.started_at === "string" && typeof round.result === "string" && ROUND_RESULTS.has(round.result)
      ? [{ started_at: round.started_at, ended_at: typeof round.ended_at === "string" ? round.ended_at : null, result: round.result,
        selected: typeof round.selected === "number" ? round.selected : null, reason: typeof round.reason === "string" && round.reason ? round.reason : null }]
      : []
  )).sort((left, right) => right.started_at.localeCompare(left.started_at));
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
 * The status words and the dot of a background agent's pinned row, which its screen, the window's
 * header, and the menu bar extra repeat: red ("error") when it is blocked, failed, or can't start,
 * such as "Can't review · Docker isn't running" with `cause`, the ID of that cause that blocks every
 * review; a dot with no color while a poll or review runs; and no dot otherwise, such as "Running ·
 * next poll 3:45 PM" or "Paused". What stops the plugin from running comes first, then a cause that
 * blocks every review, then what its status says. A plugin that prints status format 2 gives its
 * own words and dot (`agent`).
 */
export function describeAgent(plugin: PluginSnapshot, read: PluginStatusRead | undefined, now = new Date()): { meta: string; status: Status; cause?: string } {
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
  // A plugin that prints status format 2 says its own words and dot.
  const v2 = readStatusV2(plugin, read);
  if (v2) return v2.agent ? { meta: v2.agent.words, status: v2.agent.dot } : { meta: "Running", status: "idle" };
  const status = readAgentStatus(read);
  if (!status) return { meta: "Running", status: "idle" };
  const blocked = readBlocked(status);
  if (blocked) return { meta: `Can't review · ${blocked.summary}`, status: "error", cause: blocked.id };
  if (status.round.running) {
    return { meta: status.round.reviewing ? `Reviewing ${status.round.reviewing}` : "Polling now", status: "running" };
  }
  if (status.paused) return { meta: "Paused", status: "idle" };
  const next = status.next_round ? shortTime(status.next_round, now) : null;
  if (status.last_round?.result === "poll_failed") {
    return { meta: `Poll failed ${shortTime(status.last_round.ended_at, now)}${next ? ` · next try ${next}` : ""}`, status: "error" };
  }
  return { meta: next ? `Running · next poll ${next}` : "Running", status: "idle" };
}
