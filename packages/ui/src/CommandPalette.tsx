import * as dialog from "@zag-js/dialog";
import { createEffect, createMemo, createSignal, createUniqueId, For, Show, untrack } from "solid-js";
import { Portal } from "@solidjs/web";
import { spread, useMachine } from "./zag";

export interface PaletteItem {
  id: string;
  label: string;
  /** Gray text after the label, such as a project path or a parent's title. */
  detail?: string;
  /** More words the search matches, such as a parent's title. */
  keywords?: readonly string[];
  disabled?: boolean;
  /** Marks the item the page shows now. */
  current?: boolean;
}

export interface PaletteGroup {
  heading: string;
  items: readonly PaletteItem[];
}

export interface CommandPaletteProps {
  open: boolean;
  /** The dialog's and the search field's name, such as "Search conversations and commands". */
  label: string;
  placeholder?: string;
  groups: readonly PaletteGroup[];
  /** Shown when nothing matches the search. */
  emptyText?: string;
  /** Called for Escape, outside clicks, and a chosen item; the caller sets `open` to false. */
  onClose: () => void;
  /**
   * Called with the chosen item's id once the palette has closed and given focus back to where it
   * was, so an item may move focus itself, for example into a dialog it opens.
   */
  onSelect: (id: string) => void;
}

/**
 * A search dialog over groups of items, such as conversations and commands. Every word typed must
 * appear in an item's label, detail, or keywords. Arrow keys move between the items that are not
 * disabled, Enter or a click chooses one, and Escape closes the palette. Zag.js's dialog keeps focus
 * inside while it is open.
 */
export function CommandPalette(props: CommandPaletteProps) {
  const id = createUniqueId();
  let input: HTMLInputElement | undefined;
  let returnFocus: HTMLElement | null = null;
  let chosen: string | null = null;
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal<string | null>(null);
  const api = useMachine(
    dialog.machine,
    () => ({
      id,
      open: props.open,
      // The palette gives focus back itself, before running the chosen item.
      restoreFocus: false,
      initialFocusEl: () => input ?? null,
      onOpenChange: (details: dialog.OpenChangeDetails) => {
        if (!details.open) props.onClose();
      },
    }),
    dialog.connect,
  );

  const visible = createMemo(() => {
    const words = query().toLowerCase().split(/\s+/).filter(Boolean);
    return props.groups
      .map((group) => ({
        heading: group.heading,
        items: group.items.filter((item) => {
          const text = [item.label, item.detail ?? "", ...(item.keywords ?? [])].join(" ").toLowerCase();
          return words.every((word) => text.includes(word));
        }),
      }))
      .filter((group) => group.items.length > 0);
  });
  const enabled = createMemo(() => visible().flatMap((group) => group.items.filter((item) => !item.disabled)));
  // The highlighted item, or the first enabled one when the search changed what is shown.
  const activeId = () => (enabled().some((item) => item.id === active()) ? active() : (enabled()[0]?.id ?? null));
  const optionId = (itemId: string) => `${id}-option-${itemId}`;

  const open = createMemo(() => props.open);
  createEffect(
    open,
    (open, wasOpen) => {
      if (open) {
        returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        chosen = null;
        setQuery("");
        setActive(null);
        queueMicrotask(() => input?.focus());
      } else if (wasOpen) {
        const item = chosen;
        chosen = null;
        returnFocus?.focus();
        returnFocus = null;
        // The caller's handler reads its own state as it is now, as a click handler would.
        if (item !== null) untrack(() => props.onSelect(item));
      }
    },
  );

  function choose(item: PaletteItem) {
    if (item.disabled) return;
    chosen = item.id;
    props.onClose();
  }

  function move(step: 1 | -1) {
    const items = enabled();
    const index = items.findIndex((item) => item.id === activeId());
    const next = items[Math.min(Math.max(index + step, 0), items.length - 1)];
    if (!next) return;
    setActive(next.id);
    document.getElementById(optionId(next.id))?.scrollIntoView?.({ block: "nearest" });
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      move(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      const item = enabled().find((candidate) => candidate.id === activeId());
      if (item) choose(item);
    }
  }

  return (
    <Portal>
      <div ref={spread(() => api().getBackdropProps())} class="pt-dialog-backdrop" />
      <div ref={spread(() => api().getPositionerProps())} class="pt-palette-positioner">
        <section ref={spread(() => api().getContentProps())} class="pt-palette">
          <h2 ref={spread(() => api().getTitleProps())} class="pt-visually-hidden">
            {props.label}
          </h2>
          <Show when={props.open}>
            <input
              ref={input}
              class="pt-palette-input"
              role="combobox"
              aria-label={props.label}
              aria-expanded="true"
              aria-controls={`${id}-list`}
              aria-autocomplete="list"
              aria-activedescendant={activeId() ? optionId(activeId()!) : undefined}
              placeholder={props.placeholder}
              value={query()}
              onInput={(event) => setQuery(event.currentTarget.value)}
              onKeyDown={onKeyDown}
            />
            <div id={`${id}-list`} class="pt-palette-list" role="listbox" aria-label={props.label}>
              <Show when={visible().length > 0} fallback={<p class="pt-palette-empty">{props.emptyText ?? "Nothing found."}</p>}>
                <For each={visible()}>
                  {(group, groupIndex) => (
                    <div class="pt-palette-group" role="group" aria-labelledby={`${id}-group-${groupIndex()}`}>
                      <div id={`${id}-group-${groupIndex()}`} class="pt-palette-heading" role="presentation">
                        {group.heading}
                      </div>
                      <For each={group.items}>
                        {(item) => (
                          <div
                            id={optionId(item.id)}
                            class="pt-palette-option"
                            role="option"
                            aria-selected={activeId() === item.id ? "true" : "false"}
                            aria-disabled={item.disabled ? "true" : undefined}
                            onPointerMove={() => {
                              if (!item.disabled) setActive(item.id);
                            }}
                            onClick={() => choose(item)}
                          >
                            <span class="pt-palette-label">
                              <span>{item.label}</span>
                              <Show when={item.detail}>
                                <small>{item.detail}</small>
                              </Show>
                            </span>
                            <Show when={item.current}>
                              <span class="pt-palette-current">Current</span>
                            </Show>
                          </div>
                        )}
                      </For>
                    </div>
                  )}
                </For>
              </Show>
            </div>
          </Show>
        </section>
      </div>
    </Portal>
  );
}
