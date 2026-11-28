import { describe, expect, it } from "vitest";
import { changedSettings, fieldKey, formValue, leafFields, settingFields, settingValue, valueAt } from "./settingsForm";

const schema = JSON.stringify({
  type: "object",
  properties: {
    schedule: {
      title: "Polling",
      type: "object",
      properties: {
        interval_minutes: { title: "Poll every (minutes)", type: "integer", minimum: 1 },
        days: { title: "Days", type: "array", items: { enum: ["mon", "tue", "wed"] } },
      },
    },
    agent: { title: "Agent", enum: ["claude", "devin"] },
    skills: { title: "Skills", type: "array", items: { type: "string" } },
    verbose: { type: "boolean" },
    extra: { type: "array", items: { type: "object" } },
  },
});
const saved = { schedule: { interval_minutes: 15, days: ["mon"] }, agent: "devin", skills: ["review"], verbose: false };

describe("the settings form", () => {
  it("draws the fields a schema describes and leaves out the kinds it cannot edit", () => {
    const fields = settingFields(schema);
    expect(fields.map((field) => [field.kind, field.title])).toEqual([["group", "Polling"], ["choice", "Agent"], ["list", "Skills"], ["switch", "verbose"]]);
    expect(leafFields(fields).map((field) => field.path)).toEqual([["schedule", "interval_minutes"], ["schedule", "days"], ["agent"], ["skills"], ["verbose"]]);
    expect(leafFields(fields)[0]).toMatchObject({ kind: "integer", title: "Poll every", unit: "minutes", minimum: 1 });
    expect(settingFields("not json")).toEqual([]);
  });

  it("shows each setting in its control and reads it back", () => {
    const [interval, days, , skills] = leafFields(settingFields(schema));
    expect(formValue(interval!, valueAt(saved, interval!.path))).toBe("15");
    expect(formValue(skills!, ["review", "style"])).toBe("review\nstyle");
    expect(settingValue(interval!, " ")).toBeNull();
    expect(settingValue(days!, ["wed", "mon"])).toEqual(["mon", "wed"]);
    expect(settingValue(skills!, "review\n\n style \n")).toEqual(["review", "style"]);
  });

  it("sends only the settings the form changed, nested as the settings nest", () => {
    const fields = settingFields(schema);
    const [interval, days, agent] = leafFields(fields);
    const draft = { [fieldKey(interval!)]: "15.0", [fieldKey(days!)]: ["mon", "tue"], [fieldKey(agent!)]: "claude" };
    expect(changedSettings(fields, saved, draft)).toEqual({ schedule: { days: ["mon", "tue"] }, agent: "claude" });
    expect(changedSettings(fields, saved, {})).toEqual({});
  });
});
