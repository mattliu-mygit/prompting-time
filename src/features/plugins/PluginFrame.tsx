import { createEffect, createSignal, onSettled, Show, untrack } from "solid-js";
import type { PluginActions, PluginFilesChange, PluginStatusRead } from "../../app/store";
import type { PluginActionResult, PluginSnapshot } from "../../bridge/types";
import { ConfirmActionDialog } from "../agents/ConfirmActionDialog";
import { createPageBridge } from "./bridge";

type Props = {
  plugin: PluginSnapshot;
  status: PluginStatusRead | undefined;
  /** The review the user opened, which the page shows once it is ready; null for the page's own start. */
  runId: string | null;
  actions: PluginActions;
  onPluginFilesChange(listener: (change: PluginFilesChange) => void): () => void;
  /** The page now shows this review, whether the user opened it here or in the page. */
  onShowReview(runId: string): void;
  /** The plugin posted the user's review of this run, after the confirmation. */
  onSubmitted?(runId: string): void;
};

type Confirmation = { action: string; args: readonly string[]; resolve(result: PluginActionResult | null): void };

/**
 * A plugin's page, in a frame sandboxed to scripts only: the page gets an origin of its own
 * ("null"), so it cannot reach this window, Tauri, or its own files' origin, and talks to Prompting
 * Time only through postMessage. The bridge checks each message and runs the plugin action it maps
 * to; an action the manifest marks confirm opens the confirmation first. The caller makes one per
 * plugin.
 */
export function PluginFrame(props: Props) {
  const name = untrack(() => props.plugin.name);
  let frame: HTMLIFrameElement | undefined;
  const [confirmation, setConfirmation] = createSignal<Confirmation | null>(null);
  const [notice, setNotice] = createSignal<string | null>(null);
  const bridge = createPageBridge({
    plugin: () => props.plugin,
    status: () => props.status,
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
    onShowReview: (runId) => props.onShowReview(runId),
    onNotice: setNotice,
    onSubmitted: (runId) => props.onSubmitted?.(runId),
  });

  onSettled(() => {
    const onMessage = (event: MessageEvent) => {
      if (frame?.contentWindow && event.source === frame.contentWindow) bridge.receive(event.data);
    };
    window.addEventListener("message", onMessage);
    const stopListening = props.onPluginFilesChange((change) => {
      if (change.plugin === name) bridge.filesChanged(change.files);
    });
    return () => {
      window.removeEventListener("message", onMessage);
      stopListening();
      bridge.dispose();
      untrack(confirmation)?.resolve(null);
    };
  });

  createEffect(() => props.runId, (runId) => bridge.open(runId));
  createEffect(() => props.status, () => bridge.statusChanged());

  return (
    <div class="plugin-page">
      <Show when={notice()}>{(text) => <p class="plugin-page-notice" role="status">{text()}</p>}</Show>
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
