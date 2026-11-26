import { createMemo, createSignal, onSettled, Show, untrack } from "solid-js";
import { ConfirmDialog } from "@prompting-time/ui";
import type { PluginActionResult, PluginSnapshot } from "../../bridge/types";
import type { PluginActions } from "../../app/store";

type Props = {
  plugin: PluginSnapshot;
  /** The name of an action the manifest marks `confirm: true`, such as Whooper's submit. */
  action: string;
  /** The arguments after the action's own, such as a review's run ID. */
  args: readonly string[];
  actions: PluginActions;
  /** The result of the last run for real, or null when nothing ran for real. */
  onClose(result: PluginActionResult | null): void;
};

type Override = { flag: string; label: string };
type Step =
  | { kind: "previewing"; note: string | null }
  | { kind: "ready" | "running"; fingerprint: string; preview: string; note: string | null }
  | { kind: "stopped"; message: string; detail: string; override: Override | null };

/**
 * How a confirm action refuses, by exit code, as whooper-submit defines them. Each refusal sends
 * nothing; 11 and 12 can be overridden with the flag after the user has read why.
 */
const REFUSALS: Record<number, (title: string) => { message: string; override?: Override }> = {
  10: (title) => ({ message: `Nothing was sent: the pull request has new commits since the review. ${title} reviews them in its next round.` }),
  11: (title) => ({
    message: `Nothing was sent: the review approves, and ${title} does not count this run as safe to approve. Approve anyway only if what it lists is fine with you.`,
    override: { flag: "--approve-anyway", label: "Approve anyway" },
  }),
  12: () => ({
    message: "Nothing was sent: the review @-mentions someone, and sending it would notify them.",
    override: { flag: "--allow-mentions", label: "Send with the mentions" },
  }),
};
/** The exit code of a run for real whose request changed after the preview showed it. */
const CHANGED = 13;

/**
 * The confirmation every action marked `confirm` waits for. It runs the action's preview, which
 * changes nothing outside this Mac, shows exactly what the action will send, and runs it for real
 * only when the user confirms, with the fingerprint of the request shown. A request that changed
 * since (exit 13) is shown again for a new confirmation; other refusals say what happened.
 */
export function ConfirmActionDialog(props: Props) {
  const plugin = untrack(() => props.plugin);
  const name = untrack(() => props.action);
  const action = plugin.actions.find((item) => item.name === name);
  const title = action?.title ?? name;
  const [step, setStep] = createSignal<Step>({ kind: "previewing", note: null });
  let flags: string[] = [];
  let lastRun: PluginActionResult | null = null;
  let active = true;

  function request(confirmedFingerprint: string | null) {
    return props.actions.pluginRequest({ plugin: plugin.name, action: name, args: [...untrack(() => props.args), ...flags], confirmedFingerprint });
  }

  function stop(result: PluginActionResult) {
    const detail = result.stderr.trim() || result.stdout.trim();
    const refusal = result.exitCode === null ? undefined : REFUSALS[result.exitCode]?.(plugin.title);
    setStep(refusal
      ? { kind: "stopped", message: refusal.message, detail, override: refusal.override && !flags.includes(refusal.override.flag) ? refusal.override : null }
      : { kind: "stopped", message: `${title} stopped ${result.exitCode === null ? "on a signal" : `with exit code ${result.exitCode}`}. ${plugin.title} said:`, detail, override: null });
  }

  async function preview(note: string | null) {
    setStep({ kind: "previewing", note });
    try {
      const result = await request(null);
      if (!active) return;
      if (result.exitCode === 0 && result.fingerprint && result.preview !== null) {
        setStep({ kind: "ready", fingerprint: result.fingerprint, preview: result.preview, note });
      } else {
        stop(result);
      }
    } catch (reason) {
      if (active) setStep({ kind: "stopped", message: messageFor(reason), detail: "", override: null });
    }
  }

  async function confirm() {
    const current = step();
    if (current.kind === "stopped" && current.override) {
      flags = [...flags, current.override.flag];
      void preview(null);
      return;
    }
    if (current.kind !== "ready") return;
    setStep({ ...current, kind: "running" });
    try {
      lastRun = await request(current.fingerprint);
    } catch (reason) {
      if (active) setStep({ kind: "stopped", message: messageFor(reason), detail: "", override: null });
      return;
    }
    if (!active) return;
    if (lastRun.exitCode === 0) props.onClose(lastRun);
    else if (lastRun.exitCode === CHANGED) void preview("Nothing was sent: what it would send changed after you saw it. This is the new request; confirm again to send it.");
    else stop(lastRun);
  }

  onSettled(() => {
    void preview(null);
    return () => {
      active = false;
    };
  });

  const shown = createMemo(() => {
    const now = step();
    return now.kind === "ready" || now.kind === "running" ? now : null;
  });
  const stopped = createMemo(() => {
    const now = step();
    return now.kind === "stopped" ? now : null;
  });
  const note = () => {
    const now = step();
    return now.kind === "stopped" ? null : now.note;
  };
  return (
    <ConfirmDialog
      open
      title={title}
      cancelLabel={stopped() ? "Close" : "Cancel"}
      confirmLabel={stopped()?.override?.label ?? title}
      busy={step().kind === "running"}
      confirmDisabled={step().kind === "previewing" || (stopped() !== null && !stopped()!.override)}
      onConfirm={() => void confirm()}
      onCancel={() => {
        active = false;
        props.onClose(lastRun);
      }}
    >
      <Show when={action?.description}>{(description) => <p>{description()}</p>}</Show>
      <Show when={note()}>{(text) => <p role="status" class="plugin-confirm-note">{text()}</p>}</Show>
      <Show when={step().kind === "previewing"}>
        <p role="status">{plugin.title} is preparing what it will send. Nothing is sent until you confirm.</p>
      </Show>
      <Show when={shown()}>
        {(ready) => (
          <>
            <p>{plugin.title} will send exactly this:</p>
            <pre class="plugin-preview" tabindex="0" aria-label="What it will send">{ready().preview}</pre>
            <p class="plugin-note">If it changes before it is sent, nothing is sent and you see the new version.</p>
            <Show when={ready().kind === "running"}><p role="status">Sending…</p></Show>
          </>
        )}
      </Show>
      <Show when={stopped()}>
        {(refused) => (
          <>
            <p role="alert">{refused().message}</p>
            <Show when={refused().detail}>
              <pre class="plugin-preview plugin-preview-text" tabindex="0" aria-label={`What ${plugin.title} said`}>{refused().detail}</pre>
            </Show>
          </>
        )}
      </Show>
    </ConfirmDialog>
  );
}

function messageFor(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Prompting Time could not run the action.";
}
