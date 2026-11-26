import * as dialog from "@zag-js/dialog";
import { createUniqueId } from "solid-js";
import { Portal, type JSX } from "@solidjs/web";
import { spread, useMachine } from "./zag";

export interface ConfirmDialogProps {
  open: boolean;
  title: string;
  /** What confirming will do. */
  children: JSX.Element;
  confirmLabel: string;
  cancelLabel?: string;
  onConfirm: () => void;
  /** Called for Cancel and Escape. */
  onCancel: () => void;
}

/**
 * A modal confirmation, with Zag.js's alert dialog behavior: focus starts on Cancel and stays
 * inside, Escape cancels, outside clicks do nothing, and focus returns to where it was.
 */
export function ConfirmDialog(props: ConfirmDialogProps) {
  const id = createUniqueId();
  const api = useMachine(
    dialog.machine,
    () => ({
      id,
      role: "alertdialog" as const,
      open: props.open,
      onOpenChange: (details: dialog.OpenChangeDetails) => {
        if (!details.open) props.onCancel();
      },
    }),
    dialog.connect,
  );
  return (
    <Portal>
      <div ref={spread(() => api().getBackdropProps())} class="pt-dialog-backdrop" />
      <div ref={spread(() => api().getPositionerProps())} class="pt-dialog-positioner">
        <section ref={spread(() => api().getContentProps())} class="pt-dialog">
          <h2 ref={spread(() => api().getTitleProps())} class="pt-dialog-title">
            {props.title}
          </h2>
          <div ref={spread(() => api().getDescriptionProps())} class="pt-dialog-description">
            {props.children}
          </div>
          <div class="pt-dialog-actions">
            <button ref={spread(() => api().getCloseTriggerProps())} class="pt-button">
              {props.cancelLabel ?? "Cancel"}
            </button>
            <button type="button" class="pt-button" data-variant="primary" onClick={() => props.onConfirm()}>
              {props.confirmLabel}
            </button>
          </div>
        </section>
      </div>
    </Portal>
  );
}
