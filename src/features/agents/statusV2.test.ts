import { describe, expect, it } from "vitest";
import type { PluginsSnapshot, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { fixturePlugin, fixtureStatus, fixtureV2Plugin, fixtureV2Status } from "../../test/plugins";
import { menuBarRequest } from "./menuBar";
import { describeAgent } from "./status";
import { readStatusV2, STATUS_V2_ITEM_LIMIT, statusV2Work } from "./statusV2";

const read = (value: unknown): PluginStatusRead => ({ value, error: null });
const plugins = (items: PluginSnapshot[], statuses: Record<string, unknown> = {}): PluginsSnapshot => ({
  loaded: true, error: null, items,
  statuses: Object.fromEntries(Object.entries(statuses).map(([name, value]): [string, PluginStatusRead] => [name, read(value)])),
});
const item = (overrides: Record<string, unknown>) => ({ ...fixtureV2Status().items[0]!, ...overrides });

describe("status format 2", () => {
  it("reads the agent, blockers, items, and truncated of a plugin whose manifest declares format 2", () => {
    const status = readStatusV2(fixtureV2Plugin(), read(fixtureV2Status({ truncated: true })))!;
    expect(status.agent).toEqual({ words: "Idle · last run 2 min ago", dot: "idle" });
    expect(status.blockers).toEqual([{ id: "codex-signed-out", title: "Codex is signed out", dot: "error", route: "/providers", action: "check-providers" }]);
    expect(status.items.map(({ id }) => id)).toEqual(["run-74c91b1d", "run-review-2", "run-running-3", "run-done-4"]);
    expect(status.items[2]).toEqual({
      id: "run-running-3", group: "running", title: "Score yesterday's sessions", detail: null, dot: "running", project: null,
      updated_at: "2026-11-26T18:50:00Z", route: "/runs/run-running-3", actions: [],
    });
    expect(status.truncated).toBe(true);
    expect(status.paused).toBe(false);
    // Other top-level fields, such as what the plugin's page reads, change nothing.
    expect(readStatusV2(fixtureV2Plugin(), read(fixtureV2Status({ paused: true, reviews: [{ run_id: "r" }] })))!.paused).toBe(true);
  });

  it("reads nothing without format 2 in both the manifest and the output", () => {
    expect(readStatusV2(fixturePlugin({ status: null }), read(fixtureV2Status()))).toBeNull();
    expect(readStatusV2(fixtureV2Plugin(), read({ version: 1, paused: false }))).toBeNull();
    expect(readStatusV2(fixtureV2Plugin(), read(fixtureV2Status({ format: 3 })))).toBeNull();
    expect(readStatusV2(fixtureV2Plugin(), { value: null, error: "status exited with 1" })).toBeNull();
    expect(readStatusV2(fixtureV2Plugin(), undefined)).toBeNull();
    expect(readStatusV2(fixtureV2Plugin({ status: { ...fixtureV2Plugin().status!, format: 1 } }), read(fixtureV2Status()))).toBeNull();
  });

  it("drops what does not fit and keeps the rest", () => {
    const status = readStatusV2(fixtureV2Plugin(), read(fixtureV2Status({
      agent: { words: "Polling", dot: "purple" },
      blockers: [
        { id: "gh", title: "gh is signed out", dot: "error", route: "/providers", action: "login" },
        { id: "no-route", title: "No route", dot: "error" },
        { id: "gh", title: "A second gh", dot: "error", route: "/providers" },
      ],
      items: [
        item({ id: "kept", actions: ["rerun", "submit", "rerun", 3] }),
        item({ id: "unknown-group", group: "archived" }),
        item({ id: "bad-dot", dot: "red" }),
        item({ id: "bad-time", updated_at: "yesterday" }),
        item({ id: "relative-route", route: "runs/1" }),
        item({ id: "" }),
        item({ id: "bad-detail", detail: 4 }),
        item({ id: "bad-actions", actions: "rerun" }),
        item({ id: "kept", title: "A second item with the same id" }),
        "not an item",
      ],
      truncated: "yes",
    })))!;
    expect(status.agent).toBeNull();
    expect(status.blockers).toEqual([{ id: "gh", title: "gh is signed out", dot: "error", route: "/providers", action: null }]);
    expect(status.items.map(({ id }) => id)).toEqual(["kept"]);
    expect(status.items[0]!.actions).toEqual(["rerun"]);
    expect(status.truncated).toBe(false);
  });

  it("reads at most 50 items", () => {
    const many = Array.from({ length: 60 }, (_, index) => item({ id: `run-${index}` }));
    const status = readStatusV2(fixtureV2Plugin(), read(fixtureV2Status({ items: many, truncated: true })))!;
    expect(status.items).toHaveLength(STATUS_V2_ITEM_LIMIT);
    expect(status.items.at(-1)!.id).toBe("run-49");
  });

  it("splits the items into Needs you groups and other groups by the manifest's order, newest first, leaving out empty groups", () => {
    const newer = item({ id: "run-newer", updated_at: "2026-11-26T19:00:00Z" });
    const status = fixtureV2Status({ items: [...fixtureV2Status().items.filter(({ group }) => group !== "review"), newer] });
    const work = statusV2Work(plugins([fixtureV2Plugin(), fixturePlugin({ status: null })], { "fixture-runs": status, whooper: fixtureStatus() }));
    expect(work.blockers).toEqual([{ plugin: "fixture-runs", pluginTitle: "Fixture runs", blocker: expect.objectContaining({ id: "codex-signed-out" }) }]);
    expect(work.needsYou.map(({ label, items }) => [label, items.map(({ id }) => id)])).toEqual([["Run failed", ["run-newer", "run-74c91b1d"]]]);
    expect(work.groups.map(({ label, group, items }) => [label, group.folded, items.map(({ id }) => id)])).toEqual([
      ["Running", false, ["run-running-3"]],
      ["Done", true, ["run-done-4"]],
    ]);
  });

  it("names the plugin in each group's label when more than one plugin prints format 2", () => {
    const other = fixtureV2Plugin({ name: "other-runs", title: "Other runs" });
    const work = statusV2Work(plugins([fixtureV2Plugin(), other], { "fixture-runs": fixtureV2Status(), "other-runs": fixtureV2Status() }));
    expect(work.needsYou.map(({ plugin, label }) => [plugin, label])).toEqual([
      ["fixture-runs", "Fixture runs · Run failed"], ["other-runs", "Other runs · Run failed"],
      ["fixture-runs", "Fixture runs · Review needed"], ["other-runs", "Other runs · Review needed"],
    ]);
  });

  it("gives the agent row the plugin's own words and dot, and marks the menu bar icon while a blocker is listed", () => {
    expect(describeAgent(fixtureV2Plugin(), read(fixtureV2Status()))).toEqual({ meta: "Idle · last run 2 min ago", status: "idle" });
    expect(describeAgent(fixtureV2Plugin(), read(fixtureV2Status({ agent: { words: "Running 2 evaluations", dot: "running" } }))))
      .toEqual({ meta: "Running 2 evaluations", status: "running" });
    // What stops the plugin from running still comes first.
    expect(describeAgent(fixtureV2Plugin({ process: { kind: "notRunning", exit: { code: 2, output: "" } } }), read(fixtureV2Status())).meta).toBe("Exited with code 2");

    const blocked = menuBarRequest(plugins([fixtureV2Plugin()], { "fixture-runs": fixtureV2Status() }), {}, 2, true);
    expect(blocked.agents[0]).toMatchObject({ plugin: "fixture-runs", status: "Idle · last run 2 min ago", alert: true });
    expect(menuBarRequest(plugins([fixtureV2Plugin()], { "fixture-runs": fixtureV2Status({ blockers: [] }) }), {}, 0, true).agents[0]!.alert).toBe(false);
  });
});
