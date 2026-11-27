import { For } from "solid-js";
import { ArrowLeftIcon, Button } from "@prompting-time/ui";
import { REVIEW_VIEWS, viewName, type FullViewId } from "./views";

const typing = (event: KeyboardEvent) => event.target instanceof HTMLElement && (event.target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName));

/**
 * j, k, and Escape on the window while a view shows in full in the walkthrough's place: after the
 * app's own Escape, which closes All work's list first, and not while typing, with a modifier, or
 * while a dialog or menu is open. Returns what stops listening.
 */
export function listenToViewKeys(keys: { step(by: 1 | -1): void; escape(): void }): () => void {
  const listener = (event: KeyboardEvent) => {
    if (event.defaultPrevented || typing(event) || event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.key !== "j" && event.key !== "k" && event.key !== "Escape") return;
    if ([...document.querySelectorAll('[role="dialog"], [role="alertdialog"], [role="menu"]:not([hidden])')].some((element) => element.checkVisibility?.())) return;
    event.preventDefault();
    if (event.key === "Escape") keys.escape();
    else keys.step(event.key === "j" ? 1 : -1);
  };
  window.addEventListener("keydown", listener);
  return () => window.removeEventListener("keydown", listener);
}

/**
 * The head of a view in full: the way back to the walkthrough, the view's name and what it shows,
 * and what j and k step through. The name takes the focus as the view opens, so a screen reader
 * says where it landed.
 */
export function ViewHead(props: { id: FullViewId; detail?: string; steps: string; headingRef(element: HTMLHeadingElement): void; onBack(): void }) {
  return (
    <header class="view-head">
      <Button onClick={() => props.onBack()}><ArrowLeftIcon />Back to the walkthrough</Button>
      <div class="view-head-text">
        <h3 id={`${props.id}-heading`} tabindex="-1" ref={props.headingRef}>{viewName(props.id)}</h3>
        <p>{REVIEW_VIEWS.find((view) => view.id === props.id)!.description}{props.detail ? ` ${props.detail}` : ""}</p>
      </div>
      <p class="view-keys"><kbd>j</kbd> <kbd>k</kbd> {props.steps} · <kbd>Esc</kbd> goes back</p>
    </header>
  );
}

/** What a view's data may miss, in the plugin's words. */
export function ViewLimits(props: { id: FullViewId; heading: string; limits: readonly string[] }) {
  return (
    <section class="view-limits" aria-labelledby={`${props.id}-limits`}>
      <h4 id={`${props.id}-limits`}>{props.heading}</h4>
      <ul>
        <For each={props.limits}>{(limit) => <li>{limit}</li>}</For>
      </ul>
    </section>
  );
}
