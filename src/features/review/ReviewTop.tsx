import { createSignal, For, Show, untrack } from "solid-js";
import { Button, ChevronDownIcon, ChevronRightIcon, CloseIcon, IconButton, StatusDot, type Status } from "@prompting-time/ui";
import { notSafeToApprove, shortDate, shortSha, submittedText, unsafeVerdict, verdictWords, type ReviewEntry } from "../agents/reviews";
import { hideSince, sinceHidden, type LastLook } from "./memory";
import type { Commit, ShownReview } from "./shown";
import { viewHasData, viewName, type ReviewViews } from "./views";

export const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;
const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);
/** "at 3:12 PM" today, "on Nov 24" before. */
export const whenText = (iso: string) => `${shortDate(iso).includes(":") ? "at" : "on"} ${shortDate(iso)}`;

/**
 * The verdict banner's color, the one color mapping's: red for a review that failed or waits on a
 * cause, green for one the plugin judges safe to approve (or, when it doesn't say, that says approve
 * once CI passes and read the whole change), orange for the rest, which need a look first.
 */
export function verdictTone(review: ReviewEntry): Status {
  if (review.outcome === "failed" || review.outcome === "waiting") return "error";
  return (review.safe_to_approve ?? (review.verdict === "approve_once_ci_passes" && !review.skipped)) ? "success" : "attention";
}

/**
 * The verdict first in the review, as a banner tinted with its color: the verdict in words, such as
 * "Approve once CI passes" or, for one the plugin judges not safe to approve, why ("· has a
 * question"), then the reason, then the version, its commit, and its draft comments.
 */
export function VerdictBanner(props: { review: ReviewEntry; number: number; shown: ShownReview }) {
  const kinds = () => props.shown.findings.map((finding) => finding.kind);
  const words = () => capital(notSafeToApprove(props.review) ? unsafeVerdict(props.review, kinds()) : props.review.skipped ? "skipped, look first" : verdictWords(props.review.verdict));
  const reason = () => props.shown.overview?.verdict_reason || props.shown.reason;
  const meta = () => [
    `Review ${props.number}`,
    shortSha(props.review.head_sha),
    plural(props.shown.findings.length, "draft comment"),
    props.review.submitted ? submittedText(props.review.submitted) : null,
  ].filter(Boolean).join(" · ");
  return (
    <section class="verdict-banner" data-tone={verdictTone(props.review)} aria-label="Verdict">
      <p class="verdict-banner-head">
        <StatusDot status={verdictTone(props.review)} />
        <span class="verdict-banner-words">{words()}</span>
        <span class="verdict-banner-meta">{meta()}</span>
      </p>
      <Show when={reason()}>{(text) => <p class="verdict-banner-reason">{text()}</p>}</Show>
    </section>
  );
}

/** The commits after `head` in the list, or every one when `head` isn't in it. */
function commitsAfter(commits: readonly Commit[], head: string | null): Commit[] {
  const index = head ? commits.findIndex((commit) => commit.sha.startsWith(head) || head.startsWith(commit.sha)) : -1;
  return index < 0 ? [...commits] : commits.slice(index + 1);
}

type NoticeProps = {
  review: ReviewEntry;
  /** Every version of the review's pull request, oldest first. */
  versions: readonly ReviewEntry[];
  commits: readonly Commit[];
  /** What the user opened of this pull request before this time, if anything. */
  look: LastLook | null;
  canRerun: boolean;
  busy: boolean;
  onRerun(): void;
  onHide(): void;
};

/**
 * "Changes since you last looked" as one line under the verdict: how many commits the pull request
 * got after the reviewed one (and how many of them since you last opened it), Rerun review, and ×,
 * which hides the line for this version of the pull request. It starts folded; the line opens what
 * you last read and the commits.
 */
function SinceNotice(props: NoticeProps) {
  const [open, setOpen] = createSignal(false);
  const sinceLook = () => (props.look ? commitsAfter(props.commits, props.look.head).length : props.commits.length);
  const fresh = (commit: Commit) => props.look !== null && commitsAfter(props.commits, props.look.head).includes(commit);
  const earlier = () => (props.look && props.look.run !== props.review.run_id ? props.versions.find((version) => version.run_id === props.look!.run) ?? null : null);
  const looked = () => {
    const look = props.look;
    if (!look) return "You haven't opened this pull request's review before.";
    if (look.run === props.review.run_id) return `You last opened this review ${whenText(look.at)}.`;
    const before = earlier();
    if (!before) return `You last opened an earlier review ${whenText(look.at)}.`;
    return `You last opened review ${props.versions.indexOf(before) + 1} (${shortSha(before.head_sha)}) ${whenText(look.at)}, which said ${verdictWords(before.verdict)} with ${plural(before.findings, "finding")}.`;
  };
  return (
    <section class="since-notice" data-open={open() ? "" : undefined} aria-label={viewName("since")}>
      <div class="since-notice-line">
        <button type="button" class="since-notice-toggle" aria-expanded={open() ? "true" : "false"} aria-controls={`since-${props.review.run_id}`} onClick={() => setOpen(!open())}>
          {open() ? <ChevronDownIcon /> : <ChevronRightIcon />}
          <span>
            {plural(props.commits.length, "new commit")} since this review
            <Show when={sinceLook() > 0 && sinceLook() < props.commits.length}> · {sinceLook()} since you last looked</Show>
          </span>
        </button>
        <Show when={props.canRerun}>
          <Button disabled={props.busy} onClick={() => props.onRerun()}>Rerun review</Button>
        </Show>
        <IconButton label="Hide until the pull request gets new commits" onClick={() => props.onHide()}><CloseIcon /></IconButton>
      </div>
      <Show when={open()}>
        <div class="since-notice-detail" id={`since-${props.review.run_id}`}>
          <p>{looked()} The review read {shortSha(props.review.head_sha) ?? "an earlier commit"}; none of these commits.</p>
          <ul class="since-commits">
            <For each={props.commits}>
              {(commit) => (
                <li>
                  <code>{commit.sha.slice(0, 7)}</code>
                  <span class="since-commit-title">{commit.title}</span>
                  <span class="since-commit-meta">
                    {[commit.author ? `@${commit.author}` : null, shortDate(commit.at), plural(commit.files.length, "file")].filter(Boolean).join(" · ")}
                    <Show when={fresh(commit)}> · new since you last looked</Show>
                  </span>
                </li>
              )}
            </For>
          </ul>
        </div>
      </Show>
    </section>
  );
}

type TopProps = Omit<NoticeProps, "commits" | "onHide"> & {
  number: number;
  shown: ShownReview;
  views: ReviewViews;
  /** Says what × did, with Undo. */
  onNotice(text: string, undo: () => void): void;
};

/** The top of a review with an overview, above the walkthrough and the diff alike: the verdict banner, then the since notice. */
export function ReviewTop(props: TopProps) {
  const [hidden, setHidden] = createSignal(untrack(() => sinceHidden(props.review)));
  const commits = () => (props.views.isOn("since") && viewHasData("since", props.shown) && !hidden() ? props.shown.commitsSinceReview ?? [] : []);
  const hide = (value: boolean) => {
    hideSince(props.review, value);
    setHidden(value);
  };
  return (
    <div class="review-top">
      <VerdictBanner review={props.review} number={props.number} shown={props.shown} />
      <Show when={commits().length > 0}>
        <SinceNotice
          review={props.review}
          versions={props.versions}
          commits={commits()}
          look={props.look}
          canRerun={props.canRerun}
          busy={props.busy}
          onRerun={props.onRerun}
          onHide={() => {
            hide(true);
            props.onNotice("Hidden until the pull request gets new commits.", () => hide(false));
          }}
        />
      </Show>
    </div>
  );
}
