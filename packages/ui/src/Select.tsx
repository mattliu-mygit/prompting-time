import { createEffect, createUniqueId, For, omit } from "solid-js";
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
  const rest = omit(props, "label", "hint", "error", "options", "class", "ref");
  let select: HTMLSelectElement | undefined;

  // New options replace the <option> elements, and a select whose chosen option is removed falls
  // back to its first, so choose the caller's value again once the new options are in: the same
  // options drawn again keep their choice, and a value given before its option exists shows
  // when the option arrives.
  createEffect(
    () => [props.options, props.value] as const,
    ([, value]) => {
      if (select && value !== undefined) select.value = String(value);
    },
  );

  return (
    <Field id={id} label={props.label} hint={props.hint} error={props.error} class={props.class}>
      <span class="pt-select">
        <select
          {...rest}
          ref={(element: HTMLSelectElement) => {
            select = element;
            (props.ref as ((element: HTMLSelectElement) => void) | undefined)?.(element);
          }}
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
