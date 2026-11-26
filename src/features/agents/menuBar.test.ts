import { afterEach, describe, expect, it } from "vitest";
import type { PluginsSnapshot, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { fixturePlugin, fixtureStatus } from "../../test/plugins";
import { menuBarRequest, readMenuBarEnabled, saveMenuBarEnabled } from "./menuBar";

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
  it("shows each agent in its row's words with its row's buttons, and how many items need you", () => {
    expect(menuBarRequest(plugins([whooper()], { whooper: fixtureStatus("Running · next poll 3:45 PM") }), {}, 3, true)).toEqual({
      visible: true,
      needsYou: 3,
      agents: [{
        plugin: "whooper", title: "Whooper", status: "Running · next poll 3:45 PM", alert: false,
        controls: [{ action: "pause", label: "Pause", enabled: true }, { action: "poll", label: "Poll now", enabled: true }],
      }],
    });
  });

  it("follows the row: Resume while the status says paused, Polling… while Poll now runs, and Start while stopped", () => {
    const controls = (snapshot: PluginsSnapshot, pending: Record<string, "poll"> = {}) => menuBarRequest(snapshot, pending, 0, true).agents[0]!.controls;
    expect(controls(plugins([whooper()], { whooper: fixtureStatus("Paused", { paused: true }) }))[0]).toEqual({ action: "resume", label: "Resume", enabled: true });
    expect(controls(plugins([whooper()], { whooper: fixtureStatus() }), { whooper: "poll" })).toEqual([
      { action: "pause", label: "Pause", enabled: false }, { action: "poll", label: "Polling…", enabled: false },
    ]);
    expect(controls(plugins([whooper({ process: { kind: "notRunning", exit: null } })]))).toEqual([
      { action: "start", label: "Start", enabled: true }, { action: "poll", label: "Poll now", enabled: false },
    ]);
    // Without pause, resume, and round actions the row has no buttons but Start.
    expect(controls(plugins([fixturePlugin()], { whooper: fixtureStatus() }))).toEqual([]);
  });

  it("marks the icon while an agent's row is red, lists a blocker, or can't start", () => {
    const failing = fixtureStatus("Poll failed 8:19 AM", { agent: { words: "Poll failed 8:19 AM", dot: "error" } });
    expect(menuBarRequest(plugins([whooper()], { whooper: failing }), {}, 0, true).agents[0]).toMatchObject({ status: "Poll failed 8:19 AM", alert: true });
    const blocked = fixtureStatus("Running", { blockers: [{ id: "docker", title: "Docker isn't running", dot: "error", route: "/causes/docker" }] });
    expect(menuBarRequest(plugins([whooper()], { whooper: blocked }), {}, 0, true).agents[0]!.alert).toBe(true);
    const missing = whooper({ process: { kind: "notRunning", exit: null }, prerequisites: [{ command: "gh", purpose: "Reads pull requests.", needed: true, path: null }] });
    expect(menuBarRequest(plugins([missing]), {}, 0, true).agents[0]).toMatchObject({ status: "Missing gh", alert: true });
    // A dot with no color on the row, as while a poll runs, is not a red one.
    const polling = fixtureStatus("Polling now", { agent: { words: "Polling now", dot: "running" } });
    expect(menuBarRequest(plugins([whooper()], { whooper: polling }), {}, 0, true).agents[0]).toMatchObject({ status: "Polling now", alert: false });
    expect(menuBarRequest(plugins([whooper({ process: { kind: "notRunning", exit: null } })]), {}, 0, true).agents[0]!.alert).toBe(false);
  });

  it("shows only while turned on with an agent installed, and remembers when the user turns it off", () => {
    expect(menuBarRequest(plugins([]), {}, 0, true).visible).toBe(false);
    expect(menuBarRequest(plugins([whooper()]), {}, 0, false).visible).toBe(false);
    expect(readMenuBarEnabled()).toBe(true);
    saveMenuBarEnabled(false);
    expect(readMenuBarEnabled()).toBe(false);
    saveMenuBarEnabled(true);
    expect(readMenuBarEnabled()).toBe(true);
  });
});
