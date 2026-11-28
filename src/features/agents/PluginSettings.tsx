import { createMemo, createSignal, createUniqueId, For, onSettled, Show, untrack } from "solid-js";
import { Button, changedSettings, Checkbox, fieldKey, formValue, NumberField, Select, settingFields, StatusDot, Textarea, TextField, valueAt, type FormValue, type LeafField, type SettingField } from "@prompting-time/ui";
import type { PluginSnapshot, PluginSource } from "../../bridge/types";
import type { PluginActions, PluginStatusRead } from "../../app/store";
import { describeAgent, missingPrograms } from "./status";

/** Prints the settings as one JSON object; the host's convention, as Whooper's whooper-settings get. */
const GET_SETTINGS = "get-settings";
/** Checks and saves the settings given as one JSON argument; the host's save_plugin_settings runs it. */
const SAVE_SETTINGS = "save-settings";

type Props = {
  plugin: PluginSnapshot;
  status: PluginStatusRead | undefined;
  actions: PluginActions;
};

/**
 * A background agent's Settings tab: what it is and where it came from, whether its process runs
 * and who started it, the programs it needs, and its settings in a form drawn from its settings
 * schema. The plugin
 * checks and saves the settings; a refusal shows the plugin's own words. The caller makes one per
 * plugin.
 */
export function PluginSettings(props: Props) {
  const name = untrack(() => props.plugin.name);
  const fields = createMemo(() => settingFields(props.plugin.settingsSchema));
  const editable = () => [GET_SETTINGS, SAVE_SETTINGS].every((action) => props.plugin.actions.some((item) => item.name === action && !item.confirm));
  const [saved, setSaved] = createSignal<Record<string, unknown> | null>(null);
  const [loadError, setLoadError] = createSignal<string | null>(null);
  const [draft, setDraft] = createSignal<Readonly<Record<string, FormValue>>>({});
  const [saving, setSaving] = createSignal(false);
  const [saveError, setSaveError] = createSignal<string | null>(null);
  const [savedNote, setSavedNote] = createSignal(false);
  let disposed = false;

  const changes = createMemo(() => changedSettings(fields(), saved(), draft()));
  const dirty = () => Object.keys(changes()).length > 0;
  const value = (field: LeafField) => draft()[fieldKey(field)] ?? formValue(field, valueAt(saved(), field.path));

  async function readSettings(): Promise<Record<string, unknown>> {
    const result = await props.actions.pluginRequest({ plugin: name, action: GET_SETTINGS, args: [], confirmedFingerprint: null });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || `${GET_SETTINGS} exited with ${result.exitCode ?? "a signal"}.`);
    let settings: unknown;
    try {
      settings = JSON.parse(result.stdout);
    } catch {
      settings = null;
    }
    if (typeof settings !== "object" || settings === null || Array.isArray(settings)) {
      throw new Error(`${props.plugin.title} did not print its settings as JSON.`);
    }
    return settings as Record<string, unknown>;
  }

  async function load() {
    setLoadError(null);
    try {
      const settings = await readSettings();
      if (!disposed) setSaved(settings);
    } catch (reason) {
      if (!disposed) setLoadError(messageFor(reason));
    }
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!dirty() || saving()) return;
    setSaving(true);
    setSaveError(null);
    setSavedNote(false);
    try {
      const result = await props.actions.savePluginSettings({ plugin: name, values: JSON.stringify(changes()) });
      if (disposed) return;
      if (result.exitCode !== 0) {
        setSaveError(result.stderr.trim() || result.stdout.trim() || `${props.plugin.title} refused the settings.`);
        return;
      }
      // Show what the plugin saved, which it may have written differently than the form sent.
      const settings = await readSettings();
      if (disposed) return;
      setSaved(settings);
      setDraft({});
      setSavedNote(true);
    } catch (reason) {
      if (!disposed) setSaveError(messageFor(reason));
    } finally {
      if (!disposed) setSaving(false);
    }
  }

  function change(field: LeafField, next: FormValue) {
    setDraft({ ...draft(), [fieldKey(field)]: next });
    setSavedNote(false);
  }

  onSettled(() => {
    if (untrack(editable)) void load();
    return () => {
      disposed = true;
    };
  });

  return (
    // novalidate: the plugin checks the values, so a number under the schema's minimum still goes to it and its refusal shows.
    <form class="plugin-settings" aria-label={`${props.plugin.title} settings`} aria-busy={saving() ? "true" : "false"} novalidate onSubmit={(event) => void save(event)}>
      <div class="plugin-scroll">
        <div class="plugin-column">
          <section class="plugin-section" aria-labelledby="plugin-about-heading">
            <h3 id="plugin-about-heading" class="section-heading">About</h3>
            <p>{props.plugin.description}</p>
            <p class="plugin-meta">Version {props.plugin.version} · {sourceText(props.plugin.source)}</p>
          </section>
          <ProcessState plugin={props.plugin} status={props.status} />
          <Programs plugin={props.plugin} />
          <section class="plugin-section" aria-labelledby="plugin-settings-heading">
            <h3 id="plugin-settings-heading" class="section-heading">Settings</h3>
            <Show when={editable()} fallback={<p class="plugin-note">{props.plugin.title} cannot print and save its settings, so they cannot be edited here.</p>}>
              <Show when={loadError()}>
                {(error) => (
                  <div role="alert" class="plugin-note">
                    <p>{props.plugin.title} could not print its settings: {error()}</p>
                    <Button onClick={() => void load()}>Read settings again</Button>
                  </div>
                )}
              </Show>
              <Show when={saved()} fallback={<Show when={!loadError()}><p role="status" class="plugin-note">Reading {props.plugin.title}'s settings…</p></Show>}>
                <Show when={fields().length > 0} fallback={<p class="plugin-note">{props.plugin.title} describes no settings.</p>}>
                  <div class="settings-fields">
                    <For each={fields()}>{(field) => <FieldView field={field} value={value} onChange={change} disabled={saving()} />}</For>
                  </div>
                </Show>
              </Show>
            </Show>
          </section>
        </div>
      </div>
      <Show when={editable() && saved()}>
        <div class="plugin-save-bar">
          <div class="plugin-save-text">
            <Show
              when={saveError()}
              fallback={<p class="plugin-note" role="status">{savedNote() ? "Saved." : `${props.plugin.title} checks these settings and saves them in its own config.`}</p>}
            >
              {(error) => <p role="alert" class="inline-error plugin-refusal">{error()}</p>}
            </Show>
          </div>
          <Button disabled={!dirty() || saving()} onClick={() => { setDraft({}); setSaveError(null); }}>Revert</Button>
          <Button type="submit" variant="primary" disabled={!dirty() || saving()}>{saving() ? "Saving…" : "Save"}</Button>
        </div>
      </Show>
    </form>
  );
}

/** One field, or a group of fields as a fieldset with its title as the legend. */
function FieldView(props: { field: SettingField; value: (field: LeafField) => FormValue; onChange(field: LeafField, value: FormValue): void; disabled: boolean }) {
  const field = untrack(() => props.field);
  const hint = field.description ?? undefined;
  switch (field.kind) {
    case "group":
      return (
        <fieldset class="settings-group">
          <legend>{field.title}</legend>
          <Show when={hint}><p class="plugin-note">{hint}</p></Show>
          <For each={field.fields}>{(child) => <FieldView field={child} value={props.value} onChange={props.onChange} disabled={props.disabled} />}</For>
        </fieldset>
      );
    case "integer":
    case "number":
      return (
        <NumberField
          label={field.title}
          unit={field.unit ?? undefined}
          hint={hint}
          min={field.minimum ?? undefined}
          step={field.kind === "integer" ? 1 : "any"}
          value={props.value(field) as string}
          disabled={props.disabled}
          onInput={(event) => props.onChange(field, event.currentTarget.value)}
        />
      );
    case "text":
      return <TextField label={field.title} hint={hint} value={props.value(field) as string} disabled={props.disabled} onInput={(event) => props.onChange(field, event.currentTarget.value)} />;
    case "choice": {
      const options = () => [
        // An unset choice shows as unset rather than as the first option, which would read as chosen.
        ...(props.value(field) === "" ? [{ value: "", label: "Not set" }] : []),
        ...field.options.map((option) => ({ value: option, label: optionLabel(option) })),
      ];
      return <Select label={field.title} hint={hint} options={options()} value={props.value(field) as string} disabled={props.disabled} onChange={(event) => props.onChange(field, event.currentTarget.value)} />;
    }
    case "choices":
      return (
        <fieldset class="settings-choices">
          <legend>{field.title}</legend>
          <div class="settings-choice-row">
            <For each={field.options}>
              {(option) => (
                <Checkbox
                  label={optionLabel(option)}
                  checked={(props.value(field) as string[]).includes(option)}
                  disabled={props.disabled}
                  onChange={(event) => {
                    const chosen = props.value(field) as string[];
                    props.onChange(field, event.currentTarget.checked ? [...chosen, option] : chosen.filter((item) => item !== option));
                  }}
                />
              )}
            </For>
          </div>
          <Show when={hint}><p class="plugin-note">{hint}</p></Show>
        </fieldset>
      );
    case "list": {
      const id = createUniqueId();
      return (
        <div class="settings-list">
          <label class="settings-label" for={id}>{field.title}</label>
          <Textarea
            id={id}
            label={field.title}
            rows={3}
            grow
            value={props.value(field) as string}
            disabled={props.disabled}
            aria-describedby={`${id}-hint`}
            onInput={(event) => props.onChange(field, event.currentTarget.value)}
          />
          <p id={`${id}-hint`} class="plugin-note">One per line.{hint ? ` ${hint}` : ""}</p>
        </div>
      );
    }
    case "switch":
      return (
        <div class="settings-switch">
          <Checkbox label={field.title} checked={props.value(field) === true} disabled={props.disabled} onChange={(event) => props.onChange(field, event.currentTarget.checked)} />
          <Show when={hint}><p class="plugin-note">{hint}</p></Show>
        </div>
      );
  }
}

/** Who runs the plugin's start command, and how its last run ended. */
function ProcessState(props: { plugin: PluginSnapshot; status: PluginStatusRead | undefined }) {
  const row = () => describeAgent(props.plugin, props.status);
  const process = () => props.plugin.process;
  const exit = () => {
    const current = process();
    return current.kind === "notRunning" ? current.exit : null;
  };
  const sentence = () => {
    const current = process();
    if (current.kind === "started") return `Prompting Time started it (process ${current.pid}) and stops it when Prompting Time quits.`;
    if (current.kind === "runningElsewhere") return `A copy started elsewhere, such as in a terminal, runs it (process ${current.pid}); Prompting Time leaves that copy running.`;
    if (props.plugin.settingsError || missingPrograms(props.plugin).length > 0) return "Prompting Time starts it when it opens once nothing below is missing.";
    return "Prompting Time starts it when it opens.";
  };
  return (
    <section class="plugin-section" aria-labelledby="plugin-process-heading">
      <h3 id="plugin-process-heading" class="section-heading">Process</h3>
      <p class="plugin-state"><StatusDot status={row().status} /><span>{row().meta}</span></p>
      <p class="plugin-note">{sentence()}</p>
      <Show when={props.status?.error}>{(error) => <p class="plugin-note">Its status action failed: {error()}</p>}</Show>
      <Show when={exit()?.output}>{(output) => <pre class="plugin-log" aria-label="The end of its output">{output()}</pre>}</Show>
    </section>
  );
}

/** The programs the manifest lists, each found, missing, or not needed with these settings. */
function Programs(props: { plugin: PluginSnapshot }) {
  const missing = () => missingPrograms(props.plugin);
  return (
    <section class="plugin-section" aria-labelledby="plugin-programs-heading">
      <h3 id="plugin-programs-heading" class="section-heading">Programs</h3>
      <Show when={props.plugin.settingsError}>
        {(error) => <p role="alert" class="inline-error">Prompting Time could not read the settings that decide which programs are needed: {error()}</p>}
      </Show>
      <Show when={missing().length > 0}>
        <p role="alert" class="inline-error">Missing: {missing().join(", ")}. Install {missing().length === 1 ? "it" : "them"} on the PATH, then reopen Prompting Time to start {props.plugin.title}.</p>
      </Show>
      <ul class="plugin-programs">
        <For each={props.plugin.prerequisites}>
          {(program) => (
            <li data-state={program.path ? "found" : program.needed ? "missing" : "unneeded"}>
              <code>{program.command}</code>
              <span>{program.path ?? (program.needed ? `missing. ${program.purpose}` : "not needed with these settings")}</span>
            </li>
          )}
        </For>
      </ul>
    </section>
  );
}

/** The actions the plugin declares, and which of them wait for your confirmation. */
export function PluginInspector(props: { plugin: PluginSnapshot }) {
  return (
    <>
      <section class="inspector-section" aria-labelledby="plugin-actions-heading">
        <div class="inspector-section-heading"><h2 id="plugin-actions-heading">Actions</h2></div>
        <dl class="inspector-list">
          <For each={props.plugin.actions}>
            {(action) => (
              <div title={action.description}>
                <dt>{action.title}</dt>
                <dd>{action.confirm ? "Asks you first" : "Runs without asking"}</dd>
              </div>
            )}
          </For>
        </dl>
      </section>
      <section class="inspector-section" aria-labelledby="plugin-files-heading">
        <div class="inspector-section-heading"><h2 id="plugin-files-heading">Files</h2></div>
        <dl class="inspector-list">
          <div><dt>Plugin</dt><dd>{props.plugin.folder}</dd></div>
          <div><dt>State</dt><dd>{props.plugin.stateFolder}</dd></div>
        </dl>
      </section>
    </>
  );
}

export function sourceText(source: PluginSource): string {
  switch (source.kind) {
    case "release": return `release archive ${source.url}, SHA-256 ${source.sha256.slice(0, 12)}…`;
    case "archive": return `local archive ${source.path}, SHA-256 ${source.sha256.slice(0, 12)}…`;
    case "folder": return `folder ${source.path}, run in place`;
  }
}

function optionLabel(option: string): string {
  return option.charAt(0).toUpperCase() + option.slice(1);
}

function messageFor(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Prompting Time could not reach the plugin.";
}
