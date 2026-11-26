import { createMemo, createSignal, For, Show } from "solid-js";
import { Button, StatusDot, type Status } from "@prompting-time/ui";
import { shortDate, shortPullRequest, verdictWords, type ReviewEntry } from "./reviews";
import type { Round } from "./status";

type Filter = "all" | "failed" | "skipped";
const FILTERS: Array<{ value: Filter; label: string }> = [
  { value: "all", label: "All" },
  { value: "failed", label: "Failed" },
  { value: "skipped", label: "Skipped" },
];

/** One line of the history: a round, or one run of a review. */
type Entry = { key: string; at: string; runId: string | null; name: string; result: string; dot: Status; detail: string; kind: Filter | null };

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;

function roundEntry(round: Round): Entry {
  const failed = round.result === "poll_failed" || round.result === "interrupted";
  const result = { done: "Done", poll_failed: "Poll failed", interrupted: "Interrupted", skipped: "Skipped" }[round.result];
  const detail = round.reason ?? (round.selected === null ? "" : `${plural(round.selected, "review")} selected`);
  return { key: `round:${round.started_at}`, at: round.started_at, runId: null, name: "Poll", result, dot: failed ? "error" : round.result === "done" ? "success" : "idle", detail, kind: failed ? "failed" : round.result === "skipped" ? "skipped" : null };
}

function reviewEntry(review: ReviewEntry): Entry {
  const base = { key: `review:${review.run_id}`, at: review.started_at, runId: review.run_id, name: `Review ${shortPullRequest(review)}` };
  if (review.skipped_by_user) return { ...base, result: "Skipped", dot: "idle", detail: "skipped by you", kind: "skipped" };
  switch (review.outcome) {
    case "running": return { ...base, result: "Running", dot: "running", detail: "running now", kind: null };
    case "waiting": return { ...base, result: "Waiting", dot: "idle", detail: review.cause?.summary ?? "", kind: null };
    case "failed": return { ...base, result: "Failed", dot: "error", detail: review.cause?.summary ?? "no reason recorded", kind: "failed" };
    case "reviewed": return {
      ...base, result: review.skipped ? "Skipped" : "Reviewed", dot: review.skipped ? "idle" : "success",
      detail: `${verdictWords(review.verdict)} · ${plural(review.findings, "finding")}`, kind: review.skipped ? "skipped" : null,
    };
  }
}

/** "3:33 PM" today, "Nov 24 3:33 PM" before. */
function when(iso: string): string {
  const time = new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const day = shortDate(iso);
  return day === time ? time : `${day} ${time}`;
}

/**
 * The agent's history, newest first: its rounds and every run of its reviews, each with its result
 * and a reason, filtered to all, failed, or skipped. A review's run opens that version of the review.
 */
export function AgentRuns(props: { rounds: readonly Round[]; reviews: readonly ReviewEntry[]; onOpenReview(runId: string): void }) {
  const [filter, setFilter] = createSignal<Filter>("all");
  const entries = createMemo(() => [...props.rounds.map(roundEntry), ...props.reviews.map(reviewEntry)].sort((left, right) => right.at.localeCompare(left.at)));
  const shown = () => entries().filter((entry) => filter() === "all" || entry.kind === filter());
  return (
    <div class="agent-panel agent-runs">
      <div class="agent-filters" role="group" aria-label="Show runs">
        <For each={FILTERS}>
          {(item) => <Button aria-pressed={filter() === item.value ? "true" : "false"} onClick={() => setFilter(item.value)}>{item.label}</Button>}
        </For>
      </div>
      <Show when={shown().length > 0} fallback={<p class="work-empty">{filter() === "all" ? "No rounds or reviews yet." : `No ${filter()} rounds or reviews.`}</p>}>
        <table class="agent-table">
          <thead>
            <tr><th scope="col">Time</th><th scope="col">Run</th><th scope="col">Result</th><th scope="col">Detail</th></tr>
          </thead>
          <tbody>
            <For each={shown()} keyed={(entry) => entry.key}>
              {(entry) => (
                <tr>
                  <td class="agent-muted">{when(entry().at)}</td>
                  <td>
                    <Show when={entry().runId} fallback={entry().name}>
                      {(runId) => <button type="button" class="agent-link" onClick={() => props.onOpenReview(runId())}>{entry().name}</button>}
                    </Show>
                  </td>
                  <td><span class="agent-result"><StatusDot status={entry().dot} />{entry().result}</span></td>
                  <td class="agent-muted">{entry().detail}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </div>
  );
}
