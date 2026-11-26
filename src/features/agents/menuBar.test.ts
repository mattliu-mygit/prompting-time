import { afterEach, describe, expect, it } from "vitest";
import type { PluginsSnapshot, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { fixturePlugin, fixtureStatus } from "../../test/plugins";
import { menuBarRequest, readMenuBarEnabled, saveMenuBarEnabled } from "./menuBar";

const now = new Date(2026, 10, 26, 9, 0);
const nextPoll = new Date(2026, 10, 26, 15, 45).toISOString();
const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const controlActions = [
  { name: "round", title: "Poll now", description: "Runs a round now.", confirm: false },
  { name: "pause", title: "Pause", description: "Pauses the schedule.", confirm: false },
  { name: "resume", title: "Resume", description: "Resumes the schedule.", confirm: false },
];
const whooper = (overrides: Partial<PluginSnapshot> = {}) => fixturePlugin({ actions: [...fixturePlugin().actions, ...controlActions], ...overrides });
const plugins = (items: PluginSnapshot[], statuses: Record<string, unknown> = {}): PluginsSnapshot => ({
  loaded: true, error: null, items,
  statuses: Object.fromEntries(Object.entries(statuses).map(([name, value]): [string, PluginStatusRead] => [name, { value, error: null }])),
});

afterEach(() => window.localStorage.clear());

describe("the menu bar extra", () => {
  it("shows each agent in its row's words with its row's buttons, and how many reviews need you", () => {
    expect(menuBarRequest(plugins([whooper()], { whooper: fixtureStatus(nextPoll) }), {}, 3, true, now)).toEqual({
      visible: true,
      reviewsNeedingYou: 3,
      agents: [{
        plugin: "whooper", title: "Whooper", status: `Running · next poll ${time(nextPoll)}`, cause: null, alert: false,
        controls: [{ action: "pause", label: "Pause", enabled: true }, { action: "poll", label: "Poll now", enabled: true }],
      }],
    });
  });

  it("follows the row: Resume while paused, Polling… while a poll runs, and Start while stopped", () => {
    const controls = (snapshot: PluginsSnapshot, pending: Record<string, "poll"> = {}) => menuBarRequest(snapshot, pending, 0, true, now).agents[0]!.controls;
    expect(controls(plugins([whooper()], { whooper: fixtureStatus(nextPoll, { paused: true }) }))[0]).toEqual({ action: "resume", label: "Resume", enabled: true });
    expect(controls(plugins([whooper()], { whooper: fixtureStatus(nextPoll) }), { whooper: "poll" })).toEqual([
      { action: "pause", label: "Pause", enabled: false }, { action: "poll", label: "Polling…", enabled: false },
    ]);
    expect(controls(plugins([whooper({ process: { kind: "notRunning", exit: null } })]))).toEqual([
      { action: "start", label: "Start", enabled: true }, { action: "poll", label: "Poll now", enabled: false },
    ]);
    // Without pause, resume, and round actions the row has no buttons but Start.
    expect(controls(plugins([fixturePlugin()], { whooper: fixtureStatus(nextPoll) }))).toEqual([]);
  });

  it("marks the icon while an agent is blocked, fails, or can't start, and names the cause the words open", () => {
    const blocked = fixtureStatus(nextPoll, { blocked: { id: "docker", summary: "Docker isn't running", scope: "all", since: null, waiting: 4 } });
    expect(menuBarRequest(plugins([whooper()], { whooper: blocked }), {}, 0, true, now).agents[0]).toMatchObject({
      status: "Can't review · Docker isn't running", cause: "docker", alert: true,
    });
    const failedPoll = fixtureStatus(nextPoll, { last_round: { started_at: nextPoll, ended_at: nextPoll, result: "poll_failed", selected: null, reviews: [] } });
    expect(menuBarRequest(plugins([whooper()], { whooper: failedPoll }), {}, 0, true, now).agents[0]!.alert).toBe(true);
    const missing = whooper({ process: { kind: "notRunning", exit: null }, prerequisites: [{ command: "gh", purpose: "Reads pull requests.", needed: true, path: null }] });
    expect(menuBarRequest(plugins([missing]), {}, 0, true, now).agents[0]).toMatchObject({ status: "Missing gh", alert: true });
    expect(menuBarRequest(plugins([whooper()], { whooper: fixtureStatus(nextPoll, { paused: true }) }), {}, 0, true, now).agents[0]!.alert).toBe(false);
    expect(menuBarRequest(plugins([whooper({ process: { kind: "notRunning", exit: null } })]), {}, 0, true, now).agents[0]!.alert).toBe(false);
  });

  it("shows only while turned on with an agent installed, and remembers when the user turns it off", () => {
    expect(menuBarRequest(plugins([]), {}, 0, true, now).visible).toBe(false);
    expect(menuBarRequest(plugins([whooper()]), {}, 0, false, now).visible).toBe(false);
    expect(readMenuBarEnabled()).toBe(true);
    saveMenuBarEnabled(false);
    expect(readMenuBarEnabled()).toBe(false);
    saveMenuBarEnabled(true);
    expect(readMenuBarEnabled()).toBe(true);
  });
});
