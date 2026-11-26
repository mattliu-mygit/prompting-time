import { createUniqueId, For, Show } from "solid-js";
import { BotIcon, Button, EllipsisIcon, Menu, PlusIcon, StatusDot, type MenuItem } from "@prompting-time/ui";
import type { PluginsSnapshot, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { agentControls, agentMenuItems, rowButtons, type AgentAction } from "./agentMenu";
import { describeAgent } from "./status";

export type InstallKind = "archive" | "folder";

/** The two ways to add a plugin, for development: the sidebar's install menu and the palette's commands. */
export const INSTALL_ITEMS: readonly (MenuItem & { value: InstallKind })[] = [
  { value: "archive", label: "Install from a release archive…" },
  { value: "folder", label: "Run a plugin folder in place…" },
];

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
 * Background agents, pinned at the top of the sidebar outside its scrolling list, so each agent and
 * its controls stay one click away: one compact row per installed plugin. Without any, or when the
 * list fails, it offers the install menu, which otherwise lives in the palette.
 */
export function BackgroundAgents(props: Props) {
  return (
    <Show when={props.plugins.loaded}>
      <section class="agents-pinned" aria-label="Background agents">
        <Show when={props.plugins.error}>{(error) => <p class="work-empty" role="alert">{error()}</p>}</Show>
        <Show when={props.plugins.items.length === 0}>
          <div class="agents-empty">
            <Show when={!props.plugins.error}><p class="work-empty">No background agents</p></Show>
            <Menu label="Add background agent" icon={<PlusIcon />} items={INSTALL_ITEMS} onSelect={(value) => props.onInstall(value as InstallKind)} />
          </div>
        </Show>
        <Show when={props.plugins.items.length > 0}>
          <ul class="agents-list">
            <For each={props.plugins.items} keyed={(plugin) => plugin.name}>
              {(plugin) => (
                <AgentRow
                  plugin={plugin()}
                  status={props.plugins.statuses[plugin().name]}
                  selected={props.selected === plugin().name}
                  pending={props.pending[plugin().name] ?? null}
                  onSelect={() => props.onSelect(plugin().name)}
                  onAction={(action) => props.onAction(plugin().name, action)}
                />
              )}
            </For>
          </ul>
        </Show>
      </section>
    </Show>
  );
}

type RowProps = {
  plugin: PluginSnapshot;
  status: PluginStatusRead | undefined;
  selected: boolean;
  pending: AgentAction | null;
  onSelect(): void;
  onAction(action: AgentAction): void;
};

/**
 * One agent: its name, which opens its screen; Pause or Resume, or Start while it is stopped, and
 * Poll now as buttons; a ⋯ menu with Stop, Settings, and Open logs; and its status in words under
 * them, red while it fails or can't start.
 */
function AgentRow(props: RowProps) {
  const statusId = createUniqueId();
  const words = () => describeAgent(props.plugin, props.status);
  const controls = () => agentControls(props.plugin, props.status, props.pending);
  const buttons = () => rowButtons(props.plugin, props.status, props.pending);
  return (
    <li class="agent-pin">
      <div class="agent-pin-top">
        <button
          type="button"
          class="agent-pin-name"
          aria-current={props.selected ? "true" : undefined}
          aria-describedby={statusId}
          title={`Open ${props.plugin.title}'s screen: logs and settings`}
          onClick={() => props.onSelect()}
        >
          <BotIcon />
          <span>{props.plugin.title}</span>
        </button>
        <span class="agent-pin-actions">
          <Show when={buttons()[0]}>
            {(control) => <Button disabled={control().disabled} onClick={() => props.onAction(control().action)}>{control().label}</Button>}
          </Show>
          <Show when={buttons()[1]}>
            {(control) => <Button disabled={control().disabled} onClick={() => props.onAction(control().action)}>{control().label}</Button>}
          </Show>
          {/* Start sits on the row while the agent is stopped, so the menu offers only Stop. */}
          <Menu
            label={`${props.plugin.title} actions`}
            icon={<EllipsisIcon />}
            items={agentMenuItems(props.plugin, props.status, props.pending, { poll: false, pause: false, power: controls().power.action === "stop", logsLabel: "Open logs" })}
            onSelect={(value) => props.onAction(value as AgentAction)}
          />
        </span>
      </div>
      <p id={statusId} class="agent-pin-status">
        <StatusDot status={words().status} />
        <span>{words().meta}</span>
      </p>
    </li>
  );
}
