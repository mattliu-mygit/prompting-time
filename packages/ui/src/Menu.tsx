import * as menu from "@zag-js/menu";
import { createUniqueId, For } from "solid-js";
import { Portal, type JSX } from "@solidjs/web";
import { IconButton } from "./Button";
import { spread, useMachine } from "./zag";

export interface MenuItem {
  value: string;
  label: string;
  disabled?: boolean;
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
 * An icon button that opens a list of actions, with Zag.js's menu behavior: arrow keys and
 * typeahead move between items, Enter or a click runs one, and Escape or an outside click closes
 * the list and returns focus to the button.
 */
export function Menu(props: MenuProps) {
  const id = createUniqueId();
  const api = useMachine(menu.machine, () => ({ id, onSelect: (details) => props.onSelect(details.value) }), menu.connect);
  return (
    <>
      <IconButton ref={spread(() => api().getTriggerProps())} label={props.label}>
        {props.icon}
      </IconButton>
      <Portal>
        <div ref={spread(() => api().getPositionerProps())} class="pt-menu-positioner">
          <ul ref={spread(() => api().getContentProps())} class="pt-menu">
            <For each={props.items}>
              {(item) => (
                <li ref={spread(() => api().getItemProps({ value: item.value, disabled: item.disabled }))} class="pt-menu-item">
                  {item.label}
                </li>
              )}
            </For>
          </ul>
        </div>
      </Portal>
    </>
  );
}
