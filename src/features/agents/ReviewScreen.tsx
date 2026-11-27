import { createEffect, createMemo, createSignal, Match, onSettled, Show, Switch, untrack } from "solid-js";
import { Portal, type JSX } from "@solidjs/web";
import { Button, EllipsisIcon, Menu, StatusDot, type MenuItem } from "@prompting-time/ui";
import type { AgentControls, PluginActions, PluginFilesChange, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { PluginFrame } from "../plugins/PluginFrame";
import { ACTIONS, hasAction, messageOf, runPluginAction } from "./actions";
import { FailedReview } from "./FailedReview";
import { RunNow } from "./RunNow";
import { blockingCauses, notSafeToApprove, readFindingKinds, readReviews, reviewState, shortDate, shortPullRequest, shortSha, unsafeVerdict, versionsOf, type ReviewEntry } from "./reviews";
import { readAgentStatus, readBlocked } from "./status";
import { ConnectionsView } from "../review/Connections";
import { rememberLook, type LastLook } from "../review/memory";
import { ReviewTop } from "../review/ReviewTop";
import { readShownReview, type Overview, type ShownReview } from "../review/shown";
import { REVIEW_VIEWS, viewHasData, viewListed, type ReviewViews } from "../review/views";
import { Walkthrough } from "../review/Walkthrough";

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
  /** The plugin posted the user's review of this run, from its page or the walkthrough. */
  onSubmitted(runId: string): void;
  /** The views of a review the user turned on, for every review. */
  views: ReviewViews;
  /** The window's header row, where the review's controls go; without one they get a row of their own. */
  headerSlot?: HTMLElement | null;
  /** Says what an action did at the bottom of the window, with Undo. */
  onNotice(text: string, undo: () => void): void;
};

/** What a review's header asks of the app for its run: skip it, take it out of the lists, or put it back. */
export type ReviewTriage = { skip(runId: string): Promise<void>; remove(runId: string): Promise<void>; restore(runId: string): Promise<void> };

/**
 * The open review's state words in the window's header, after the dot its row would have, such as
 * "approve once CI passes" in green. For a review that says approve once CI passes but that the
 * plugin judges not safe to approve, they say why, in orange, from the kinds of its findings, which
 * show-review prints: "approve once CI passes · has a question". Until show-review answers, or when
 * it fails, they say what the status says.
 */
export function ReviewStateWords(props: { plugin: PluginSnapshot; review: ReviewEntry; actions: PluginActions }) {
  const [shown, setShown] = createSignal<{ runId: string; kinds: string[] | null } | null>(null);
  createEffect(() => (notSafeToApprove(props.review) ? props.review.run_id : null), (runId) => {
    if (runId === null) return;
    let current = true;
    // Only a new run reads again; the plugin's actions are read as they are now.
    untrack(() => runPluginAction(props.actions, props.plugin, ACTIONS.showReview, [runId]))
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
  // The dot follows the status, which the kinds only explain.
  const status = () => reviewState(props.review).status;
  return (
    <>
      <Show when={status() !== "idle"}>
        <StatusDot status={status()} />
      </Show>
      {words()}
    </>
  );
}

/** "Review 2 · dc0c0bf · 3:12 PM": the version, the commit it reviewed, and when it ended or started. */
export function versionLabel(review: ReviewEntry, number: number): string {
  return [`Review ${number}`, shortSha(review.head_sha), shortDate(review.ended_at ?? review.started_at)].filter(Boolean).join(" · ");
}

/** What show-review printed for the open run, or why it couldn't be read. */
type ShownRead = { runId: string; shown: ShownReview | null; error: string | null };
/** What a review with an overview shows: its walkthrough, the plugin's page (Diff), or the map of how the changed code connects in the walkthrough's place. */
type ReviewMode = "walkthrough" | "diff" | "connections";

/**
 * One review of a pull request. Its controls sit in the window's header row: a ⋯ menu with the pull
 * request's versions (an earlier one opens read-only, since only the newest can be submitted), Rerun
 * review, Skip, and Remove; Retry review after a failure; and, for a review the plugin wrote an
 * overview of, Walkthrough | Diff and the Views menu. Below, banners say when the review is removed
 * or an earlier version. A finished review with an overview opens on Prompting Time's walkthrough,
 * headed by the verdict banner and the since notice, which head its Diff too; its Diff, and a
 * finished review without an overview, is the plugin's page, which stays loaded while the user
 * moves between finished reviews. A review that runs, waits, or failed gets Prompting Time's own
 * screen.
 */
export function ReviewScreen(props: Props) {
  const reviews = createMemo(() => readReviews(props.status?.value));
  const entry = createMemo(() => reviews().find((review) => review.run_id === props.runId) ?? null);
  const body = createMemo(() => {
    const outcome = entry()?.outcome;
    return outcome === "running" ? "running" : outcome === "failed" || outcome === "waiting" ? "problem" : "page";
  });
  const [read, setRead] = createSignal<ShownRead | null>(null);
  // What each run shows; a review opens on its walkthrough.
  const [modes, setModes] = createSignal<Readonly<Record<string, ReviewMode>>>({});
  // What the user opened of each run's pull request before, read once as the run opens.
  const looks = new Map<string, LastLook | null>();
  const finished = () => (body() === "page" ? props.runId : null);
  // Set once the screen is gone, so a late answer reads none of its props.
  let disposed = false;
  onSettled(() => () => {
    disposed = true;
  });

  async function load(runId: string) {
    let read: ShownRead;
    try {
      const plugin = untrack(() => props.plugin);
      read = { runId, shown: readShownReview(await runPluginAction(untrack(() => props.actions), plugin, ACTIONS.showReview, [runId]), runId), error: null };
    } catch (reason) {
      read = { runId, shown: null, error: messageOf(reason) };
    }
    if (disposed || untrack(finished) !== runId) return;
    // A failed read again keeps what an earlier read of the same run showed.
    setRead((now) => (read.shown === null && now?.runId === runId && now.shown ? now : read));
  }
  createEffect(finished, (runId) => {
    if (runId) void load(runId);
  });
  onSettled(() => props.onPluginFilesChange((change) => {
    const runId = untrack(finished);
    if (runId && change.plugin === untrack(() => props.plugin.name) && (change.files === null || change.files.some((file) => file.includes(runId)))) void load(runId);
  }));

  const shownFor = (runId: string) => {
    const now = read();
    return now?.runId === runId ? now : null;
  };
  /** The run's overview when the plugin wrote one, with what show-review printed. */
  const withOverview = (runId: string) => {
    const shown = shownFor(runId)?.shown;
    return shown?.overview ? (shown as ShownReview & { overview: Overview }) : null;
  };
  const mode = (runId: string) => modes()[runId] ?? "walkthrough";
  const setMode = (runId: string, next: ReviewMode) => setModes((now) => ({ ...now, [runId]: next }));
  /** The page shows once show-review answered: for a review without an overview, or in its Diff. */
  const pageShown = () => {
    const runId = finished();
    return runId !== null && shownFor(runId) !== null && (!withOverview(runId) || mode(runId) === "diff");
  };
  const sameCause = (review: ReviewEntry) => {
    const cause = review.cause;
    if (!cause || cause.scope !== "all") return null;
    const group = blockingCauses(reviews(), readBlocked(readAgentStatus(props.status))).find((item) => item.id === cause.id);
    return { count: group?.reviews.length ?? 0, open: () => props.onOpenCause(cause.id) };
  };
  return (
    <div class="review-screen">
      {/* One part per run, so what Rerun or Skip printed stays with the run it ran for. */}
      <Show when={entry()?.run_id} keyed>
        {(runId: string) => {
          const review = () => entry() ?? reviews().find((item) => item.run_id === runId)!;
          if (!looks.has(runId)) looks.set(runId, rememberLook(untrack(review)));
          return (
            <ReviewRun
              plugin={props.plugin}
              review={review()}
              versions={versionsOf(reviews(), review())}
              actions={props.actions}
              refresh={() => props.controls.refreshPlugins()}
              onOpenReview={props.onOpenReview}
              triage={props.triage}
              headerSlot={props.headerSlot}
              views={props.views}
              shown={withOverview(runId)}
              look={looks.get(runId) ?? null}
              mode={mode(runId)}
              onMode={(next) => setMode(runId, next)}
              pageShown={pageShown()}
              onNotice={props.onNotice}
              onSubmitted={props.onSubmitted}
            />
          );
        }}
      </Show>
      <div class="review-body">
        <Switch>
          <Match when={body() === "page"}>
            <Show when={finished() && !shownFor(finished()!)}>
              <p role="status" class="plugin-note review-reading">Reading the review…</p>
            </Show>
            <div class="review-page" hidden={!pageShown()}>
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
            </div>
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

type RunProps = {
  plugin: PluginSnapshot;
  review: ReviewEntry;
  /** Every run of the review's pull request, oldest first. */
  versions: readonly ReviewEntry[];
  actions: PluginActions;
  refresh(): Promise<void>;
  onOpenReview(runId: string): void;
  triage: ReviewTriage;
  headerSlot?: HTMLElement | null;
  views: ReviewViews;
  /** What show-review printed, when the plugin wrote an overview of the run. */
  shown: (ShownReview & { overview: Overview }) | null;
  look: LastLook | null;
  mode: ReviewMode;
  onMode(mode: ReviewMode): void;
  /** Whether the plugin's page shows under this run's part. */
  pageShown: boolean;
  onNotice(text: string, undo: () => void): void;
  onSubmitted(runId: string): void;
};

/** One run's controls in the header row, its banners, and its walkthrough, or its top above the page in Diff. */
function ReviewRun(props: RunProps) {
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
  const canRemove = () => !earlier() && hasAction(props.plugin, ACTIONS.remove);
  const newCommits = () => !earlier() && !running() && props.review.head_sha !== null && props.review.pr_head_sha !== null && props.review.head_sha !== props.review.pr_head_sha;

  /** Runs one task at a time and shows its refusal; `done` runs after it unless the run's part is gone. */
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

  /** The ⋯ menu: the pull request's versions, then Rerun review, Skip, and Remove or Restore. */
  const moreItems = (): MenuItem[] => [
    ...(props.versions.length > 1
      ? [...props.versions].reverse().map((review) => ({ value: `version:${review.run_id}`, label: versionLabel(review, props.versions.indexOf(review) + 1), type: "radio" as const, checked: review.run_id === props.review.run_id }))
      : []),
    ...(canRerun() && !problem() ? [{ value: "rerun", label: "Rerun review", disabled: busy() }] : []),
    ...(canSkip() && !skipped() ? [{ value: "skip", label: "Skip", disabled: busy() }] : []),
    ...(canRemove() ? [removed() ? { value: "restore", label: "Restore", disabled: busy() } : { value: "remove", label: "Remove", disabled: busy() }] : []),
  ];
  function choose(value: string) {
    if (value.startsWith("version:")) {
      const runId = value.slice("version:".length);
      if (runId !== props.review.run_id) props.onOpenReview(runId);
    } else if (value === "rerun") void rerun();
    else if (value === "skip") void skip();
    else if (value === "remove") void run(() => props.triage.remove(props.review.run_id), () => {});
    else if (value === "restore") void run(() => props.triage.restore(props.review.run_id), () => {});
  }
  const viewItems = (): MenuItem[] => REVIEW_VIEWS.filter((view) => (props.shown ? viewListed(view, props.shown) : view.ready)).map((view) => ({
    value: view.id,
    label: view.name,
    description: props.shown && !viewHasData(view.id, props.shown) ? `${view.description} Nothing for this review.` : view.description,
    type: "checkbox" as const,
    checked: props.views.isOn(view.id),
  }));
  /** How many of the views this review's menu lists are off, for the menu's button. */
  const viewsOff = () => viewItems().filter((item) => !item.checked).length;
  /** The map of how the changed code connects, while it shows in the walkthrough's place. */
  const map = () => (props.mode === "connections" && props.views.isOn("connections") ? props.shown?.connections ?? null : null);
  // Turning the view off, here or in the palette, or a reread without the map, goes back to the walkthrough.
  createEffect(() => props.mode === "connections" && map() === null, (gone) => {
    if (gone) untrack(() => props.onMode("walkthrough"));
  });

  const controls = (
    <div class="review-controls">
      <Show when={skipped() && canSkip()}>
        <span class="plugin-note">Skipped</span>
        <Button disabled={busy()} onClick={() => void unskip()}>Undo</Button>
      </Show>
      <Show when={problem() && canRerun()}>
        <Button variant="primary" disabled={busy()} onClick={() => void rerun()}>Retry review</Button>
      </Show>
      <Show when={moreItems().length > 0}>
        <Menu label="More review actions" icon={<EllipsisIcon />} items={moreItems()} onSelect={choose} />
      </Show>
      <Show when={props.shown}>
        <div class="review-modes" role="group" aria-label="Show the review as">
          <button type="button" class="pt-button" aria-pressed={props.mode === "diff" ? "false" : "true"} onClick={() => props.onMode("walkthrough")}>Walkthrough</button>
          <button type="button" class="pt-button" aria-pressed={props.mode === "diff" ? "true" : "false"} onClick={() => props.onMode("diff")}>Diff</button>
        </div>
        <Menu
          label="Views in reviews"
          text={viewsOff() > 0 ? `Views · ${viewsOff()} off` : "Views"}
          items={viewItems()}
          closeOnSelect={false}
          onSelect={(id) => {
            const view = REVIEW_VIEWS.find((item) => item.id === id);
            if (view) props.views.setOn(view.id, !props.views.isOn(view.id));
          }}
        />
      </Show>
    </div>
  );

  const top = (shown: ShownReview & { overview: Overview }): JSX.Element => (
    <ReviewTop
      review={props.review}
      number={number()}
      versions={props.versions}
      shown={shown}
      views={props.views}
      look={props.look}
      canRerun={canRerun()}
      busy={busy()}
      onRerun={() => void rerun()}
      onNotice={props.onNotice}
    />
  );

  return (
    <>
      <Show when={props.headerSlot} fallback={<div class="review-header-bar">{controls}</div>}>
        {(slot) => <Portal mount={slot()}>{controls}</Portal>}
      </Show>
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
      {/* A review with an overview says this in its since notice instead. */}
      <Show when={newCommits() && !props.shown}>
        <div class="review-banner" role="note">
          <span><strong>New commits since this review</strong> · it reviewed {shortSha(props.review.head_sha)}; the pull request is at {shortSha(props.review.pr_head_sha)}.</span>
          <Show when={canRerun()}>
            <Button variant="primary" disabled={busy()} onClick={() => void rerun()}>Rerun review</Button>
          </Show>
        </div>
      </Show>
      <Show when={queued()}>{(text) => <RunNow plugin={props.plugin} actions={props.actions} queued={text()} />}</Show>
      <Show when={error()}>{(text) => <p role="alert" class="inline-error plugin-refusal">{text()}</p>}</Show>
      <Show when={props.shown}>
        {(shown) => (
          <Show when={props.mode !== "diff"} fallback={<Show when={props.pageShown}><div class="review-page-top">{top(shown())}</div></Show>}>
            <Walkthrough
              plugin={props.plugin}
              review={props.review}
              shown={shown()}
              views={props.views}
              actions={props.actions}
              top={top(shown())}
              active={map() === null}
              canDecide={!earlier() && props.review.submitted === null && !removed()}
              onShowDiff={() => props.onMode("diff")}
              onOpenConnections={() => props.onMode("connections")}
              onSubmitted={props.onSubmitted}
            />
            <Show when={map()}>
              {(connections) => <ConnectionsView connections={connections()} patch={shown().patch} onBack={() => props.onMode("walkthrough")} />}
            </Show>
          </Show>
        )}
      </Show>
    </>
  );
}
