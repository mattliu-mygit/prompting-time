import { createUniqueId, omit, Show } from "solid-js";
import type { JSX } from "@solidjs/web";

export interface FieldProps {
  label: string;
  /** Gray text under the control, such as "5 to 120". */
  hint?: string;
  /** Text in the error color under the control. It also marks the control invalid. */
  error?: string;
}

/** The label, control, hint, and error that TextField, NumberField, and Select share. */
export function Field(props: FieldProps & { id: string; class?: JSX.HTMLAttributes<HTMLDivElement>["class"]; children: JSX.Element }) {
  return (
    <div class={["pt-field", props.class]}>
      <label class="pt-field-label" for={props.id}>
        {props.label}
      </label>
      <div class="pt-field-control">{props.children}</div>
      <Show when={props.hint}>
        <p id={`${props.id}-hint`} class="pt-field-hint">
          {props.hint}
        </p>
      </Show>
      <Show when={props.error}>
        <p id={`${props.id}-error`} class="pt-field-error">
          {props.error}
        </p>
      </Show>
    </div>
  );
}

/** The ids of the hint and error paragraphs that describe a field's control, if it has either. */
export function describedBy(id: string, props: FieldProps): string | undefined {
  const ids = [props.hint && `${id}-hint`, props.error && `${id}-error`].filter(Boolean);
  return ids.length > 0 ? ids.join(" ") : undefined;
}

export interface TextFieldProps extends FieldProps, Omit<JSX.InputHTMLAttributes<HTMLInputElement>, "type" | "id"> {}

/** A one-line text input with its label. */
export function TextField(props: TextFieldProps) {
  const id = createUniqueId();
  const rest = omit(props, "label", "hint", "error", "class");
  return (
    <Field id={id} label={props.label} hint={props.hint} error={props.error} class={props.class}>
      <input
        {...rest}
        type="text"
        id={id}
        class="pt-input"
        aria-invalid={props.error ? "true" : undefined}
        aria-describedby={describedBy(id, props)}
      />
    </Field>
  );
}

export interface NumberFieldProps extends FieldProps, Omit<JSX.InputHTMLAttributes<HTMLInputElement>, "type" | "id"> {
  /** The unit after the input, such as "minutes". */
  unit?: string;
}

/** A native number input with its label and an optional unit. `min`, `max`, and `step` work as on any number input. */
export function NumberField(props: NumberFieldProps) {
  const id = createUniqueId();
  const rest = omit(props, "label", "hint", "error", "unit", "class");
  return (
    <Field id={id} label={props.label} hint={props.hint} error={props.error} class={props.class}>
      <input
        {...rest}
        type="number"
        id={id}
        class="pt-input"
        aria-invalid={props.error ? "true" : undefined}
        aria-describedby={describedBy(id, props)}
      />
      <Show when={props.unit}>
        <span class="pt-field-unit">{props.unit}</span>
      </Show>
    </Field>
  );
}
