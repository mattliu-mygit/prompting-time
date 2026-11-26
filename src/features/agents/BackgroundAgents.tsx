import { For, Show } from "solid-js";
import { BotIcon, EllipsisIcon, ListRow, Menu, PlusIcon } from "@prompting-time/ui";
import type { PluginsSnapshot } from "../../app/store";
import { agentMenuItems, type AgentAction } from "./agentMenu";
import { describeAgent } from "./status";

export type InstallKind = "archive" | "folder";

type Props = {
  plugins: PluginsSnapshot;
  /** The plugin whose screen the workspace shows. */
  selected: string | null;
  /** The control that runs now for each plugin, by plugin name. */
  pending: Readonly<Record<string, AgentAction>>;
  onSelect(name: string): void;
  onAction(name: string, action: AgentAction): void;
  onInstall(kind: InstallKind): void;
};

/**
 * "Background agents" at the bottom of All work: one quiet row per installed plugin, with its
 * icon, name, one status sentence, and a status dot, and a menu beside it with Poll now, Pause or
 * Resume, Stop or Start, Settings, and Open logs. A row opens the plugin's screen. The add button
 * installs a plugin for development.
 */
export function BackgroundAgents(props: Props) {
  return (
    <section class="agents-group" aria-labelledby="agents-heading">
      <div class="agents-heading">
        <h2 id="agents-heading" class="work-group-heading">Background agents</h2>
        <Menu
          label="Add background agent"
          icon={<PlusIcon />}
          items={[
            { value: "archive", label: "Install from a release archive…" },
            { value: "folder", label: "Run a plugin folder in place…" },
          ]}
          onSelect={(value) => props.onInstall(value as InstallKind)}
        />
      </div>
      <Show when={props.plugins.error}>{(error) => <p class="work-empty" role="alert">{error()}</p>}</Show>
      <Show when={props.plugins.loaded && !props.plugins.error && props.plugins.items.length === 0}>
        <p class="work-empty">None installed.</p>
      </Show>
      <ul class="work-rows">
        <For each={props.plugins.items} keyed={(plugin) => plugin.name}>
          {(plugin) => {
            const status = () => props.plugins.statuses[plugin().name];
            const row = () => describeAgent(plugin(), status());
            return (
              <li class="agent-row">
                <ListRow
                  icon={<BotIcon />}
                  title={plugin().title}
                  meta={row().meta}
                  status={row().status}
                  selected={props.selected === plugin().name}
                  onClick={() => props.onSelect(plugin().name)}
                />
                <span class="agent-row-menu">
                  <Menu
                    label={`${plugin().title} actions`}
                    icon={<EllipsisIcon />}
                    items={agentMenuItems(plugin(), status(), props.pending[plugin().name] ?? null, { poll: true, pause: true, logsLabel: "Open logs" })}
                    onSelect={(value) => props.onAction(plugin().name, value as AgentAction)}
                  />
                </span>
              </li>
            );
          }}
        </For>
      </ul>
    </section>
  );
}
