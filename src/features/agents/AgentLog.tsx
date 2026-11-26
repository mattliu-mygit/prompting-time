import { createSignal, onSettled, Show, untrack } from "solid-js";
import { CopyButton, TextField } from "@prompting-time/ui";
import type { PluginFilesChange } from "../../app/store";
import type { PluginLog, PluginSnapshot } from "../../bridge/types";
import { messageOf } from "./actions";

/**
 * While the plugin keeps changing its files, its log is read again at most this often: a review
 * writes its transcript several times a second, and each read is a command.
 */
const REREAD_INTERVAL_MS = 2000;

type Props = {
  plugin: PluginSnapshot;
  readLog(name: string): Promise<PluginLog>;
  onPluginFilesChange(listener: (change: PluginFilesChange) => void): () => void;
};

/**
 * The end of what the agent's start command printed since Prompting Time last started it, with a
 * search that keeps only matching lines and a copy of the lines shown. It reads the log when it
 * opens and again as the plugin's files change, which they do while it works.
 */
export function AgentLog(props: Props) {
  const name = untrack(() => props.plugin.name);
  const [log, setLog] = createSignal<PluginLog | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [query, setQuery] = createSignal("");
  let reading = false;
  let again = false;
  let disposed = false;
  let box: HTMLPreElement | undefined;

  const lines = () => (log()?.text ? log()!.text.split("\n") : []);
  const shown = () => lines().filter((line) => line.toLowerCase().includes(query().trim().toLowerCase()));

  async function read() {
    if (reading) {
      again = true;
      return;
    }
    reading = true;
    do {
      again = false;
      try {
        const next = await props.readLog(name);
        if (disposed) return;
        const atEnd = !box || box.scrollTop + box.clientHeight >= box.scrollHeight - 4;
        setLog(next);
        setError(null);
        // Follow new lines while the reader is at the end, as a terminal does.
        if (atEnd) queueMicrotask(() => box && (box.scrollTop = box.scrollHeight));
      } catch (reason) {
        if (!disposed) setError(messageOf(reason));
      }
      if (again && !disposed) await new Promise((resolve) => setTimeout(resolve, REREAD_INTERVAL_MS));
    } while (again && !disposed);
    reading = false;
  }

  onSettled(() => {
    void read();
    const stop = props.onPluginFilesChange((change) => {
      if (change.plugin === name) void read();
    });
    return () => {
      disposed = true;
      stop();
    };
  });

  return (
    <div class="agent-panel agent-log">
      <div class="agent-log-bar">
        <TextField label="Search the log" value={query()} onInput={(event) => setQuery(event.currentTarget.value)} />
        <span class="agent-muted" role="status">{shown().length === 1 ? "1 line" : `${shown().length} lines`}</span>
        <CopyButton content={shown().join("\n")} label="Copy log" />
      </div>
      <Show when={error()}>{(text) => <p role="alert" class="inline-error plugin-refusal">{text()}</p>}</Show>
      <Show when={props.plugin.process.kind === "runningElsewhere"}>
        <p class="plugin-note">A copy started elsewhere runs {props.plugin.title}, so its output goes where it was started; this is what Prompting Time's last copy printed.</p>
      </Show>
      <Show when={log()} fallback={<Show when={!error()}><p role="status" class="plugin-note">Reading the log…</p></Show>}>
        {(current) => (
          <>
            <Show when={current().truncated}><p class="plugin-note">The end of {current().path}.</p></Show>
            <Show when={lines().length > 0} fallback={<p class="work-empty">{props.plugin.title} has printed nothing since Prompting Time started it.</p>}>
              <pre ref={(element: HTMLPreElement) => { box = element; }} class="plugin-log agent-log-text" tabindex="0" aria-label={`${props.plugin.title}'s log`}>{shown().join("\n")}</pre>
            </Show>
          </>
        )}
      </Show>
    </div>
  );
}
