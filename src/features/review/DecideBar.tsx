import { createSignal, Show, untrack } from "solid-js";
import { Button } from "@prompting-time/ui";
import type { PluginActions } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { messageOf, runPluginAction } from "../agents/actions";
import { ConfirmActionDialog } from "../agents/ConfirmActionDialog";
import type { ReviewEntry } from "../agents/reviews";
import { plural } from "./ReviewTop";
import type { ShownReview } from "./shown";

type Props = {
  plugin: PluginSnapshot;
  review: ReviewEntry;
  shown: ShownReview;
  actions: PluginActions;
  /** Whether the bar shows in full; until then it waits as one Decide… button. */
  open: boolean;
  onOpen(): void;
  /** The drafts are edited in the plugin's page, the review's Diff. */
  onEditDrafts(): void;
  /** The plugin posted the review, after the confirmation. */
  onSubmitted(runId: string): void;
};

/**
 * The review's decision: Comment or Approve, and Submit review, which sets the drafts' event with the
 * plugin's draft action and then submits through the confirmation every confirm action gets. It
 * waits as one Decide… button in the corner until you click it or reach the end of the walkthrough.
 */
export function DecideBar(props: Props) {
  const [event, setEvent] = createSignal<"APPROVE" | "COMMENT">(untrack(() => props.shown.drafts?.event ?? "COMMENT"));
  const [confirming, setConfirming] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let bar: HTMLDivElement | undefined;
  const included = () => props.shown.findings.filter((finding) => props.shown.drafts?.findings[finding.id]?.included).length;
  const name = () => `decide-${props.review.run_id}`;

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      await runPluginAction(props.actions, props.plugin, "draft", [props.review.run_id, event() === "APPROVE" ? "approve" : "comment"]);
      setConfirming(true);
    } catch (reason) {
      setError(messageOf(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Show
      when={props.open}
      fallback={
        <div class="decide-corner">
          <Button
            variant="primary"
            onClick={() => {
              props.onOpen();
              // The choice takes the focus once the bar shows, so the keyboard carries on there.
              requestAnimationFrame(() => bar?.querySelector<HTMLInputElement>("input:checked")?.focus());
            }}
          >
            Decide…
          </Button>
        </div>
      }
    >
      <div class="decide-bar" role="group" aria-label="Your decision" ref={(element: HTMLDivElement) => { bar = element; }}>
        <span class="decide-count">{plural(props.shown.findings.length, "draft comment")}, {included()} included</span>
        <label class="decide-choice"><input type="radio" name={name()} checked={event() === "COMMENT"} onChange={() => setEvent("COMMENT")} /> Comment</label>
        <label class="decide-choice"><input type="radio" name={name()} checked={event() === "APPROVE"} onChange={() => setEvent("APPROVE")} /> Approve</label>
        <button type="button" class="decide-edit" onClick={() => props.onEditDrafts()}>Edit drafts in Diff</button>
        <span class="decide-spacer" />
        <Show when={error()}>{(text) => <span role="alert" class="inline-error">{text()}</span>}</Show>
        <Button variant="primary" disabled={busy()} onClick={() => void submit()}>Submit review</Button>
        <Show when={confirming()}>
          <ConfirmActionDialog
            plugin={props.plugin}
            action="submit"
            args={[props.review.run_id]}
            actions={props.actions}
            onClose={(result) => {
              setConfirming(false);
              if (result?.exitCode === 0) props.onSubmitted(props.review.run_id);
            }}
          />
        </Show>
      </div>
    </Show>
  );
}
