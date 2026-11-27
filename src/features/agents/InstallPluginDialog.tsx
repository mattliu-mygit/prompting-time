import { createSignal, Show, untrack } from "solid-js";
import { ConfirmDialog, TextField } from "@prompting-time/ui";
import type { PluginSource } from "../../bridge/types";
import type { InstallKind } from "./PluginsSection";

type Props = {
  kind: InstallKind;
  /** The archive or folder the user picked. */
  path: string;
  /** Installs the plugin; a refused install throws with the reason. */
  onInstall(source: PluginSource): Promise<void>;
  onCancel(): void;
};

/**
 * Installs a plugin for development, from a release archive on this Mac, checked against the
 * SHA-256 the user gives, or from a plugin folder that runs in place. The host checks the manifest
 * and the programs the plugin needs, then starts it; a refusal stays in the dialog.
 */
export function InstallPluginDialog(props: Props) {
  const archive = untrack(() => props.kind === "archive");
  const [sha256, setSha256] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const checksum = () => sha256().trim().toLowerCase();

  async function install() {
    setBusy(true);
    setError(null);
    try {
      await props.onInstall(archive ? { kind: "archive", path: props.path, sha256: checksum() } : { kind: "folder", path: props.path });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Prompting Time could not install the plugin.");
      setBusy(false);
    }
  }

  return (
    <ConfirmDialog
      open
      title={archive ? "Install from a release archive" : "Run a plugin folder in place"}
      confirmLabel={busy() ? "Installing…" : "Install"}
      busy={busy()}
      confirmDisabled={archive && !/^[0-9a-f]{64}$/.test(checksum())}
      onConfirm={() => void install()}
      onCancel={() => props.onCancel()}
    >
      <p>
        {archive
          ? "Prompting Time installs the archive only if its SHA-256 matches, checks its manifest and the programs it needs, and starts it."
          : "For developing a plugin: Prompting Time runs the folder where it is, checks its manifest and the programs it needs, and starts it now and whenever Prompting Time opens."}
      </p>
      <p class="install-path"><code>{props.path}</code></p>
      <Show when={archive}>
        <TextField
          label="SHA-256"
          hint="The archive's checksum, 64 hexadecimal digits, such as the ones in its .sha256 file."
          value={sha256()}
          spellcheck={false}
          autocomplete="off"
          disabled={busy()}
          onInput={(event) => setSha256(event.currentTarget.value)}
        />
      </Show>
      <Show when={error()}>{(message) => <p role="alert" class="inline-error">{message()}</p>}</Show>
    </ConfirmDialog>
  );
}
