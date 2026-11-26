import { createSignal, For, onSettled, Show, untrack } from "solid-js";
import { Button, Markdown, StatusDot, Textarea } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginActions } from "../../app/store";
import { ACTIONS, messageOf, runPluginAction } from "./actions";
import { readFailedReview, reviewTitle, waitingText, type FailedReviewRecord, type ReviewAnswer, type ReviewEntry } from "./reviews";

type Props = {
  plugin: PluginSnapshot;
  /** The failed or waiting review as the plugin's status lists it. The caller makes one screen per run. */
  review: ReviewEntry;
  actions: PluginActions;
  /** The reviews waiting on the same cause that blocks every review, with a way to open them all; null otherwise. */
  sameCause: { count: number; open(): void } | null;
};

/**
 * A review that failed or waits, on Prompting Time's own screen, since page protocol 1 shows only
 * reviews with findings: the pull request, the cause the plugin named, why the review failed as it
 * recorded it, and the answers to earlier questions, from the show-review action. A review waiting
 * on a cause that blocks every review links to that cause's screen. Ask why runs the ask action
 * with the user's question and adds the answer; the header above retries. A failure shows the
 * plugin's own words.
 */
export function FailedReview(props: Props) {
  // A run's pull request does not change, and there is one screen per run, so the entry is read once.
  const review = untrack(() => props.review);
  const [record, setRecord] = createSignal<FailedReviewRecord | null>(null);
  const [readError, setReadError] = createSignal<string | null>(null);
  const [asking, setAsking] = createSignal(false);
  const waiting = review.outcome === "waiting";
  const [question, setQuestion] = createSignal(waiting ? "Why is this review waiting?" : "Why did this review fail?");
  const [askBusy, setAskBusy] = createSignal(false);
  const [askError, setAskError] = createSignal<string | null>(null);
  const [newAnswers, setNewAnswers] = createSignal<ReviewAnswer[]>([]);
  let disposed = false;

  const answers = () => [...(record()?.answers ?? []), ...newAnswers()];
  const askHint = () => props.plugin.actions.find((action) => action.name === ACTIONS.ask)?.description;

  const run = (name: string, args: string[]) => runPluginAction(props.actions, props.plugin, name, args);

  async function load() {
    try {
      const shown = readFailedReview(await run(ACTIONS.showReview, [review.run_id]), review.run_id);
      if (!disposed) setRecord(shown);
    } catch (reason) {
      if (!disposed) setReadError(messageOf(reason));
    }
  }

  async function ask(event: SubmitEvent) {
    event.preventDefault();
    const text = question().trim();
    if (!text || askBusy()) return;
    setAskBusy(true);
    setAskError(null);
    try {
      const answer = await run(ACTIONS.ask, [review.run_id, text]);
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
          <h3 id="failed-review-heading"><Show when={!waiting}><StatusDot status="error" /></Show>{waiting ? `Waiting · ${review.cause?.summary ?? "blocked"}` : "Review failed"}</h3>
          <Show when={!waiting && review.cause}>{(cause) => <p>{cause().summary}</p>}</Show>
          <Show when={waiting}>
            <p>{props.plugin.title} stopped its round when this cause blocked every review, so this review waits for the next round.</p>
          </Show>
          <Show
            when={readError()}
            fallback={
              <Show when={record()} fallback={<p role="status" class="plugin-note">Reading the review…</p>}>
                {(shown) => (
                  <Show when={shown().failure} fallback={<Show when={!waiting && !review.cause}><p>{props.plugin.title} didn't record why this review failed.</p></Show>}>
                    {(failure) => <pre class="plugin-log" tabindex="0" aria-label="Why it failed">{failure()}</pre>}
                  </Show>
                )}
              </Show>
            }
          >
            {(error) => <p role="alert" class="inline-error plugin-refusal">{error()}</p>}
          </Show>
          <div class="failed-review-actions">
            <Button disabled={asking()} onClick={() => setAsking(true)}>Ask why</Button>
            <Show when={props.sameCause}>
              {(cause) => <Button onClick={() => cause().open()}>See all {waitingText(cause().count)}</Button>}
            </Show>
          </div>
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
