import { For, Show } from "solid-js";
import { BotIcon, ListRow, Menu, PlusIcon, PullRequestIcon } from "@prompting-time/ui";
import type { PluginsSnapshot } from "../../app/store";
import { describeAgent } from "./status";

export type InstallKind = "archive" | "folder";

type Props = {
  plugins: PluginsSnapshot;
  /** The plugin whose settings the workspace shows. */
  selected: string | null;
  /** How many past reviews each plugin's status lists, by plugin name. */
  history: Readonly<Record<string, number>>;
  /** The plugin whose past reviews the workspace shows. */
  selectedHistory: string | null;
  onSelect(name: string): void;
  onOpenHistory(name: string): void;
  onInstall(kind: InstallKind): void;
};

/**
 * "Background agents" at the bottom of All work: one quiet row per installed plugin, with its
 * icon, name, one gray line from its process and status action, and a status dot. A row opens the
 * plugin's settings. A plugin that lists past reviews gets a row under its own that opens them. The
 * menu installs a plugin for development.
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
            const row = () => describeAgent(plugin(), props.plugins.statuses[plugin().name]);
            return (
              <li>
                <ListRow
                  icon={<BotIcon />}
                  title={plugin().title}
                  meta={row().meta}
                  status={row().status}
                  selected={props.selected === plugin().name}
                  onClick={() => props.onSelect(plugin().name)}
                />
                <Show when={props.history[plugin().name]}>
                  {(count) => (
                    <ListRow
                      class="agent-history-row"
                      icon={<PullRequestIcon />}
                      title="Past reviews"
                      count={count()}
                      meta="the last 30 days"
                      selected={props.selectedHistory === plugin().name}
                      onClick={() => props.onOpenHistory(plugin().name)}
                    />
                  )}
                </Show>
              </li>
            );
          }}
        </For>
      </ul>
    </section>
  );
}
