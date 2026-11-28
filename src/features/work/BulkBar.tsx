import { For } from "solid-js";
import { Button, CloseIcon, IconButton } from "@prompting-time/ui";

type Props = {
  /** How many Needs you entries are checked. */
  count: number;
  /** The row actions every checked entry offers, by name with the manifest's title; Hide applies to all. */
  actions: readonly { name: string; title: string }[];
  /** What a bulk action running now says, such as "Rerun · 2 of 3"; the buttons wait until it ends. */
  progress: string | null;
  onHide(): void;
  onAction(name: string): void;
  onClear(): void;
};

/**
 * The bar at the bottom of the sidebar while Needs you entries are checked: how many, Hide, the
 * row actions every checked entry offers, and × (or Escape) to uncheck them all.
 */
export function BulkBar(props: Props) {
  return (
    <div
      class="work-bulk"
      role="group"
      aria-label="Checked in Needs you"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || props.progress) return;
        event.preventDefault();
        props.onClear();
      }}
    >
      <span class="work-bulk-count" role="status">{props.progress ?? `${props.count} selected`}</span>
      <div class="work-bulk-actions">
        <Button disabled={props.progress !== null} onClick={() => props.onHide()}>Hide</Button>
        <For each={props.actions} keyed={(action) => action.name}>
          {(action) => <Button disabled={props.progress !== null} onClick={() => props.onAction(action().name)}>{action().title}</Button>}
        </For>
        <IconButton label="Clear selection" disabled={props.progress !== null} onClick={() => props.onClear()}><CloseIcon /></IconButton>
      </div>
    </div>
  );
}
