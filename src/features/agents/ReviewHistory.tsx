import { For, Show } from "solid-js";
import { ListRow, PullRequestIcon } from "@prompting-time/ui";
import type { PluginSnapshot } from "../../bridge/types";
import type { PluginStatusRead } from "../../app/store";
import { pastReviews, readReviews, reviewState, reviewTime, reviewTitle, shortDate, shortPullRequest } from "./reviews";

type Props = {
  plugin: PluginSnapshot;
  status: PluginStatusRead | undefined;
  onOpen(runId: string): void;
};

/**
 * A plugin's past reviews, newest first, from the reviews its status lists: each with its pull
 * request, how it ended or what the user submitted, and when. A row opens the review in the
 * plugin's page, with its diff, findings, and what was submitted; a failed review opens on
 * Prompting Time's own screen for it.
 */
export function ReviewHistory(props: Props) {
  const reviews = () => pastReviews(readReviews(props.status?.value));
  return (
    <section class="plugin-scroll" aria-labelledby="review-history-title">
      <div class="plugin-column">
        <header class="plugin-heading">
          <p class="eyebrow">{props.plugin.title}</p>
          <h2 id="review-history-title">Past reviews</h2>
          <p>The last 30 days, newest first. Open a review to see its diff, its findings, and what you submitted, or why it failed.</p>
        </header>
        <Show when={props.status?.error}>{(error) => <p role="alert" class="inline-error">{error()}</p>}</Show>
        <Show when={!props.status}>
          <p class="work-empty">{props.plugin.title} lists its reviews while it runs.</p>
        </Show>
        <Show when={props.status?.value && reviews().length === 0}>
          <p class="work-empty">No reviews ended in the last 30 days.</p>
        </Show>
        <ul class="work-rows review-history">
          <For each={reviews()} keyed={(review) => review.run_id}>
            {(review) => {
              const state = () => reviewState(review());
              return (
                <li>
                  <ListRow
                    icon={<PullRequestIcon />}
                    title={reviewTitle(review())}
                    count={review().findings > 0 ? review().findings : undefined}
                    status={state().status}
                    meta={`${shortPullRequest(review())} · ${state().words} · ${shortDate(reviewTime(review()))}`}
                    onClick={() => props.onOpen(review().run_id)}
                  />
                </li>
              );
            }}
          </For>
        </ul>
      </div>
    </section>
  );
}
