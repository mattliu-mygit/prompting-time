// A plugin's settings form without its controls: the fields drawn from the JSON Schema a plugin
// ships with its settings (its manifest's settings schema), the value each control shows for a
// setting, and the settings a filled-in form changed. Prompting Time's Settings tab draws its form
// from these, and a plugin page can draw its own the same way. The form edits these kinds of
// setting: objects (as groups), numbers and integers, strings, booleans, a string enum, an array
// of a string enum, and an array of strings; a setting of any other kind is left out of the form
// and keeps the value the plugin's config gives it. The form checks only that a number lies in the
// range its schema gives (rangeError); the plugin's save-settings checks everything else and
// refuses what it does not take.

/** One field of the form, or a group of them. `path` is the setting's place in the settings object. */
export type SettingField =
  | { kind: "group"; path: string[]; title: string; description: string | null; fields: SettingField[] }
  | {
      kind: "integer" | "number";
      path: string[];
      title: string;
      description: string | null;
      unit: string | null;
      minimum: number | null;
      exclusiveMinimum: number | null;
      maximum: number | null;
      exclusiveMaximum: number | null;
    }
  | { kind: "text" | "list" | "switch"; path: string[]; title: string; description: string | null }
  | { kind: "choice" | "choices"; path: string[]; title: string; description: string | null; options: string[] };

export type LeafField = Exclude<SettingField, { kind: "group" }>;
/** What a field's control holds: text for inputs, the chosen options, or whether a switch is on. */
export type FormValue = string | string[] | boolean;

type Schema = {
  type?: string;
  title?: string;
  description?: string;
  properties?: Record<string, Schema>;
  enum?: unknown[];
  items?: Schema;
  minimum?: unknown;
  exclusiveMinimum?: unknown;
  maximum?: unknown;
  exclusiveMaximum?: unknown;
};

/** The form's top-level fields; an empty list when the schema describes no settings object. */
export function settingFields(schemaText: string): SettingField[] {
  let schema: Schema;
  try {
    schema = JSON.parse(schemaText) as Schema;
  } catch {
    return [];
  }
  return fieldsOf(schema, []);
}

function fieldsOf(schema: Schema, path: string[]): SettingField[] {
  return Object.entries(schema.properties ?? {}).flatMap(([key, child]) => {
    const field = fieldFor(child, [...path, key], key);
    return field ? [field] : [];
  });
}

function fieldFor(schema: Schema, path: string[], key: string): SettingField | null {
  const title = schema.title ?? key;
  const description = schema.description ?? null;
  if (schema.properties) return { kind: "group", path, title, description, fields: fieldsOf(schema, path) };
  const options = stringOptions(schema.enum);
  if (options) return { kind: "choice", path, title, description, options };
  switch (schema.type) {
    case "integer":
    case "number": {
      // "Poll every (minutes)" reads as the label "Poll every" with the unit after the input.
      const unit = /^(.*\S)\s*\(([^()]+)\)$/.exec(title);
      return {
        kind: schema.type,
        path,
        title: unit?.[1] ?? title,
        unit: unit?.[2] ?? null,
        description,
        minimum: bound(schema.minimum),
        exclusiveMinimum: bound(schema.exclusiveMinimum),
        maximum: bound(schema.maximum),
        exclusiveMaximum: bound(schema.exclusiveMaximum),
      };
    }
    case "string":
      return { kind: "text", path, title, description };
    case "boolean":
      return { kind: "switch", path, title, description };
    case "array": {
      const itemOptions = stringOptions(schema.items?.enum);
      if (itemOptions) return { kind: "choices", path, title, description, options: itemOptions };
      if (schema.items?.type === "string") return { kind: "list", path, title, description };
      return null;
    }
    default:
      return null;
  }
}

/**
 * A range keyword's bound in draft 2020-12's form, a number; null when the schema has none or gives
 * draft 4's boolean `exclusiveMinimum` or `exclusiveMaximum`, which the form leaves to the plugin.
 */
function bound(value: unknown): number | null {
  return typeof value === "number" ? value : null;
}

function stringOptions(values: unknown[] | undefined): string[] | null {
  return Array.isArray(values) && values.length > 0 && values.every((value) => typeof value === "string") ? values as string[] : null;
}

export function leafFields(fields: readonly SettingField[]): LeafField[] {
  return fields.flatMap((field) => (field.kind === "group" ? leafFields(field.fields) : [field]));
}

export function fieldKey(field: LeafField): string {
  return JSON.stringify(field.path);
}

/** The setting at `path` in the settings the plugin printed, or undefined when it has none. */
export function valueAt(values: unknown, path: readonly string[]): unknown {
  return path.reduce<unknown>((value, key) => (typeof value === "object" && value !== null ? (value as Record<string, unknown>)[key] : undefined), values);
}

/** The control's value for a setting's value. A list shows one item per line. */
export function formValue(field: LeafField, value: unknown): FormValue {
  switch (field.kind) {
    case "integer":
    case "number":
      return typeof value === "number" ? String(value) : "";
    case "text":
    case "choice":
      return typeof value === "string" ? value : "";
    case "choices":
      return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
    case "list":
      return Array.isArray(value) ? value.filter((item) => typeof item === "string").join("\n") : "";
    case "switch":
      return value === true;
  }
}

/**
 * The setting's value for what the control holds. An emptied number becomes null, and the plugin
 * decides whether it takes that.
 */
export function settingValue(field: LeafField, value: FormValue): unknown {
  switch (field.kind) {
    case "integer":
    case "number":
      return String(value).trim() === "" ? null : Number(value);
    case "text":
    case "choice":
      return String(value);
    case "choices":
      return field.options.filter((option) => (value as string[]).includes(option));
    case "list":
      return String(value).split("\n").map((line) => line.trim()).filter(Boolean);
    case "switch":
      return value === true;
  }
}

/**
 * What a number must be when its control holds a value outside the range its schema's `minimum`,
 * `exclusiveMinimum`, `maximum`, and `exclusiveMaximum` give, such as "Must be above 0." for
 * `"exclusiveMinimum": 0`; null when the value is in range, the control is empty, or the field holds
 * no number. Integers and numbers take the same bounds; whether an integer is whole, like every
 * other check, is the plugin's.
 */
export function rangeError(field: LeafField, value: FormValue): string | null {
  if (field.kind !== "integer" && field.kind !== "number") return null;
  const number = settingValue(field, value);
  if (typeof number !== "number") return null;
  if (field.minimum !== null && number < field.minimum) return `Must be at least ${field.minimum}.`;
  if (field.exclusiveMinimum !== null && number <= field.exclusiveMinimum) return `Must be above ${field.exclusiveMinimum}.`;
  if (field.maximum !== null && number > field.maximum) return `Must be at most ${field.maximum}.`;
  if (field.exclusiveMaximum !== null && number >= field.exclusiveMaximum) return `Must be below ${field.exclusiveMaximum}.`;
  return null;
}

/**
 * Only the settings the form changed, nested as the settings object nests them, such as
 * `{"schedule": {"interval_minutes": 30}}`, so the plugin rewrites only those lines of its config.
 */
export function changedSettings(fields: readonly SettingField[], saved: unknown, draft: Readonly<Record<string, FormValue>>): Record<string, unknown> {
  const changes: Record<string, unknown> = {};
  for (const field of leafFields(fields)) {
    const value = draft[fieldKey(field)];
    if (value === undefined) continue;
    const next = settingValue(field, value);
    // Both sides go through the control's value, so "15.0" for 15, or a list's trailing blank line, is no change.
    if (JSON.stringify(next) === JSON.stringify(settingValue(field, formValue(field, valueAt(saved, field.path))))) continue;
    let target = changes;
    for (const key of field.path.slice(0, -1)) target = (target[key] ??= {}) as Record<string, unknown>;
    target[field.path.at(-1)!] = next;
  }
  return changes;
}
