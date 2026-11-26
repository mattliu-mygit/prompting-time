import { omit } from "solid-js";
import type { JSX } from "@solidjs/web";

export interface ButtonProps extends JSX.ButtonHTMLAttributes<HTMLButtonElement> {
  /** "primary" for the main action on a screen, such as Submit review; "plain" (the default) for the rest. */
  variant?: "primary" | "plain";
}

/** A text button. It is type="button" unless told otherwise, so it never submits a form by accident. */
export function Button(props: ButtonProps) {
  const rest = omit(props, "variant", "class", "children");
  return (
    <button type="button" {...rest} class={["pt-button", props.class]} data-variant={props.variant ?? "plain"}>
      {props.children}
    </button>
  );
}

export interface IconButtonProps extends JSX.ButtonHTMLAttributes<HTMLButtonElement> {
  /** What the button does, such as "Search". It names the button and shows as its tooltip. */
  label: string;
  /** The icon, usually an SVG. */
  children: JSX.Element;
}

/** A square button that shows only an icon. */
export function IconButton(props: IconButtonProps) {
  const rest = omit(props, "label", "class", "children");
  return (
    <button type="button" {...rest} class={["pt-icon-button", props.class]} aria-label={props.label} title={props.label}>
      {props.children}
    </button>
  );
}
