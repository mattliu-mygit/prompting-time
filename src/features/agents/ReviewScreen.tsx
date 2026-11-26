import { createEffect, createMemo, createSignal, Match, onSettled, Show, Switch } from "solid-js";
import { Button, Menu, StatusDot } from "@prompting-time/ui";
import type { AgentControls, PluginActions, PluginFilesChange, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { PluginFrame } from "../plugins/PluginFrame";
import { ACTIONS, hasAction, messageOf, runPluginAction } from "./actions";
import { FailedReview } from "./FailedReview";
import { RunNow } from "./RunNow";
import { blockingCauses, notSafeToApprove, readFindingKinds, readReviews, reviewState, shortDate, shortPullRequest, shortSha, unsafeVerdict, versionsOf, type ReviewEntry } from "./reviews";
import { readAgentStatus, readBlocked } from "./status";

type Props = {
  plugin: PluginSnapshot;
  status: PluginStatusRead | undefined;
  /** The run the user opened: the newest review of its pull request, or an earlier version. */
  runId: string;
  actions: PluginActions;
  controls: Pick<AgentControls, "refreshPlugins">;
  onPluginFilesChange(listener: (change: PluginFilesChange) => void): () => void;
  onOpenReview(runId: string): void;
  onOpenCause(cause: string): void;
  /** Skip, Remove, and Restore run here, so that after Skip or Remove the next item opens; each throws the plugin's refusal. */
  triage: ReviewTriage;
  /** The plugin posted the user's review of this run from its page. */
  onSubmitted(runId: string): void;
};

/** What a review's header asks of the app for its run: skip it, take it out of the lists, or put it back. */
export type ReviewTriage = { skip(runId: string): Promise<void>; remove(runId: string): Promise<void>; restore(runId: string): Promise<void> };

/**
 * The open review's state words in the window's header, such as "approve once CI passes". For a
 * review that says approve once CI passes but that the plugin judges not safe to approve, they say
 * why, from the kinds of its findings, which show-review prints: "approve once CI passes · has a
 * question". Until show-review answers, or when it fails, they say what the status says.
 */
export function ReviewStateWords(props: { plugin: PluginSnapshot; review: ReviewEntry; actions: PluginActions }) {
  const [shown, setShown] = createSignal<{ runId: string; kinds: string[] | null } | null>(null);
  createEffect(() => (notSafeToApprove(props.review) ? props.review.run_id : null), (runId) => {
    if (runId === null) return;
    let current = true;
    runPluginAction(props.actions, props.plugin, ACTIONS.showReview, [runId])
      .then((stdout) => readFindingKinds(stdout, runId))
      .then((kinds) => {
        if (current) setShown({ runId, kinds });
      }, () => {});
    return () => {
      current = false;
    };
  });
  const words = () => {
    const known = shown();
    return known?.runId === props.review.run_id && notSafeToApprove(props.review) ? unsafeVerdict(props.review, known.kinds) : reviewState(props.review).words;
  };
  return <>{words()}</>;
}

/** "Review 2 · dc0c0bf · 3:12 PM": the version, the commit it reviewed, and when it ended or started. */
export function versionLabel(review: ReviewEntry, number: number): string {
  return [`Review ${number}`, shortSha(review.head_sha), shortDate(review.ended_at ?? review.started_at)].filter(Boolean).join(" · ");
}

/**
 * One review of a pull request: a header with its version and the commit it reviewed, a menu of the
 * pull request's other versions (an earlier one opens read-only, since only the newest can be
 * submitted), Rerun review (Retry review after a failure), Skip with Undo, and a banner when the
 * pull request has commits after the reviewed one. Below it, the plugin's page for a finished
 * review, which stays loaded while the user moves between finished reviews, or Prompting Time's
 * own screen for a review that runs, waits, or failed.
 */
export function ReviewScreen(props: Props) {
  const reviews = createMemo(() => readReviews(props.status?.value));
  const entry = createMemo(() => reviews().find((review) => review.run_id === props.runId) ?? null);
  const body = createMemo(() => {
    const outcome = entry()?.outcome;
    return outcome === "running" ? "running" : outcome === "failed" || outcome === "waiting" ? "problem" : "page";
  });
  const sameCause = (review: ReviewEntry) => {
    const cause = review.cause;
    if (!cause || cause.scope !== "all") return null;
    const group = blockingCauses(reviews(), readBlocked(readAgentStatus(props.status))).find((item) => item.id === cause.id);
    return { count: group?.reviews.length ?? 0, open: () => props.onOpenCause(cause.id) };
  };
  return (
    <div class="review-screen">
      {/* One header per run, so what Rerun or Skip printed stays with the run it ran for. */}
      <Show when={entry()?.run_id} keyed>
        {(runId: string) => (
          <ReviewHeader
            plugin={props.plugin}
            review={entry() ?? reviews().find((review) => review.run_id === runId)!}
            versions={versionsOf(reviews(), entry()!)}
            actions={props.actions}
            refresh={() => props.controls.refreshPlugins()}
            onOpenReview={props.onOpenReview}
            triage={props.triage}
          />
        )}
      </Show>
      <div class="review-body">
        <Switch>
          <Match when={body() === "page"}>
            <PluginFrame
              plugin={props.plugin}
              status={props.status}
              runId={props.runId}
              actions={props.actions}
              onPluginFilesChange={props.onPluginFilesChange}
              onShowReview={(runId) => {
                if (runId !== props.runId) props.onOpenReview(runId);
              }}
              onSubmitted={props.onSubmitted}
            />
          </Match>
          <Match when={body() === "problem"}>
            <Show when={entry()} keyed>
              {(review: ReviewEntry) => <FailedReview plugin={props.plugin} review={review} actions={props.actions} sameCause={sameCause(review)} />}
            </Show>
          </Match>
          <Match when={body() === "running" && entry()}>
            {(review) => (
              <div class="plugin-scroll">
                <div class="plugin-column">
                  <section class="plugin-section failed-review" aria-labelledby="running-review-heading">
                    <h3 id="running-review-heading"><StatusDot status="running" />Reviewing… started {shortDate(review().started_at)}</h3>
                    <p>{props.plugin.title} is reviewing {review().repo}#{review().pr}{review().head_sha ? ` at ${shortSha(review().head_sha)}` : ""}. Its findings open here when it finishes.</p>
                  </section>
                </div>
              </div>
            )}
          </Match>
        </Switch>
      </div>
    </div>
  );
}

type HeaderProps = {
  plugin: PluginSnapshot;
  review: ReviewEntry;
  /** Every run of the review's pull request, oldest first. */
  versions: readonly ReviewEntry[];
  actions: PluginActions;
  refresh(): Promise<void>;
  onOpenReview(runId: string): void;
  triage: ReviewTriage;
};

/** The review's header: its version and the others, Rerun review, Skip with Undo, Remove or Restore, and its banners. */
function ReviewHeader(props: HeaderProps) {
  const [busy, setBusy] = createSignal(false);
  const [queued, setQueued] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  // Whether the user skipped the review, until the plugin's status says so too.
  const [skippedHere, setSkippedHere] = createSignal<boolean | null>(null);
  let disposed = false;

  const number = () => props.versions.findIndex((review) => review.run_id === props.review.run_id) + 1;
  const newest = () => props.versions.at(-1) ?? props.review;
  const earlier = () => newest().run_id !== props.review.run_id;
  const running = () => props.review.outcome === "running";
  const problem = () => props.review.outcome === "failed" || props.review.outcome === "waiting";
  const skipped = () => skippedHere() ?? props.review.skipped_by_user;
  const canRerun = () => !earlier() && !running() && hasAction(props.plugin, ACTIONS.retry);
  const removed = () => !earlier() && props.review.removed_by_user;
  const canSkip = () => !earlier() && !running() && !removed() && props.review.submitted === null && hasAction(props.plugin, ACTIONS.skip);
  const canRemove = () => !earlier() && !removed() && hasAction(props.plugin, ACTIONS.remove);
  const newCommits = () => !earlier() && !running() && props.review.head_sha !== null && props.review.pr_head_sha !== null && props.review.head_sha !== props.review.pr_head_sha;
  const label = () => `${versionLabel(props.review, number())}${earlier() ? " · read-only" : ""}`;

  /** Runs one task at a time and shows its refusal; `done` runs after it unless the header is gone. */
  async function run<T>(task: () => Promise<T>, done: (result: T) => void) {
    setBusy(true);
    setError(null);
    try {
      const result = await task();
      if (!disposed) done(result);
    } catch (reason) {
      if (!disposed) setError(messageOf(reason));
    } finally {
      if (!disposed) setBusy(false);
    }
  }

  const act = (name: string, args: string[], done: (printed: string) => void) => run(async () => {
    const printed = await runPluginAction(props.actions, props.plugin, name, args);
    void props.refresh();
    return printed;
  }, done);

  const rerun = () => act(ACTIONS.retry, [props.review.run_id], (printed) => setQueued(printed || `${props.plugin.title} reviews ${shortPullRequest(props.review)} again in its next round.`));
  const skip = () => run(() => props.triage.skip(props.review.run_id), () => setSkippedHere(true));
  const unskip = () => act(ACTIONS.skip, [props.review.run_id, "--undo"], () => setSkippedHere(false));

  onSettled(() => () => {
    disposed = true;
  });

  return (
    <header class="review-header">
      <div class="review-header-bar">
        <Show when={props.versions.length > 1} fallback={<span class="review-version">{label()}</span>}>
          <Menu
            label="Versions of this review"
            text={label()}
            items={[...props.versions].reverse().map((review) => {
              const position = props.versions.indexOf(review) + 1;
              return { value: review.run_id, label: versionLabel(review, position), type: "radio" as const, checked: review.run_id === props.review.run_id };
            })}
            onSelect={(runId) => {
              if (runId !== props.review.run_id) props.onOpenReview(runId);
            }}
          />
        </Show>
        <span class="review-header-spacer" />
        <Show when={running() && !earlier()}>
          <span class="plugin-note">Reviewing… started {shortDate(props.review.started_at)}</span>
        </Show>
        <Show when={canSkip()}>
          <Show when={skipped()} fallback={<Button disabled={busy()} onClick={() => void skip()}>Skip</Button>}>
            <span class="plugin-note">Skipped</span>
            <Button disabled={busy()} onClick={() => void unskip()}>Undo</Button>
          </Show>
        </Show>
        <Show when={canRemove()}>
          <Button disabled={busy()} onClick={() => void run(() => props.triage.remove(props.review.run_id), () => {})}>Remove</Button>
        </Show>
        <Show when={canRerun()}>
          <Button variant={problem() || newCommits() ? "primary" : "plain"} disabled={busy()} onClick={() => void rerun()}>{problem() ? "Retry review" : "Rerun review"}</Button>
        </Show>
      </div>
      <Show when={removed()}>
        <div class="review-banner" role="note">
          <span><strong>Removed from your lists</strong> · its files stay on disk, and search still finds it.</span>
          <Show when={hasAction(props.plugin, ACTIONS.remove)}>
            <Button disabled={busy()} onClick={() => void run(() => props.triage.restore(props.review.run_id), () => {})}>Restore</Button>
          </Show>
        </div>
      </Show>
      <Show when={earlier()}>
        <div class="review-banner" role="note">
          <span>Review {number()} of {props.versions.length}, read-only: only the newest review can be submitted.</span>
          <Button onClick={() => props.onOpenReview(newest().run_id)}>Open review {props.versions.length}</Button>
        </div>
      </Show>
      <Show when={newCommits()}>
        <div class="review-banner" role="note">
          <span><strong>New commits since this review</strong> · it reviewed {shortSha(props.review.head_sha)}; the pull request is at {shortSha(props.review.pr_head_sha)}.</span>
          <Show when={canRerun()}>
            <Button variant="primary" disabled={busy()} onClick={() => void rerun()}>Rerun review</Button>
          </Show>
        </div>
      </Show>
      <Show when={queued()}>{(text) => <RunNow plugin={props.plugin} actions={props.actions} queued={text()} />}</Show>
      <Show when={error()}>{(text) => <p role="alert" class="inline-error plugin-refusal">{text()}</p>}</Show>
    </header>
  );
}
