export type Status = "idle" | "running" | "attention" | "success" | "error";

export interface StatusDotProps {
  status: Status;
  /**
   * Words for the status, such as "Failed". Leave it out when nearby text already says the status;
   * the dot is then hidden from assistive technology.
   */
  label?: string;
}

/** A small colored dot that shows status without a colored tag. */
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
