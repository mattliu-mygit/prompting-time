import { describe, expect, it } from "vitest";
import { changedSettings, fieldKey, formValue, leafFields, rangeError, settingFields, settingValue, valueAt, type LeafField } from "./settingsForm";

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

/** The one field of a schema whose only setting is `limit`. */
function limitField(limit: Record<string, unknown>): LeafField {
  return leafFields(settingFields(JSON.stringify({ type: "object", properties: { limit } })))[0]!;
}

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

  it("reads a number's range from minimum, exclusiveMinimum, maximum, and exclusiveMaximum", () => {
    expect(limitField({ type: "number", minimum: 1, exclusiveMinimum: 0, maximum: 10, exclusiveMaximum: 11 }))
      .toMatchObject({ minimum: 1, exclusiveMinimum: 0, maximum: 10, exclusiveMaximum: 11 });
    expect(limitField({ type: "integer" })).toMatchObject({ minimum: null, exclusiveMinimum: null, maximum: null, exclusiveMaximum: null });
    // Draft 4's boolean form is no bound in draft 2020-12, so the plugin checks it.
    expect(limitField({ type: "integer", minimum: 0, exclusiveMinimum: true, maximum: 9, exclusiveMaximum: true }))
      .toMatchObject({ minimum: 0, exclusiveMinimum: null, maximum: 9, exclusiveMaximum: null });
  });

  it.each(["integer", "number"])("refuses an %s outside each bound and says the bound in words", (type) => {
    const check = (bound: Record<string, number>, value: string) => rangeError(limitField({ type, ...bound }), value);
    expect(check({ minimum: 5 }, "4")).toBe("Must be at least 5.");
    expect(check({ minimum: 5 }, "5")).toBeNull();
    expect(check({ exclusiveMinimum: 0 }, "-1")).toBe("Must be above 0.");
    expect(check({ exclusiveMinimum: 0 }, "0")).toBe("Must be above 0.");
    expect(check({ exclusiveMinimum: 0 }, "1")).toBeNull();
    expect(check({ maximum: 120 }, "121")).toBe("Must be at most 120.");
    expect(check({ maximum: 120 }, "120")).toBeNull();
    expect(check({ exclusiveMaximum: 100 }, "100")).toBe("Must be below 100.");
    expect(check({ exclusiveMaximum: 100 }, "99")).toBeNull();
    // An emptied number goes to the plugin as null, and the plugin decides whether it takes that.
    expect(check({ minimum: 5 }, "")).toBeNull();
  });

  it("takes a number's fraction inside the range, and leaves an integer's fraction to the plugin", () => {
    const above = limitField({ type: "number", exclusiveMinimum: 0 });
    expect(rangeError(above, "0.5")).toBeNull();
    expect(rangeError(above, "-0.5")).toBe("Must be above 0.");
    const half = limitField({ type: "number", maximum: 0.5 });
    expect(rangeError(half, "0.5")).toBeNull();
    expect(rangeError(half, "0.51")).toBe("Must be at most 0.5.");
    // 5.5 is no whole number, but it is in range: the plugin's save-settings refuses it.
    const whole = limitField({ type: "integer", minimum: 5 });
    expect(rangeError(whole, "5.5")).toBeNull();
    expect(rangeError(whole, "4.5")).toBe("Must be at least 5.");
  });

  it("sends only the settings the form changed, nested as the settings nest", () => {
    const fields = settingFields(schema);
    const [interval, days, agent] = leafFields(fields);
    const draft = { [fieldKey(interval!)]: "15.0", [fieldKey(days!)]: ["mon", "tue"], [fieldKey(agent!)]: "claude" };
    expect(changedSettings(fields, saved, draft)).toEqual({ schedule: { days: ["mon", "tue"] }, agent: "claude" });
    expect(changedSettings(fields, saved, {})).toEqual({});
  });
});
