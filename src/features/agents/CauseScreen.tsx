import { createMemo, createSignal, For, onSettled, Show } from "solid-js";
import { AlertIcon, Button, CloseIcon, ConfirmDialog, IconButton, ListRow, Markdown, PullRequestIcon, Textarea } from "@prompting-time/ui";
import type { PluginActions, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { ACTIONS, hasAction, messageOf, runPluginAction } from "./actions";
import { RunNow } from "./RunNow";
import { blockingCauses, readReviews, reviewState, reviewTitle, shortDate, shortPullRequest, waitingText, type ReviewAnswer, type ReviewEntry } from "./reviews";
import { readAgentStatus, readBlocked } from "./status";

type Props = {
  plugin: PluginSnapshot;
  status: PluginStatusRead | undefined;
  /** The cause's ID, as the plugin names it. The caller makes one screen per cause. */
  cause: string;
  actions: PluginActions;
  onOpenReview(runId: string): void;
  onOpenLogs(): void;
  /** Takes these reviews out of the lists, the app offering Undo; throws when the plugin refused every one. */
  onRemove(runIds: readonly string[]): Promise<void>;
};

type Retried = { queued: number; refused: Array<{ review: ReviewEntry; reason: string }> };

/**
 * A cause that blocks every review, such as Docker not running, with the reviews it stopped: Retry
 * all runs the plugin's retry action for each waiting or failed review, one after another, then
 * offers Run now; Ask why runs the ask action about the newest of them; Open logs opens the
 * plugin's log; and the pull requests are listed by repository, each opening its review.
 */
export function CauseScreen(props: Props) {
  const reviews = createMemo(() => readReviews(props.status?.value));
  const group = createMemo(() => blockingCauses(reviews(), readBlocked(readAgentStatus(props.status))).find((cause) => cause.id === props.cause) ?? null);
  const affected = () => group()?.reviews ?? [];
  const since = () => group()?.since ?? affected().map((review) => review.ended_at ?? review.started_at).sort()[0] ?? null;
  const byRepo = createMemo(() => {
    const repos = new Map<string, ReviewEntry[]>();
    for (const review of affected()) repos.set(review.repo, [...(repos.get(review.repo) ?? []), review]);
    return [...repos];
  });
  const [progress, setProgress] = createSignal<{ done: number; total: number } | null>(null);
  const [retried, setRetried] = createSignal<Retried | null>(null);
  const [asking, setAsking] = createSignal(false);
  const [question, setQuestion] = createSignal("Why did this review fail?");
  const [askBusy, setAskBusy] = createSignal(false);
  const [askError, setAskError] = createSignal<string | null>(null);
  const [answers, setAnswers] = createSignal<Array<ReviewAnswer & { about: string }>>([]);
  // Remove all asks first; while it runs, and why the plugin refused.
  const [removeAsked, setRemoveAsked] = createSignal(false);
  const [removing, setRemoving] = createSignal(false);
  const [removeError, setRemoveError] = createSignal<string | null>(null);
  let disposed = false;
  const canRemove = () => hasAction(props.plugin, ACTIONS.remove);
  const plural = (count: number) => (count === 1 ? "1 review" : `${count} reviews`);

  async function remove(runIds: readonly string[]) {
    setRemoving(true);
    setRemoveError(null);
    try {
      await props.onRemove(runIds);
    } catch (reason) {
      if (!disposed) setRemoveError(messageOf(reason));
    } finally {
      if (!disposed) {
        setRemoving(false);
        setRemoveAsked(false);
      }
    }
  }

  const run = (name: string, args: string[]) => runPluginAction(props.actions, props.plugin, name, args);

  /** Retries each review the cause stopped, one at a time: each retry is a process of the plugin's that writes its queue. */
  async function retryAll() {
    const targets = affected();
    const refused: Retried["refused"] = [];
    setRetried(null);
    for (const [index, review] of targets.entries()) {
      setProgress({ done: index, total: targets.length });
      try {
        await run(ACTIONS.retry, [review.run_id]);
      } catch (reason) {
        refused.push({ review, reason: messageOf(reason) });
      }
      if (disposed) return;
    }
    setProgress(null);
    setRetried({ queued: targets.length - refused.length, refused });
  }

  async function ask(event: SubmitEvent) {
    event.preventDefault();
    const review = affected()[0];
    const text = question().trim();
    if (!review || !text || askBusy()) return;
    setAskBusy(true);
    setAskError(null);
    try {
      const answer = await run(ACTIONS.ask, [review.run_id, text]);
      if (disposed) return;
      setAnswers([...answers(), { question: text, answer, about: shortPullRequest(review) }]);
      setQuestion("");
      setAsking(false);
    } catch (reason) {
      if (!disposed) setAskError(messageOf(reason));
    } finally {
      if (!disposed) setAskBusy(false);
    }
  }

  onSettled(() => () => {
    disposed = true;
  });

  const queuedText = (result: Retried) => `Queued ${result.queued === 1 ? "1 review" : `${result.queued} reviews`} for ${props.plugin.title}'s next round.`;

  return (
    <section class="plugin-scroll" aria-labelledby="cause-title">
      <div class="plugin-column">
        <header class="plugin-heading">
          <p class="eyebrow">Blocks every review · {props.plugin.title}</p>
          <h2 id="cause-title" class="cause-title"><AlertIcon />{group()?.summary ?? props.cause}</h2>
          <p class="plugin-meta">{waitingText(affected().length)}{since() ? ` · since ${shortDate(since()!)}` : ""}</p>
        </header>
        <section class="plugin-section failed-review" aria-label="What to do">
          <p>{props.plugin.title} stops a round when this cause blocks every review, and the round's other reviews wait instead of failing. Fix the cause, then retry them; the next round tries again.</p>
          <div class="failed-review-actions">
            <Show when={hasAction(props.plugin, ACTIONS.retry)}>
              <Button variant="primary" disabled={progress() !== null || affected().length === 0} onClick={() => void retryAll()}>
                {progress() ? `Retrying ${progress()!.done + 1} of ${progress()!.total}…` : `Retry all ${affected().length}`}
              </Button>
            </Show>
            <Show when={hasAction(props.plugin, ACTIONS.ask)}>
              <Button disabled={asking() || affected().length === 0} onClick={() => setAsking(true)}>Ask why</Button>
            </Show>
            <Button onClick={() => props.onOpenLogs()}>Open logs</Button>
            <Show when={canRemove()}>
              <Button disabled={removing() || affected().length === 0} onClick={() => setRemoveAsked(true)}>Remove all</Button>
            </Show>
          </div>
          <Show when={removeError()}>{(error) => <p role="alert" class="inline-error plugin-refusal">{error()}</p>}</Show>
          <Show when={retried()}>
            {(result) => (
              <>
                <Show when={result().refused.length > 0}>
                  <div role="alert" class="inline-error plugin-refusal">
                    <p>{props.plugin.title} refused {result().refused.length === 1 ? "1 retry" : `${result().refused.length} retries`}:</p>
                    <ul class="cause-refusals">
                      <For each={result().refused}>{(item) => <li>{shortPullRequest(item.review)}: {item.reason}</li>}</For>
                    </ul>
                  </div>
                </Show>
                <Show when={result().queued > 0}>
                  <RunNow plugin={props.plugin} actions={props.actions} queued={queuedText(result())} />
                </Show>
              </>
            )}
          </Show>
          <Show when={asking() && affected()[0]}>
            {(review) => (
              <form class="failed-review-ask" onSubmit={(event) => void ask(event)}>
                <Textarea
                  ref={(element: HTMLTextAreaElement) => queueMicrotask(() => element.focus())}
                  label={`Question about ${shortPullRequest(review())}'s review`}
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
                <Show when={askError()}>{(error) => <p role="alert" class="inline-error plugin-refusal">{error()}</p>}</Show>
              </form>
            )}
          </Show>
        </section>
        <Show when={answers().length > 0}>
          <section class="plugin-section" aria-labelledby="cause-answers">
            <h3 id="cause-answers" class="section-heading">Answers</h3>
            <ol class="review-answers" aria-labelledby="cause-answers">
              <For each={answers()}>
                {(entry) => (
                  <li>
                    <p class="review-answer-question">{entry.question} · about {entry.about}</p>
                    <Markdown content={entry.answer} />
                  </li>
                )}
              </For>
            </ol>
          </section>
        </Show>
        <For each={byRepo()}>
          {([repo, items]) => (
            <section class="plugin-section" aria-labelledby={`cause-repo-${repo.replace(/[^a-z0-9]/gi, "-")}`}>
              <h3 id={`cause-repo-${repo.replace(/[^a-z0-9]/gi, "-")}`} class="section-heading">{repo} · {items.length}</h3>
              <ul class="work-rows">
                <For each={items} keyed={(review) => review.run_id}>
                  {(review) => (
                    <li class="cause-row">
                      <ListRow
                        icon={<PullRequestIcon />}
                        title={reviewTitle(review())}
                        status={reviewState(review()).status}
                        meta={`${shortPullRequest(review())} · ${reviewState(review()).words} · ${shortDate(review().ended_at ?? review().started_at)}`}
                        onClick={() => props.onOpenReview(review().run_id)}
                      />
                      <Show when={canRemove()}>
                        <IconButton class="row-remove" label={`Remove ${shortPullRequest(review())}`} disabled={removing()} onClick={() => void remove([review().run_id])}>
                          <CloseIcon />
                        </IconButton>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>
            </section>
          )}
        </For>
      </div>
      <ConfirmDialog
        open={removeAsked()}
        busy={removing()}
        title={`Remove ${plural(affected().length)}?`}
        cancelLabel="Keep them"
        confirmLabel={`Remove ${plural(affected().length)}`}
        confirmDisabled={affected().length === 0}
        onConfirm={() => void remove(affected().map((review) => review.run_id))}
        onCancel={() => setRemoveAsked(false)}
      >
        <p>They wait on “{group()?.summary ?? props.cause}”. Removing takes them out of your lists, and {props.plugin.title} won't retry them. You can undo right after.</p>
        <p>Each review's files stay on disk in {props.plugin.title}'s state folder, and search still finds them, tagged removed.</p>
      </ConfirmDialog>
    </section>
  );
}
