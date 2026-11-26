import { createUniqueId, omit, Show } from "solid-js";
import { Dynamic, type JSX } from "@solidjs/web";
import { StatusDot, type Status } from "./StatusDot";

export interface ListRowProps extends Omit<JSX.HTMLAttributes<HTMLElement>, "title" | "onClick"> {
  /** What the row is, such as a pull request or a chat, as an SVG icon. */
  icon: JSX.Element;
  title: string;
  /** One gray line under the title that says the rest, such as "acme/widgets#101 · approve once CI passes". */
  meta?: string;
  /** A dot before the meta line. */
  status?: Status;
  /** A short count after the title, such as 2 comments or "4 queued". */
  count?: number | string;
  /** Marks the row whose item the page shows (aria-current on a button row). */
  selected?: boolean;
  /**
   * "button" (the default) for a row in a plain list. "li" for an item of a role="tree" list: the
   * caller passes role="treeitem", its aria attributes, tabindex, and key handling.
   */
  element?: "button" | "li";
  /** A control at the end of the title line, such as a button that shows a row's children. */
  trailing?: JSX.Element;
  /** Content under the meta line, across the row. */
  children?: JSX.Element;
  onClick?: (event: MouseEvent) => void;
}

/** One row of a list such as All work: icon, title, count, and a gray meta line with a status dot. */
export function ListRow(props: ListRowProps) {
  const id = createUniqueId();
  const rest = omit(props, "icon", "title", "meta", "status", "count", "selected", "element", "trailing", "children", "onClick", "class");
  // aria-labelledby joins the parts with spaces; a name taken from the content would run "2" and
  // the meta line together wherever the browser does not treat the parts as blocks.
  const labelledBy = () =>
    props["aria-label"]
      ? undefined
      : [`${id}-title`, props.count !== undefined && `${id}-count`, props.meta && `${id}-meta`].filter(Boolean).join(" ");
  const button = () => (props.element ?? "button") === "button";
  return (
    <Dynamic
      // A plain tag name, so one set of attributes types for both elements.
      component={(props.element ?? "button") as string}
      type={button() ? "button" : undefined}
      {...rest}
      class={["pt-list-row", props.class]}
      aria-current={button() && props.selected ? "true" : undefined}
      aria-labelledby={labelledBy()}
      onClick={(event: MouseEvent) => props.onClick?.(event)}
    >
      <span class="pt-list-row-icon" aria-hidden="true">
        {props.icon}
      </span>
      <span id={`${id}-title`} class="pt-list-row-title">
        {props.title}
      </span>
      <Show when={props.count !== undefined || props.trailing}>
        <span class="pt-list-row-end">
          <Show when={props.count !== undefined}>
            <span id={`${id}-count`} class="pt-list-row-count">
              {props.count}
            </span>
          </Show>
          {props.trailing}
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
      <Show when={props.children}>
        <div class="pt-list-row-children">{props.children}</div>
      </Show>
    </Dynamic>
  );
}
