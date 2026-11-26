import * as menu from "@zag-js/menu";
import { createUniqueId, For, Show } from "solid-js";
import { Portal, type JSX } from "@solidjs/web";
import { IconButton } from "./Button";
import { CheckIcon } from "./icons";
import { spread, useMachine } from "./zag";

export interface MenuItem {
  value: string;
  label: string;
  disabled?: boolean;
  /**
   * Makes the item a choice that shows whether it is on: "radio" for one choice among several, such
   * as a status filter, or "checkbox" for an independent switch. Leave it out for an action.
   */
  type?: "radio" | "checkbox";
  /** Whether a radio or checkbox item is on. The menu shows it; `onSelect` reports a choice. */
  checked?: boolean;
}

export interface MenuProps {
  /** The trigger's accessible name and tooltip, such as "More actions". */
  label: string;
  /** The trigger's icon. */
  icon: JSX.Element;
  items: readonly MenuItem[];
  onSelect: (value: string) => void;
}

/**
 * An icon button that opens a list of actions or choices, with Zag.js's menu behavior: arrow keys
 * and typeahead move between items, Enter or a click runs one, and Escape or an outside click closes
 * the list and returns focus to the button. A radio or checkbox item is a menuitemradio or
 * menuitemcheckbox with aria-checked, and shows a check mark while it is on.
 */
export function Menu(props: MenuProps) {
  const id = createUniqueId();
  const api = useMachine(menu.machine, () => ({ id, onSelect: (details) => props.onSelect(details.value) }), menu.connect);
  const itemProps = (item: MenuItem) =>
    item.type
      ? api().getOptionItemProps({ type: item.type, value: item.value, checked: item.checked ?? false, disabled: item.disabled })
      : api().getItemProps({ value: item.value, disabled: item.disabled });
  return (
    <>
      <IconButton ref={spread(() => api().getTriggerProps())} label={props.label}>
        {props.icon}
      </IconButton>
      <Portal>
        <div ref={spread(() => api().getPositionerProps())} class="pt-menu-positioner">
          <ul ref={spread(() => api().getContentProps())} class="pt-menu">
            {/* Keyed by value, so a caller that rebuilds its items keeps the same rows. */}
            <For each={props.items} keyed={(item) => item.value}>
              {(item) => (
                <li ref={spread(() => itemProps(item()))} class="pt-menu-item">
                  <Show when={item().type}>
                    <span class="pt-menu-item-check" aria-hidden="true">
                      <Show when={item().checked}>
                        <CheckIcon />
                      </Show>
                    </span>
                  </Show>
                  {item().label}
                </li>
              )}
            </For>
          </ul>
        </div>
      </Portal>
    </>
  );
}
