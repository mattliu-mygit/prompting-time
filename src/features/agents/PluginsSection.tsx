import { createUniqueId, For, Show } from "solid-js";
import { BotIcon, EllipsisIcon, Menu, StatusDot, type MenuItem } from "@prompting-time/ui";
import type { PluginsSnapshot, PluginStatusRead } from "../../app/store";
import type { PluginSnapshot } from "../../bridge/types";
import { agentMenuItems, type AgentAction } from "./agentMenu";
import { describeAgent } from "./status";

export type InstallKind = "archive" | "folder";

/** The two ways to add a plugin: the Plugins section's Add plugin menu and the palette's commands. */
export const INSTALL_ITEMS: readonly (MenuItem & { value: InstallKind })[] = [
  { value: "archive", label: "Install from a release archive…" },
  { value: "folder", label: "Run a plugin folder in place…" },
];

type Props = {
  plugins: PluginsSnapshot;
  /** The plugin whose screen, or page at its home route, the workspace shows. */
  selected: string | null;
  /** The control that runs now for each plugin, by plugin name. */
  pending: Readonly<Record<string, AgentAction>>;
  onSelect(name: string): void;
  onAction(name: string, action: AgentAction): void;
  onInstall(kind: InstallKind): void;
};

/**
 * The sidebar's Plugins section, at its bottom: one row per installed plugin and Add plugin, which
 * opens the install menu. A plugin's items live in its page; only those that need you show in the
 * sidebar, under Needs you.
 */
export function PluginsSection(props: Props) {
  const headingId = createUniqueId();
  return (
    <Show when={props.plugins.loaded}>
      <section class="sidebar-plugins" aria-labelledby={headingId}>
        <h2 id={headingId} class="work-group-heading sidebar-plugins-heading">Plugins</h2>
        <Show when={props.plugins.error}>{(error) => <p class="work-empty" role="alert">{error()}</p>}</Show>
        <Show when={props.plugins.items.length > 0}>
          <ul class="plugin-rows">
            <For each={props.plugins.items} keyed={(plugin) => plugin.name}>
              {(plugin) => (
                <PluginRow
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
        <div class="plugin-add">
          <Menu label="Add plugin" text="Add plugin…" items={INSTALL_ITEMS} onSelect={(value) => props.onInstall(value as InstallKind)} />
        </div>
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
 * One plugin: its name, which opens its page at its home route; its status words under it, red
 * while it fails or can't start; and a ⋯ menu with its controls, Settings, and Logs.
 */
function PluginRow(props: RowProps) {
  const statusId = createUniqueId();
  const words = () => describeAgent(props.plugin, props.status);
  return (
    <li class="plugin-row">
      <button
        type="button"
        class="plugin-row-name"
        aria-current={props.selected ? "true" : undefined}
        aria-describedby={statusId}
        onClick={() => props.onSelect()}
      >
        <BotIcon />
        <span>{props.plugin.title}</span>
      </button>
      <Menu
        label={`${props.plugin.title} actions`}
        icon={<EllipsisIcon />}
        items={agentMenuItems(props.plugin, props.status, props.pending)}
        onSelect={(value) => props.onAction(value as AgentAction)}
      />
      <p id={statusId} class="plugin-row-status">
        <StatusDot status={words().status} />
        <span>{words().meta}</span>
      </p>
    </li>
  );
}
