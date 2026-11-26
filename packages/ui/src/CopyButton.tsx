import { createSignal, Show } from "solid-js";
import { CopyIcon } from "./icons";

export interface CopyButtonProps {
  /** The exact text to copy. */
  content: string;
  /** The button's text, such as "Copy code", or its name and tooltip when `iconOnly`. */
  label: string;
  /** Show only the copy icon. */
  iconOnly?: boolean;
}

/**
 * Copies text to the clipboard and says whether that worked, in a status next to the button. A
 * result only shows for the text that was copied, so a copy started before the text changed, as in
 * a streaming reply, never reports success for the new text.
 */
export function CopyButton(props: CopyButtonProps) {
  const [result, setResult] = createSignal<{ content: string; copied: boolean } | null>(null);
  let attempts = 0;
  async function copy() {
    const attempt = ++attempts;
    const content = props.content;
    let copied = true;
    try {
      await navigator.clipboard.writeText(content);
    } catch {
      copied = false;
    }
    if (attempt === attempts) setResult({ content, copied });
  }
  const shown = () => {
    const current = result();
    return current && current.content === props.content ? current : null;
  };
  return (
    <span class="pt-copy">
      <button
        type="button"
        class={props.iconOnly ? "pt-icon-button" : "pt-button"}
        aria-label={props.iconOnly ? props.label : undefined}
        title={props.iconOnly ? props.label : undefined}
        onClick={() => void copy()}
      >
        {props.iconOnly ? <CopyIcon /> : props.label}
      </button>
      <Show when={shown()}>{(current) => <span role="status">{current().copied ? "Copied" : "Could not copy. Try again."}</span>}</Show>
    </span>
  );
}
