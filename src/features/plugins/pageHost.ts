import type { PluginActions } from "../../app/store";
import type { PluginActionResult, PluginSnapshot } from "../../bridge/types";
import type { ConfirmOutcome } from "../agents/ConfirmActionDialog";
import { PROTOCOL_VERSION, readPageMessage, type ActionFailure, type ActionStage, type HostMessage, type HostUpdate } from "./protocol";

export type PageHostOptions = {
  /** The plugin's latest snapshot: its title and declared actions, each with its confirm flag. */
  plugin(): PluginSnapshot;
  actions: PluginActions;
  /** Sends a message to the page's frame. */
  post(message: HostMessage): void;
  /** Previews a confirm action and runs it for real once the user confirms; says how that ended. */
  confirm(action: string, args: readonly string[]): Promise<ConfirmOutcome>;
};

export type PageHost = {
  /** A message from the page's frame; one that does not fit protocol 2 is dropped. */
  receive(data: unknown): void;
  /** Shows this route in the page: now when the page is ready, else once it says it is. */
  open(route: string | null): void;
  /** Files changed in the plugin's state folder; null when more changed than one event lists. */
  filesChanged(files: readonly string[] | null): void;
  dispose(): void;
};

/**
 * The flags with which Prompting Time previews a confirm action and then runs what the user
 * confirmed. A page never passes them itself, so it cannot run a confirm action unconfirmed.
 */
const CONFIRM_FLAGS = ["--print", "--expect"];

/** A call that failed, with what the page's `error` adds to its message. */
class CallError extends Error {
  constructor(message: string, readonly failure: ActionFailure = {}) {
    super(message);
  }
}

function messageFor(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Prompting Time could not run the action.";
}

/**
 * Prompting Time's side of a page of protocol 2. A call runs only an action the manifest
 * declares, with the page's arguments after the action's own command; an action marked confirm
 * runs only through the confirmation, which previews it with `--print` and runs it with the
 * fingerprint the user confirmed. The page hears which route the user opened and which files
 * changed.
 */
export function createPageHost(options: PageHostOptions): PageHost {
  let ready = false;
  let disposed = false;
  let route: string | null = null;

  function post(update: HostUpdate) {
    if (!disposed) options.post({ ...update, version: PROTOCOL_VERSION });
  }

  async function call(name: string, args: string[]): Promise<unknown> {
    const plugin = options.plugin();
    const action = plugin.actions.find((item) => item.name === name);
    if (!action) throw new Error(`${plugin.title} declares no ${name} action.`);
    let result: PluginActionResult;
    let stage: Exclude<ActionStage, "cancelled"> = "run";
    if (action.confirm) {
      if (args.some((arg) => CONFIRM_FLAGS.some((flag) => arg === flag || arg.startsWith(`${flag}=`)))) {
        throw new Error(`${name} asks to confirm first, so Prompting Time adds ${CONFIRM_FLAGS.join(" and ")} itself.`);
      }
      const outcome = await options.confirm(name, args);
      if (outcome.stage === "cancelled") throw new CallError("Nothing ran: the confirmation was cancelled.", { stage: "cancelled" });
      if ("error" in outcome) throw new CallError(outcome.error, { stage: outcome.stage });
      ({ stage, result } = outcome);
    } else {
      try {
        result = await options.actions.pluginRequest({ plugin: plugin.name, action: name, args, confirmedFingerprint: null });
      } catch (reason) {
        throw new CallError(messageFor(reason), { stage });
      }
    }
    // A preview the confirmation ended on never gives the page a result, even one that exited 0.
    if (result.exitCode !== 0 || stage === "preview") {
      throw new CallError(result.stderr.trim() || result.stdout.trim() || `${name} exited with ${result.exitCode ?? "a signal"}.`, {
        stage,
        exit_code: result.exitCode,
        stderr: result.stderr,
      });
    }
    try {
      return JSON.parse(result.stdout);
    } catch {
      return result.stdout;
    }
  }

  return {
    receive(data) {
      const message = readPageMessage(data);
      if (!message || disposed) return;
      if (message.type === "ready") {
        // A page that loads again starts at its own start, so it hears the open route again.
        ready = true;
        if (route !== null) post({ type: "open", route });
        return;
      }
      call(message.action, message.args).then(
        (json) => post({ type: "result", reply_to: message.id, json }),
        (reason) => post({ type: "error", reply_to: message.id, message: messageFor(reason), ...(reason instanceof CallError ? reason.failure : {}) }),
      );
    },
    open(next) {
      route = next;
      if (ready && next !== null) post({ type: "open", route: next });
    },
    filesChanged(files) {
      if (ready) post({ type: "changed", files: files === null ? null : [...files] });
    },
    dispose() {
      disposed = true;
    },
  };
}
