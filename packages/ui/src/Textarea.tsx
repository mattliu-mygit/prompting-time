import { createEffect, omit, onSettled } from "solid-js";
import type { JSX } from "@solidjs/web";

export interface TextareaProps extends Omit<JSX.TextareaHTMLAttributes<HTMLTextAreaElement>, "children"> {
  /** The field's accessible name, such as "Message". Show a visible label next to it if the page needs one. */
  label: string;
  /**
   * Grow with the text, from `rows` lines up to the field's maximum height (30% of the window
   * unless the page sets `max-block-size`), then scroll inside.
   */
  grow?: boolean;
}

/** A native multi-line text input in Sigil's input style. */
export function Textarea(props: TextareaProps) {
  const rest = omit(props, "label", "grow", "class", "ref");
  let field: HTMLTextAreaElement | undefined;

  // The height that fits the text: the content height plus the borders, since the field's box
  // includes them. Shrinking needs "auto" first, or scrollHeight stays at the old height.
  function fit() {
    if (!field || !props.grow) return;
    field.style.height = "auto";
    const style = window.getComputedStyle(field);
    const borders = parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth);
    field.style.height = `${field.scrollHeight + borders}px`;
  }

  // Fit again when the text changes, from typing or from the caller's value.
  createEffect(
    () => props.value,
    () => fit(),
  );

  // A narrower field wraps more lines. Fit in the next frame, not inside the observer's callback,
  // so the height change cannot trigger the observer again in the same frame.
  onSettled(() => {
    if (!field || !props.grow || typeof ResizeObserver === "undefined") return;
    let width = field.clientWidth;
    let frame: number | null = null;
    const observer = new ResizeObserver(() => {
      if (!field || field.clientWidth === width) return;
      width = field.clientWidth;
      if (frame !== null) return;
      frame = window.requestAnimationFrame(() => {
        frame = null;
        fit();
      });
    });
    observer.observe(field);
    return () => {
      observer.disconnect();
      if (frame !== null) window.cancelAnimationFrame(frame);
    };
  });

  return (
    <textarea
      {...rest}
      ref={(element: HTMLTextAreaElement) => {
        field = element;
        (props.ref as ((element: HTMLTextAreaElement) => void) | undefined)?.(element);
      }}
      class={["pt-textarea", props.class]}
      data-grow={props.grow ? "" : undefined}
      aria-label={props.label}
      onInput={(event) => {
        fit();
        const handler = props.onInput;
        if (typeof handler === "function") handler(event);
      }}
    />
  );
}
