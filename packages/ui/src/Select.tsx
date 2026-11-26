import { createUniqueId, For, omit } from "solid-js";
import type { JSX } from "@solidjs/web";
import { describedBy, Field, type FieldProps } from "./Field";

export interface SelectOption {
  value: string;
  label: string;
}

export interface SelectProps extends FieldProps, Omit<JSX.SelectHTMLAttributes<HTMLSelectElement>, "id" | "children"> {
  options: readonly SelectOption[];
}

/** A native select with its label. The open list is the operating system's own. */
export function Select(props: SelectProps) {
  const id = createUniqueId();
  const rest = omit(props, "label", "hint", "error", "options", "class");
  return (
    <Field id={id} label={props.label} hint={props.hint} error={props.error} class={props.class}>
      <span class="pt-select">
        <select
          {...rest}
          id={id}
          class="pt-select-input"
          aria-invalid={props.error ? "true" : undefined}
          aria-describedby={describedBy(id, props)}
        >
          <For each={props.options}>{(option) => <option value={option.value}>{option.label}</option>}</For>
        </select>
      </span>
    </Field>
  );
}
