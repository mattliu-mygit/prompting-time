import { For, Match, Show, Switch } from "solid-js";
import { BotIcon, Button, EllipsisIcon, Menu, StatusDot, Tabs } from "@prompting-time/ui";
import type { AgentControls, PluginActions, PluginFilesChange, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { AgentLog } from "./AgentLog";
import { agentControls, agentMenuItems, type AgentAction, type AgentTab } from "./agentMenu";
import { PluginSettings } from "./PluginSettings";
import { describeAgent } from "./status";

const TABS: Array<{ value: AgentTab; label: string }> = [
  { value: "logs", label: "Logs" },
  { value: "settings", label: "Settings" },
];

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
};

/**
 * A background agent's screen: its status in one sentence, Poll now and Pause or Resume, a menu
 * with Stop or Start, Settings, and Logs, and the tabs Logs and Settings. What the plugin does,
 * such as its history of jobs, its page shows.
 */
export function AgentScreen(props: Props) {
  const row = () => describeAgent(props.plugin, props.status);
  const controls = () => agentControls(props.plugin, props.status, props.pending);
  return (
    <section class="agent-screen" aria-labelledby="agent-title">
      <header class="agent-header">
        <span class="agent-icon" aria-hidden="true"><BotIcon /></span>
        <div class="agent-heading">
          <p class="eyebrow">Background agent</p>
          <h2 id="agent-title">{props.plugin.title}</h2>
          <p class="agent-status"><StatusDot status={row().status} /><span>{row().meta}</span></p>
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
      <Tabs class="agent-tabs" label={props.plugin.title} tabs={TABS} value={props.tab} onChange={(value) => props.onTab(value as AgentTab)}>
        <Switch>
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
