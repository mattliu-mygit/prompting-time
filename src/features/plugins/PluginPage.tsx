import { createEffect, createSignal, onSettled, Show, untrack } from "solid-js";
import type { PluginActions, PluginFilesChange } from "../../app/store";
import type { PluginActionResult, PluginSnapshot } from "../../bridge/types";
import { ConfirmActionDialog } from "../agents/ConfirmActionDialog";
import { createPageHost } from "./pageHost";

type Props = {
  plugin: PluginSnapshot;
  /**
   * The route the user opened, from an item's or blocker's row, Command-J, or the plugin's name
   * (`/`), or the next item Prompting Time opened when the last one left Needs you. Each new object
   * sends `open` again, so opening the same row twice shows it again after the page moved on.
   */
  target: { route: string } | null;
  actions: PluginActions;
  onPluginFilesChange(listener: (change: PluginFilesChange) => void): () => void;
};

type Confirmation = { action: string; args: readonly string[]; resolve(result: PluginActionResult | null): void };

/**
 * A plugin's page of page protocol 2, in a frame sandboxed to scripts only: the page gets an
 * origin of its own ("null"), so it cannot reach this window, Tauri, or its own files' origin, and
 * talks to Prompting Time only through postMessage. The page runs its plugin's declared actions
 * through the page host; an action the manifest marks confirm opens the confirmation first. The
 * caller makes one per plugin and keeps it while the user opens the plugin's rows.
 */
export function PluginPage(props: Props) {
  const name = untrack(() => props.plugin.name);
  let frame: HTMLIFrameElement | undefined;
  const [confirmation, setConfirmation] = createSignal<Confirmation | null>(null);
  const host = createPageHost({
    // The host reads the latest plugin each time the page calls; each read is a snapshot.
    plugin: () => untrack(() => props.plugin),
    actions: props.actions,
    // The sandboxed page's origin is opaque, so no origin names it; the message goes to this frame's window only.
    post: (message) => frame?.contentWindow?.postMessage(message, "*"),
    confirm: (action, args) => new Promise((resolve, reject) => {
      if (untrack(confirmation)) {
        reject(new Error("Another action is waiting for your confirmation."));
        return;
      }
      setConfirmation({ action, args, resolve });
    }),
  });

  onSettled(() => {
    const onMessage = (event: MessageEvent) => {
      if (frame?.contentWindow && event.source === frame.contentWindow) host.receive(event.data);
    };
    window.addEventListener("message", onMessage);
    const stopListening = props.onPluginFilesChange((change) => {
      if (change.plugin === name) host.filesChanged(change.files);
    });
    return () => {
      window.removeEventListener("message", onMessage);
      stopListening();
      host.dispose();
      untrack(confirmation)?.resolve(null);
    };
  });

  createEffect(() => props.target, (target) => host.open(target?.route ?? null));

  return (
    <div class="plugin-page">
      <iframe
        ref={(element: HTMLIFrameElement) => {
          frame = element;
        }}
        class="plugin-frame"
        title={`${props.plugin.title}'s page`}
        sandbox="allow-scripts"
        src={props.plugin.pageUrl}
      />
      <Show when={confirmation()} keyed>
        {(current: Confirmation) => (
          <ConfirmActionDialog
            plugin={props.plugin}
            action={current.action}
            args={current.args}
            actions={props.actions}
            onClose={(result) => {
              setConfirmation(null);
              current.resolve(result);
            }}
          />
        )}
      </Show>
    </div>
  );
}
