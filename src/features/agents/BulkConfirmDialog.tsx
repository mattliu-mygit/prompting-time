import { createSignal, For, onSettled, Show, untrack } from "solid-js";
import { ConfirmDialog } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginActions } from "../../app/store";

type Props = {
  plugin: PluginSnapshot;
  /** The name of an action the manifest marks `confirm: true`. */
  action: string;
  /** The items it runs on, each with its title and the arguments after the action's own, such as `--item <id>`. */
  items: readonly { title: string; args: readonly string[] }[];
  actions: PluginActions;
  /** How it ended: how many items ran, failed or would not run, and the user cancelled, with the first failure's words. */
  onClose(outcome: BulkOutcome): void;
};

export type BulkOutcome = { ran: number; failed: number; cancelled: number; failure: string | null };

/** One item in the confirmation: what its preview will send, or why it won't run. */
type Row = { title: string; args: readonly string[] } & ({ kind: "ready"; fingerprint: string; preview: string } | { kind: "refused"; reason: string });

/** The exit code of a run for real whose request changed after the preview showed it. */
const CHANGED = 13;

/**
 * One confirmation for a bulk action marked `confirm`. It previews every item in turn, shows each
 * item's title with what it will send, or "won't run" and why when its preview refused, and on
 * Confirm runs each ready item with its own fingerprint, one after another; Cancel runs nothing. The
 * items whose request changed since (exit 13) are previewed again and shown for one more
 * confirmation.
 */
export function BulkConfirmDialog(props: Props) {
  const plugin = untrack(() => props.plugin);
  const name = untrack(() => props.action);
  const title = plugin.actions.find((item) => item.name === name)?.title ?? name;
  const description = plugin.actions.find((item) => item.name === name)?.description;
  // The rows the dialog shows. Solid applies a write on its next flush, so the logic reads `shown`,
  // which changes at once, and the view reads the signal.
  let shown: Row[] = [];
  const [rows, setRows] = createSignal<Row[]>([]);
  const show = (next: Row[]) => {
    shown = next;
    setRows(next);
  };
  // "previewing" or "running" with how far it got, or "ready" for the user's choice.
  const [step, setStep] = createSignal<{ kind: "previewing" | "running"; done: number; total: number } | { kind: "ready" }>({ kind: "previewing", done: 0, total: 0 });
  const [note, setNote] = createSignal<string | null>(null);
  const totals = { ran: 0, failed: 0, failure: null as string | null };
  // How many items the current round asks about, which Cancel counts as cancelled while they preview.
  let pending = 0;
  let active = true;

  const request = (args: readonly string[], confirmedFingerprint: string | null) =>
    props.actions.pluginRequest({ plugin: plugin.name, action: name, args: [...args], confirmedFingerprint });
  const ready = (list = shown) => list.filter((row): row is Extract<Row, { kind: "ready" }> => row.kind === "ready");
  const refused = (list = shown) => list.filter((row) => row.kind === "refused");

  async function preview(items: readonly { title: string; args: readonly string[] }[]) {
    const next: Row[] = [];
    pending = items.length;
    for (const [index, item] of items.entries()) {
      setStep({ kind: "previewing", done: index, total: items.length });
      try {
        const result = await request(item.args, null);
        if (result.exitCode === 0 && result.fingerprint && result.preview !== null) next.push({ ...item, kind: "ready", fingerprint: result.fingerprint, preview: result.preview });
        else next.push({ ...item, kind: "refused", reason: result.stderr.trim() || result.stdout.trim() || `exit code ${result.exitCode ?? "a signal"}` });
      } catch (reason) {
        next.push({ ...item, kind: "refused", reason: messageFor(reason) });
      }
      if (!active) return;
    }
    show(next);
    setStep({ kind: "ready" });
  }

  /** Ends the confirmation, counting this round's items that won't run as failed. */
  function close(cancelled: number) {
    active = false;
    const failure = totals.failure ?? refused()[0]?.reason ?? null;
    props.onClose({ ran: totals.ran, failed: totals.failed + refused().length, cancelled, failure });
  }

  async function confirm() {
    const items = ready();
    const changed: Row[] = [];
    for (const [index, row] of items.entries()) {
      setStep({ kind: "running", done: index, total: items.length });
      try {
        const result = await request(row.args, row.fingerprint);
        if (result.exitCode === 0) totals.ran += 1;
        else if (result.exitCode === CHANGED) changed.push(row);
        else {
          totals.failed += 1;
          totals.failure ??= result.stderr.trim() || result.stdout.trim() || `exit code ${result.exitCode ?? "a signal"}`;
        }
      } catch (reason) {
        totals.failed += 1;
        totals.failure ??= messageFor(reason);
      }
    }
    if (changed.length === 0) return close(0);
    // This round's items that won't run count now, as a second round lists only the changed items.
    totals.failed += refused().length;
    totals.failure ??= refused()[0]?.reason ?? null;
    show([]);
    setNote(`Nothing was sent for ${changed.length === 1 ? "this item" : `these ${changed.length} items`}: what ${changed.length === 1 ? "it" : "they"} would send changed after you saw it. This is the new request; confirm again to send it.`);
    await preview(changed.map(({ title: itemTitle, args }) => ({ title: itemTitle, args })));
  }

  onSettled(() => {
    void preview(untrack(() => props.items));
    return () => {
      active = false;
    };
  });

  const count = (n: number) => `${n} ${n === 1 ? "item" : "items"}`;
  return (
    <ConfirmDialog
      open
      title={title}
      cancelLabel={step().kind === "ready" && ready(rows()).length === 0 ? "Close" : "Cancel"}
      confirmLabel={title}
      busy={step().kind === "running"}
      confirmDisabled={step().kind !== "ready" || ready(rows()).length === 0}
      onConfirm={() => void confirm()}
      onCancel={() => close(step().kind === "previewing" ? pending : ready().length)}
    >
      <Show when={description}>{(text) => <p>{text()}</p>}</Show>
      <Show when={note()}>{(text) => <p role="status" class="plugin-confirm-note">{text()}</p>}</Show>
      <Show when={step().kind === "previewing" ? step() as { done: number; total: number } : null}>
        {(now) => <p role="status">{plugin.title} is preparing what it will send ({now().done + 1} of {now().total}). Nothing is sent until you confirm.</p>}
      </Show>
      <Show when={step().kind !== "previewing"}>
        <p>
          {count(rows().length)}: {plugin.title} will send exactly this for {count(ready(rows()).length)}
          {refused(rows()).length > 0 ? `; ${count(refused(rows()).length)} won't run` : ""}.
        </p>
        <ul class="bulk-confirm-list" tabindex="0" aria-label="What it will send">
          <For each={rows()}>
            {(row) => (
              <li class="bulk-confirm-item">
                <p class="bulk-confirm-title">{row.title}</p>
                {row.kind === "ready"
                  ? <pre class="plugin-preview">{row.preview}</pre>
                  : <p class="plugin-confirm-refused">won't run: {row.reason}</p>}
              </li>
            )}
          </For>
        </ul>
        <p class="plugin-note">If a request changes before it is sent, nothing is sent for it and you see the new version.</p>
        <Show when={step().kind === "running" ? step() as { done: number; total: number } : null}>
          {(now) => <p role="status">Sending {now().done + 1} of {now().total}…</p>}
        </Show>
      </Show>
    </ConfirmDialog>
  );
}

function messageFor(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Prompting Time could not run the action.";
}
