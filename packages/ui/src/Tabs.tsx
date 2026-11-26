import { createUniqueId, For, untrack } from "solid-js";
import type { JSX } from "@solidjs/web";

export interface Tab {
  value: string;
  label: string;
}

export interface TabsProps {
  /** The tab list's accessible name, such as the screen it switches. */
  label: string;
  tabs: readonly Tab[];
  /** The selected tab's value. */
  value: string;
  onChange: (value: string) => void;
  /** The selected tab's panel. */
  children: JSX.Element;
  class?: JSX.HTMLAttributes<HTMLDivElement>["class"];
}

/**
 * Tabs that switch one panel, following WAI-ARIA's tabs pattern with automatic activation: Tab
 * reaches only the selected tab, the arrow keys, Home, and End select and focus another, and the
 * panel is labelled by its tab. The caller owns the selection and draws the selected panel.
 */
export function Tabs(props: TabsProps) {
  const id = createUniqueId();
  const tabId = (value: string) => `${id}-tab-${value}`;
  const buttons = new Map<string, HTMLButtonElement>();

  function onKeyDown(event: KeyboardEvent) {
    const index = props.tabs.findIndex((tab) => tab.value === props.value);
    const last = props.tabs.length - 1;
    const next = { ArrowRight: index + 1 > last ? 0 : index + 1, ArrowLeft: index - 1 < 0 ? last : index - 1, Home: 0, End: last }[event.key];
    const tab = next === undefined ? undefined : props.tabs[next];
    if (!tab) return;
    event.preventDefault();
    props.onChange(tab.value);
    buttons.get(tab.value)?.focus();
  }

  return (
    <div class={["pt-tabs", props.class]}>
      <div role="tablist" aria-label={props.label} class="pt-tablist" onKeyDown={onKeyDown}>
        <For each={props.tabs} keyed={(tab) => tab.value}>
          {(tab) => {
            // Keyed by value, so a row's value never changes.
            const value = untrack(tab).value;
            return (
              <button
                ref={(element: HTMLButtonElement) => buttons.set(value, element)}
                type="button"
                role="tab"
                id={tabId(value)}
                class="pt-tab"
                aria-selected={props.value === value ? "true" : "false"}
                aria-controls={`${id}-panel`}
                tabindex={props.value === value ? 0 : -1}
                onClick={() => props.onChange(value)}
              >
                {tab().label}
              </button>
            );
          }}
        </For>
      </div>
      <div role="tabpanel" id={`${id}-panel`} aria-labelledby={tabId(props.value)} class="pt-tabpanel">
        {props.children}
      </div>
    </div>
  );
}
