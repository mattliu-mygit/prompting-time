import { createSignal, For, onSettled, Show, untrack } from "solid-js";
import { Button, Markdown, StatusDot, Textarea } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginActions } from "../../app/store";
import { readFailedReview, reviewTitle, type FailedReviewRecord, type ReviewAnswer, type ReviewEntry } from "./reviews";

/** The plugin actions this screen runs, by the names Whooper's manifest gives them. */
const SHOW_REVIEW = "show-review";
const RETRY = "retry";
const ROUND = "round";
const ASK = "ask";
/** The question Ask why starts with. */
const WHY = "Why did this review fail?";

type Props = {
  plugin: PluginSnapshot;
  /** The failed review as the plugin's status lists it. The caller makes one screen per run. */
  review: ReviewEntry;
  actions: PluginActions;
};

/**
 * A failed review on Prompting Time's own screen, since page protocol 1 shows only reviews with
 * findings: the pull request, why the review failed as the plugin recorded it, and the answers to
 * earlier questions, from the show-review action. Retry runs the plugin's retry action, which queues
 * the pull request for its next round, and then offers Run now, which runs the round action. Ask
 * why runs the ask action with the user's question and adds the answer. A failure shows the plugin's
 * own words.
 */
export function FailedReview(props: Props) {
  // A run's pull request does not change, and there is one screen per run, so the entry is read once.
  const review = untrack(() => props.review);
  const [record, setRecord] = createSignal<FailedReviewRecord | null>(null);
  const [readError, setReadError] = createSignal<string | null>(null);
  const [retried, setRetried] = createSignal<string | null>(null);
  const [roundOutput, setRoundOutput] = createSignal<string | null>(null);
  const [running, setRunning] = createSignal<"retry" | "round" | null>(null);
  const [runError, setRunError] = createSignal<string | null>(null);
  const [asking, setAsking] = createSignal(false);
  const [question, setQuestion] = createSignal(WHY);
  const [askBusy, setAskBusy] = createSignal(false);
  const [askError, setAskError] = createSignal<string | null>(null);
  const [newAnswers, setNewAnswers] = createSignal<ReviewAnswer[]>([]);
  let disposed = false;

  const answers = () => [...(record()?.answers ?? []), ...newAnswers()];
  const askHint = () => props.plugin.actions.find((action) => action.name === ASK)?.description;

  /** Runs one of the plugin's actions to its end and returns what it printed; a failure throws the plugin's words. */
  async function run(name: string, args: string[]): Promise<string> {
    const plugin = props.plugin;
    const action = plugin.actions.find((item) => item.name === name);
    if (!action) throw new Error(`${plugin.title} has no ${name} action.`);
    // This screen has no confirmation to offer, so it runs only actions that need none.
    if (action.confirm) throw new Error(`${plugin.title} asks to confirm ${name} first, which this screen cannot do.`);
    const result = await props.actions.pluginRequest({ plugin: plugin.name, action: name, args, confirmedFingerprint: null });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `${name} exited with ${result.exitCode ?? "a signal"}.`);
    return result.stdout.trim();
  }

  async function load() {
    try {
      const shown = readFailedReview(await run(SHOW_REVIEW, [review.run_id]), review.run_id);
      if (!disposed) setRecord(shown);
    } catch (reason) {
      if (!disposed) setReadError(messageOf(reason));
    }
  }

  /** Retry, then Run now: the button runs retry until it succeeds, and the round after it. */
  async function advance() {
    const step = retried() === null ? "retry" : "round";
    setRunning(step);
    setRunError(null);
    try {
      const output = await (step === "retry" ? run(RETRY, [review.run_id]) : run(ROUND, []));
      if (disposed) return;
      if (step === "retry") setRetried(output || `${props.plugin.title} retries the review in its next round.`);
      else setRoundOutput(output || "The round printed nothing.");
    } catch (reason) {
      if (!disposed) setRunError(messageOf(reason));
    } finally {
      if (!disposed) setRunning(null);
    }
  }

  async function ask(event: SubmitEvent) {
    event.preventDefault();
    const text = question().trim();
    if (!text || askBusy()) return;
    setAskBusy(true);
    setAskError(null);
    try {
      const answer = await run(ASK, [review.run_id, text]);
      if (disposed) return;
      setNewAnswers([...newAnswers(), { question: text, answer }]);
      setQuestion("");
      setAsking(false);
    } catch (reason) {
      if (!disposed) setAskError(messageOf(reason));
    } finally {
      if (!disposed) setAskBusy(false);
    }
  }

  const primaryLabel = () => {
    if (retried() === null) return running() === "retry" ? "Retrying…" : "Retry";
    return running() === "round" ? "Running…" : "Run now";
  };

  onSettled(() => {
    void untrack(load);
    return () => {
      disposed = true;
    };
  });

  return (
    <section class="plugin-scroll" aria-labelledby="failed-review-title">
      <div class="plugin-column">
        <header class="plugin-heading">
          <p class="eyebrow">Pull request</p>
          <h2 id="failed-review-title">{reviewTitle(review)}</h2>
          <p class="plugin-meta">{review.repo}#{review.pr}</p>
        </header>
        <section class="plugin-section failed-review" aria-labelledby="failed-review-heading">
          <h3 id="failed-review-heading"><StatusDot status="error" />Review failed</h3>
          <Show
            when={readError()}
            fallback={
              <Show when={record()} fallback={<p role="status" class="plugin-note">Reading the review…</p>}>
                {(shown) => (
                  <Show when={shown().failure} fallback={<p>{props.plugin.title} didn't record why this review failed.</p>}>
                    {(failure) => <pre class="plugin-log" tabindex="0" aria-label="Why it failed">{failure()}</pre>}
                  </Show>
                )}
              </Show>
            }
          >
            {(error) => <p role="alert" class="inline-error plugin-refusal">{error()}</p>}
          </Show>
          <div class="failed-review-actions">
            <Button variant="primary" disabled={running() !== null} onClick={() => void advance()}>{primaryLabel()}</Button>
            <Button disabled={asking()} onClick={() => setAsking(true)}>Ask why</Button>
          </div>
          <Show when={retried()}>{(output) => <p role="status">{output()}</p>}</Show>
          <Show when={running() === "round"}>
            <p role="status" class="plugin-note">{props.plugin.title} is running a round, which can take several minutes.</p>
          </Show>
          <Show when={roundOutput()}>{(output) => <pre class="plugin-log" tabindex="0" aria-label="What the round printed">{output()}</pre>}</Show>
          <Show when={runError()}>{(error) => <p role="alert" class="inline-error plugin-refusal">{error()}</p>}</Show>
          <Show when={asking()}>
            <form class="failed-review-ask" onSubmit={(event) => void ask(event)}>
              <Textarea
                ref={(element: HTMLTextAreaElement) => queueMicrotask(() => element.focus())}
                label="Question"
                rows={2}
                grow
                value={question()}
                disabled={askBusy()}
                onInput={(event) => setQuestion(event.currentTarget.value)}
              />
              <div class="failed-review-actions">
                <Button type="submit" variant="primary" disabled={askBusy() || !question().trim()}>{askBusy() ? "Asking…" : "Ask"}</Button>
                <Button disabled={askBusy()} onClick={() => { setAsking(false); setAskError(null); }}>Cancel</Button>
              </div>
              <Show when={askHint()}>{(hint) => <p class="plugin-note">{hint()}</p>}</Show>
              <Show when={askError()}>{(error) => <p role="alert" class="inline-error plugin-refusal">{error()}</p>}</Show>
            </form>
          </Show>
        </section>
        <Show when={answers().length > 0}>
          <section class="plugin-section" aria-labelledby="failed-review-answers">
            <h3 id="failed-review-answers" class="section-heading">Answers</h3>
            <ol class="review-answers" aria-labelledby="failed-review-answers">
              <For each={answers()}>
                {(entry) => (
                  <li>
                    <p class="review-answer-question">{entry.question}</p>
                    <Markdown content={entry.answer} />
                  </li>
                )}
              </For>
            </ol>
          </section>
        </Show>
      </div>
    </section>
  );
}

function messageOf(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Prompting Time could not run the action.";
}
