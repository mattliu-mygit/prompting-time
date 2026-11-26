import { createMemo, For, Match, Show, Switch } from "solid-js";
import { BotIcon, Button, EllipsisIcon, Menu, StatusDot, Tabs } from "@prompting-time/ui";
import type { AgentControls, PluginActions, PluginFilesChange, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { AgentLog } from "./AgentLog";
import { agentControls, agentMenuItems, type AgentAction, type AgentTab } from "./agentMenu";
import { AgentRuns } from "./AgentRuns";
import { PluginSettings } from "./PluginSettings";
import { newestRuns, readReviews, readyToDecide, shortDate } from "./reviews";
import { describeAgent, readAgentStatus, readRounds, type Round } from "./status";

const TABS: Array<{ value: AgentTab; label: string }> = [
  { value: "runs", label: "Runs" },
  { value: "logs", label: "Logs" },
  { value: "settings", label: "Settings" },
];
const DAYS = 15;
const ROUND_WORDS: Record<Round["result"], string> = { done: "done", poll_failed: "poll failed", interrupted: "interrupted", skipped: "skipped" };

type Props = {
  plugin: PluginSnapshot;
  status: PluginStatusRead | undefined;
  tab: AgentTab;
  onTab(tab: AgentTab): void;
  /** The control that runs now for this agent, and why the last one failed, in the plugin's words. */
  pending: AgentAction | null;
  error: string | null;
  /** Runs a control; App asks before it stops the agent. */
  onAction(action: AgentAction): void;
  actions: PluginActions;
  controls: Pick<AgentControls, "readPluginLog">;
  onPluginFilesChange(listener: (change: PluginFilesChange) => void): () => void;
  onOpenReview(runId: string): void;
};

const sameDay = (iso: string, day: Date) => new Date(iso).toDateString() === day.toDateString();

/**
 * A background agent's screen: its status in one sentence, Poll now and Pause or Resume, a menu
 * with Stop or Start, Settings, and Logs, its health (the last poll, today's counts, and its rounds
 * per day for 15 days), and the tabs Runs (its history), Logs, and Settings.
 */
export function AgentScreen(props: Props) {
  const row = () => describeAgent(props.plugin, props.status);
  const status = () => readAgentStatus(props.status);
  const controls = () => agentControls(props.plugin, props.status, props.pending);
  return (
    <section class="agent-screen" aria-labelledby="agent-title">
      <header class="agent-header">
        <span class="agent-icon" aria-hidden="true"><BotIcon /></span>
        <div class="agent-heading">
          <p class="eyebrow">Background agent</p>
          <h2 id="agent-title">{props.plugin.title}</h2>
          <p class="agent-status"><StatusDot status={row().status} /><span>{row().meta}</span></p>
          <Show when={status()?.paused && props.plugin.process.kind !== "notRunning"}>
            <p class="plugin-note">Reviews in progress finish, and no poll starts until you resume. Reopening Prompting Time resumes {props.plugin.title}.</p>
          </Show>
        </div>
        <div class="agent-actions">
          <For each={[controls().poll, controls().pause].filter((control) => control !== null)}>
            {(control) => <Button disabled={control.disabled} onClick={() => props.onAction(control.action)}>{control.label}</Button>}
          </For>
          <Menu
            label={`More ${props.plugin.title} actions`}
            icon={<EllipsisIcon />}
            items={agentMenuItems(props.plugin, props.status, props.pending, { poll: false, pause: false, power: true, logsLabel: "Logs" })}
            onSelect={(value) => props.onAction(value as AgentAction)}
          />
        </div>
      </header>
      <Show when={props.error}>{(error) => <p role="alert" class="inline-error plugin-refusal agent-error">{error()}</p>}</Show>
      <HealthStrip plugin={props.plugin} status={props.status} />
      <Tabs class="agent-tabs" label={props.plugin.title} tabs={TABS} value={props.tab} onChange={(value) => props.onTab(value as AgentTab)}>
        <Switch>
          <Match when={props.tab === "runs"}>
            <AgentRuns rounds={readRounds(status())} reviews={readReviews(props.status?.value)} onOpenReview={props.onOpenReview} />
          </Match>
          <Match when={props.tab === "logs"}>
            <AgentLog plugin={props.plugin} readLog={props.controls.readPluginLog} onPluginFilesChange={props.onPluginFilesChange} />
          </Match>
          <Match when={props.tab === "settings"}>
            <PluginSettings plugin={props.plugin} status={props.status} actions={props.actions} />
          </Match>
        </Switch>
      </Tabs>
    </section>
  );
}

/** The last poll, today's counts, and the rounds of each of the last 15 days, as small bars. */
function HealthStrip(props: { plugin: PluginSnapshot; status: PluginStatusRead | undefined }) {
  const today = new Date();
  const status = () => readAgentStatus(props.status);
  const rounds = createMemo(() => readRounds(status()));
  const reviews = createMemo(() => readReviews(props.status?.value));
  const lastPoll = () => {
    const round = rounds()[0];
    if (round) return { at: round.ended_at ?? round.started_at, result: round.result };
    const last = status()?.last_round;
    return last ? { at: last.ended_at, result: last.result } : null;
  };
  const counts = createMemo(() => ({
    reviewed: reviews().filter((review) => review.outcome === "reviewed" && review.ended_at !== null && sameDay(review.ended_at, today)).length,
    deciding: newestRuns(reviews()).filter(readyToDecide).length,
    failed: reviews().filter((review) => review.outcome === "failed" && sameDay(review.ended_at ?? review.started_at, today)).length,
    waiting: newestRuns(reviews()).filter((review) => review.outcome === "waiting").length,
  }));
  const days = createMemo(() => Array.from({ length: DAYS }, (_, index) => {
    const day = new Date(today);
    day.setDate(day.getDate() - (DAYS - 1 - index));
    const ofDay = rounds().filter((round) => sameDay(round.started_at, day));
    return {
      key: day.toDateString(),
      label: day.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" }),
      done: ofDay.filter((round) => round.result === "done").length,
      failed: ofDay.filter((round) => round.result === "poll_failed" || round.result === "interrupted").length,
      skipped: ofDay.filter((round) => round.result === "skipped").length,
    };
  }));
  const highest = () => Math.max(1, ...days().map((day) => day.done + day.failed + day.skipped));
  const summary = () => {
    const last = days().at(-1)!;
    return `Rounds per day for ${DAYS} days; today ${last.done} done, ${last.failed} failed, ${last.skipped} skipped`;
  };
  return (
    <div class="agent-health">
      <div class="agent-health-cell">
        <small>Last poll</small>
        <Show when={lastPoll()} fallback={<strong>None yet</strong>}>
          {(poll) => <strong>{shortDate(poll().at)} · {ROUND_WORDS[poll().result]}</strong>}
        </Show>
        <span>{status()?.next_round ? `next ${shortDate(status()!.next_round!)}` : describeAgent(props.plugin, props.status).meta}</span>
      </div>
      <div class="agent-health-cell">
        <small>Today</small>
        <strong>{counts().reviewed} reviewed · {counts().deciding} waiting on you</strong>
        <span data-bad={counts().failed > 0 || counts().waiting > 0 ? "true" : undefined}>
          {counts().failed} failed{counts().waiting > 0 ? ` · ${counts().waiting} blocked` : ""}
        </span>
      </div>
      <div class="agent-health-cell agent-health-chart">
        <small>Last {DAYS} days</small>
        <div class="agent-bars" role="img" aria-label={summary()}>
          <For each={days()} keyed={(day) => day.key}>
            {(day) => (
              <span class="agent-bar" title={`${day().label}: ${day().done} done, ${day().failed} failed, ${day().skipped} skipped`}>
                <span class="agent-bar-skipped" style={{ "block-size": `${(day().skipped / highest()) * 100}%` }} />
                <span class="agent-bar-failed" style={{ "block-size": `${(day().failed / highest()) * 100}%` }} />
                <span class="agent-bar-done" style={{ "block-size": `${(day().done / highest()) * 100}%` }} />
              </span>
            )}
          </For>
        </div>
      </div>
    </div>
  );
}
