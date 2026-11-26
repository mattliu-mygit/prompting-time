/**
 * What a status dot shows. Each color means one thing wherever it appears, and the words beside the
 * dot always say it: "error" is red, for blocked or failed; "attention" is orange, for waiting on
 * you; "success" is green, for ready to approve. "running" is a dot with no color, for work under
 * way, and "idle" draws no dot but keeps its place, for every other status.
 */
export type Status = "idle" | "running" | "attention" | "success" | "error";

export interface StatusDotProps {
  status: Status;
  /**
   * Words for the status, such as "Failed". Leave it out when nearby text already says the status;
   * the dot is then hidden from assistive technology.
   */
  label?: string;
}

/** A small dot before a status's words, instead of a colored tag. */
export function StatusDot(props: StatusDotProps) {
  return (
    <span
      class="pt-status-dot"
      data-status={props.status}
      role={props.label ? "img" : undefined}
      aria-label={props.label}
      aria-hidden={props.label ? undefined : "true"}
    />
  );
}
