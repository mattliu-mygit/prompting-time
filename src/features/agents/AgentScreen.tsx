import { Match, Show, Switch } from "solid-js";
import { Tabs } from "@prompting-time/ui";
import type { AgentControls, PluginActions, PluginFilesChange, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { AgentLog } from "./AgentLog";
import type { AgentTab } from "./agentMenu";
import { PluginSettings } from "./PluginSettings";

const TABS: Array<{ value: AgentTab; label: string }> = [
  { value: "logs", label: "Logs" },
  { value: "settings", label: "Settings" },
];

type Props = {
  plugin: PluginSnapshot;
  status: PluginStatusRead | undefined;
  tab: AgentTab;
  onTab(tab: AgentTab): void;
  /** Why the last control run from the header or the row failed, in the plugin's words. */
  error: string | null;
  actions: PluginActions;
  controls: Pick<AgentControls, "readPluginLog">;
  onPluginFilesChange(listener: (change: PluginFilesChange) => void): () => void;
};

/**
 * A plugin's Logs and Settings, in tabs. The window's header above it holds the plugin's name, its
 * status words, and its ⋯ menu with its controls; what the plugin does, such as its history of
 * jobs, its page shows.
 */
export function AgentScreen(props: Props) {
  return (
    <section class="agent-screen" aria-label={props.plugin.title}>
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
