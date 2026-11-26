import { createSignal, onSettled, Show } from "solid-js";
import { Button } from "@prompting-time/ui";
import type { PluginActions } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { ACTIONS, hasAction, messageOf, runPluginAction } from "./actions";

type Props = {
  plugin: PluginSnapshot;
  actions: PluginActions;
  /** What the retry did, such as what the plugin printed. */
  queued: string;
};

/**
 * What follows a retry, which queues reviews for the plugin's next round: what it queued, and Run
 * now, which runs the round action now and shows what the round printed, or the plugin's words when
 * it fails.
 */
export function RunNow(props: Props) {
  const [running, setRunning] = createSignal(false);
  const [output, setOutput] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  let disposed = false;

  async function run() {
    setRunning(true);
    setError(null);
    try {
      const printed = await runPluginAction(props.actions, props.plugin, ACTIONS.round);
      if (!disposed) setOutput(printed || "The round printed nothing.");
    } catch (reason) {
      if (!disposed) setError(messageOf(reason));
    } finally {
      if (!disposed) setRunning(false);
    }
  }

  onSettled(() => () => {
    disposed = true;
  });

  return (
    <div class="run-now">
      <div class="failed-review-actions">
        <p role="status">{props.queued}</p>
        <Show when={hasAction(props.plugin, ACTIONS.round)}>
          <Button variant="primary" disabled={running()} onClick={() => void run()}>{running() ? "Running…" : "Run now"}</Button>
        </Show>
      </div>
      <Show when={running()}>
        <p role="status" class="plugin-note">{props.plugin.title} is running a round, which can take several minutes.</p>
      </Show>
      <Show when={output()}>{(printed) => <pre class="plugin-log" tabindex="0" aria-label="What the round printed">{printed()}</pre>}</Show>
      <Show when={error()}>{(text) => <p role="alert" class="inline-error plugin-refusal">{text()}</p>}</Show>
    </div>
  );
}
