/**
 * The messages between a plugin's page and Prompting Time: page protocol version 2, the only one
 * Prompting Time speaks, which a manifest names as `page.protocol: 2`. The page runs its plugin's
 * declared actions itself, so Prompting Time knows no plugin's requests. Every message is a JSON
 * object with `version: 2`, `type`, and only the fields its type lists; Prompting Time drops a
 * message from the page that does not fit, and runs nothing for it.
 *
 * Page to host: `ready`, once the page listens; and `call`, which runs one of the manifest's
 * actions with `args` after the action's own command. Prompting Time confirms first when the
 * action is marked `confirm`, and answers with `result` (what the action printed, as JSON when it
 * is JSON, else as text) or `error`, each with the call's `id` as `reply_to`. A `result` holds the
 * whole output: an action that printed more than PLUGIN_ACTION_OUTPUT_LIMIT bytes gets an `error`
 * with `truncated: true` instead. An `error` always has a `message` to show; one for a call that
 * reached the plugin or the confirmation also says how far it got (`stage`) and, when the
 * plugin's command refused, its `exit_code` and `stderr` (see ActionFailure).
 *
 * Host to page, unasked: `open`, the route of an item or blocker the user opened, sent once the
 * page is ready; and `changed`, the files that changed in the plugin's state folder (null when
 * more changed than one event lists), after which the page reads again with `call`.
 */
export const PROTOCOL_VERSION = 2;

/** The most arguments one call passes, and the most characters they hold together. */
export const MAX_CALL_ARGS = 64;
export const MAX_CALL_ARGS_LENGTH = 64 * 1024;

export type PageMessage =
  | { version: typeof PROTOCOL_VERSION; type: "ready" }
  | { version: typeof PROTOCOL_VERSION; type: "call"; id: number; action: string; args: string[] };

/**
 * How far a call that failed got: "cancelled" when the user cancelled the confirmation, "preview"
 * when a confirm action's `--print` preview failed, so the user had nothing to confirm, and "run"
 * when the action failed when run for real (with `--expect` for a confirm action).
 */
export type ActionStage = "cancelled" | "preview" | "run";

/**
 * What an `error` adds for a call that reached the plugin or the confirmation. `stage` is absent
 * when Prompting Time refused the call before then, as for an undeclared action. `exit_code` and
 * `stderr` are present only when the plugin's command ran and refused: `exit_code` is null
 * when it ended on a signal, and `stderr` is what it wrote there, untrimmed, at most its last MiB.
 * `truncated` is present, and true, only when the command succeeded but printed more than
 * PLUGIN_ACTION_OUTPUT_LIMIT bytes, so Prompting Time sends none of it rather than a cut-off part.
 */
export type ActionFailure = { stage?: ActionStage; exit_code?: number | null; stderr?: string; truncated?: true };

export type HostUpdate =
  | { type: "result"; reply_to: number; json: unknown }
  | ({ type: "error"; reply_to?: number; message: string } & ActionFailure)
  | { type: "open"; route: string }
  | { type: "changed"; files: string[] | null };

export type HostMessage = HostUpdate & { version: typeof PROTOCOL_VERSION };

const MESSAGE_FIELDS: Record<PageMessage["type"], readonly string[]> = {
  ready: ["version", "type"],
  call: ["version", "type", "id", "action", "args"],
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function fitsArgs(args: unknown): args is string[] {
  if (!Array.isArray(args) || args.length > MAX_CALL_ARGS || !args.every((arg) => typeof arg === "string")) return false;
  return args.reduce((total: number, arg: string) => total + arg.length, 0) <= MAX_CALL_ARGS_LENGTH;
}

/** The message in `data` when it fits protocol version 2, or null: the check before anything runs. */
export function readPageMessage(data: unknown): PageMessage | null {
  if (!isRecord(data) || data.version !== PROTOCOL_VERSION || typeof data.type !== "string" || !Object.hasOwn(MESSAGE_FIELDS, data.type)) return null;
  const fields = MESSAGE_FIELDS[data.type as PageMessage["type"]];
  if (Object.keys(data).length !== fields.length || !fields.every((field) => Object.hasOwn(data, field))) return null;
  if (data.type === "call" && !(Number.isSafeInteger(data.id) && typeof data.action === "string" && data.action !== "" && fitsArgs(data.args))) return null;
  return data as unknown as PageMessage;
}
