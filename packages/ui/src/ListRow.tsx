import { createUniqueId, Show } from "solid-js";
import type { JSX } from "@solidjs/web";
import { StatusDot, type Status } from "./StatusDot";

export interface ListRowProps {
  /** What the row is, such as a pull request or a chat, as an SVG icon. */
  icon: JSX.Element;
  title: string;
  /** One gray line under the title that says the rest, such as "acme/widgets#101 · approve once CI passes". */
  meta?: string;
  /** A dot before the meta line. */
  status?: Status;
  /** A short count after the title, such as 2 comments or "4 queued". */
  count?: number | string;
  /** Marks the row whose item the page shows (aria-current). */
  selected?: boolean;
  onClick?: (event: MouseEvent) => void;
}

/** One row of a list such as All work: icon, title, count, and a gray meta line with a status dot. */
export function ListRow(props: ListRowProps) {
  const id = createUniqueId();
  // aria-labelledby joins the parts with spaces; a name taken from the content would run "2" and
  // the meta line together wherever the browser does not treat the parts as blocks.
  const labelledBy = () =>
    [`${id}-title`, props.count !== undefined && `${id}-count`, props.meta && `${id}-meta`].filter(Boolean).join(" ");
  return (
    <button
      type="button"
      class="pt-list-row"
      aria-current={props.selected ? "true" : undefined}
      aria-labelledby={labelledBy()}
      onClick={(event) => props.onClick?.(event)}
    >
      <span class="pt-list-row-icon" aria-hidden="true">
        {props.icon}
      </span>
      <span id={`${id}-title`} class="pt-list-row-title">
        {props.title}
      </span>
      <Show when={props.count !== undefined}>
        <span id={`${id}-count`} class="pt-list-row-count">
          {props.count}
        </span>
      </Show>
      <Show when={props.meta}>
        <span class="pt-list-row-meta">
          <Show when={props.status}>{(status) => <StatusDot status={status()} />}</Show>
          <span id={`${id}-meta`} class="pt-list-row-meta-text">
            {props.meta}
          </span>
        </span>
      </Show>
    </button>
  );
}
