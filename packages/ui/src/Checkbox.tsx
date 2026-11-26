import { omit } from "solid-js";
import type { JSX } from "@solidjs/web";

export interface CheckboxProps extends Omit<JSX.InputHTMLAttributes<HTMLInputElement>, "type"> {
  label: JSX.Element;
}

/** A native checkbox with its label. Pass `checked` and `onChange` to control it. */
export function Checkbox(props: CheckboxProps) {
  const rest = omit(props, "label", "class");
  return (
    <label class={["pt-checkbox", props.class]}>
      <input {...rest} type="checkbox" />
      <span>{props.label}</span>
    </label>
  );
}
