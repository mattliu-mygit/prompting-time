import { describe, expect, it, vi } from "vitest";
import type { PluginActionRequest, PluginActionResult } from "../../bridge/types";
import { actionResult, fixtureV2Plugin, fixtureV2Status } from "../../test/plugins";
import type { ConfirmOutcome } from "../agents/ConfirmActionDialog";
import { createPageHost } from "./pageHost";
import { MAX_CALL_ARGS, MAX_CALL_ARGS_LENGTH, MAX_CALLS_IN_FLIGHT, readPageMessage } from "./protocol";

const call = (id: number, action: string, args: string[] = []) => ({ version: 2, type: "call", id, action, args });

function setup(confirm: (action: string, args: readonly string[]) => Promise<ConfirmOutcome> = async () => ({ stage: "run", result: actionResult({ stdout: "deleted\n" }) })) {
  const posted: Record<string, unknown>[] = [];
  let release = () => {};
  const hanging = new Promise<PluginActionResult>((resolve) => {
    release = () => resolve(actionResult({ stdout: "done\n" }));
  });
  const pluginRequest = vi.fn(async (request: PluginActionRequest): Promise<PluginActionResult> => {
    switch (request.action) {
      case "status":
        return actionResult({ stdout: `${JSON.stringify(fixtureV2Status())}\n` });
      case "rerun":
        if (request.args.includes("missing")) return actionResult({ exitCode: 2, stderr: "fixture-runs: no run missing\n" });
        if (request.args.includes("killed")) return actionResult({ exitCode: null, stdout: "half done\n" });
        if (request.args.includes("unstartable")) throw new Error("Prompting Time could not start fixture-runs.");
        if (request.args.includes("hang")) return hanging;
        if (request.args.includes("huge")) return actionResult({ stdout: '{"runs": [{"id": "run-1"}, {"id": "ru', stdoutTruncated: true });
        if (request.args.includes("huge-refused")) return actionResult({ exitCode: 3, stdout: "x", stdoutTruncated: true, stderr: "fixture-runs: too much\n" });
        return actionResult({ stdout: "queued again\n" });
      default:
        throw new Error(`unexpected ${request.action}`);
    }
  });
  const confirmSpy = vi.fn(confirm);
  const host = createPageHost({
    plugin: () => fixtureV2Plugin(),
    actions: { pluginRequest, savePluginSettings: vi.fn() },
    post: (message) => posted.push(message),
    confirm: confirmSpy,
  });
  return {
    host, posted, pluginRequest, confirm: confirmSpy,
    /** Ends every call that hangs. */
    release: () => release(),
    /** Sends a message from the page and waits for what it starts. */
    async send(message: unknown) {
      host.receive(message);
      for (let round = 0; round < 10; round += 1) await Promise.resolve();
    },
  };
}

describe("page protocol 2 messages", () => {
  it("reads ready and call", () => {
    expect(readPageMessage({ version: 2, type: "ready" })).toEqual({ version: 2, type: "ready" });
    expect(readPageMessage(call(1, "rerun", ["--item", "run-1"]))).toEqual(call(1, "rerun", ["--item", "run-1"]));
    expect(readPageMessage(call(2, "status"))).toEqual(call(2, "status"));
  });

  it("refuses a message that does not fit, including one with too many or too long arguments", () => {
    for (const message of [
      "ready",
      null,
      [],
      { type: "ready" },
      { version: 1, type: "ready" },
      { version: 1, type: "ready", id: 1 },
      { version: "2", type: "ready" },
      { version: 3, type: "ready" },
      { version: 2, type: "ready", id: 1 },
      { version: 2, type: "open", route: "/runs" },
      { version: 2, type: "result", reply_to: 1, json: {} },
      { version: 2, type: "toString" },
      { version: 2, type: "call", id: 1, action: "rerun" },
      { ...call(1, "rerun"), extra: true },
      call(1.5, "rerun"),
      { ...call(1, "rerun"), id: "1" },
      call(1, ""),
      { ...call(1, "rerun"), action: ["rerun"] },
      { ...call(1, "rerun"), args: "--item run-1" },
      { ...call(1, "rerun"), args: ["--item", 1] },
      call(1, "rerun", Array.from({ length: MAX_CALL_ARGS + 1 }, () => "x")),
      call(1, "rerun", ["x".repeat(MAX_CALL_ARGS_LENGTH), "y"]),
    ]) {
      expect(readPageMessage(message), JSON.stringify(message)?.slice(0, 80)).toBeNull();
    }
    expect(readPageMessage(call(1, "rerun", Array.from({ length: MAX_CALL_ARGS }, () => "x")))).not.toBeNull();
    expect(readPageMessage(call(1, "rerun", ["x".repeat(MAX_CALL_ARGS_LENGTH)]))).not.toBeNull();
  });
});

describe("the page host", () => {
  it("drops messages that do not fit and runs nothing for them", async () => {
    const page = setup();
    await page.send({ version: 1, type: "submit", id: 1, run_id: "run-1" });
    await page.send({ ...call(1, "delete"), extra: true });
    expect(page.posted).toEqual([]);
    expect(page.pluginRequest).not.toHaveBeenCalled();
    expect(page.confirm).not.toHaveBeenCalled();
  });

  it("runs a declared action with the page's arguments and answers with what it printed, as JSON or text", async () => {
    const page = setup();
    await page.send(call(1, "status"));
    expect(page.pluginRequest).toHaveBeenCalledWith({ plugin: "fixture-runs", action: "status", args: [], confirmedFingerprint: null });
    expect(page.posted).toEqual([{ version: 2, type: "result", reply_to: 1, json: fixtureV2Status() }]);

    await page.send(call(2, "rerun", ["--item", "run-74c91b1d"]));
    expect(page.pluginRequest).toHaveBeenLastCalledWith({ plugin: "fixture-runs", action: "rerun", args: ["--item", "run-74c91b1d"], confirmedFingerprint: null });
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "result", reply_to: 2, json: "queued again\n" });

  });

  it("tells the page the exit code and stderr of an action that failed, and that it failed when run", async () => {
    const page = setup();
    await page.send(call(1, "rerun", ["--item", "missing"]));
    expect(page.posted.at(-1)).toEqual({
      version: 2, type: "error", reply_to: 1, message: "fixture-runs: no run missing", stage: "run", exit_code: 2, stderr: "fixture-runs: no run missing\n",
    });

    await page.send(call(2, "rerun", ["--item", "killed"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 2, message: "half done", stage: "run", exit_code: null, stderr: "" });

    await page.send(call(3, "rerun", ["--item", "unstartable"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 3, message: "Prompting Time could not start fixture-runs.", stage: "run" });
  });

  it("answers an action that printed more than Prompting Time keeps with an error, not the cut-off part", async () => {
    const page = setup(async () => ({ stage: "run", result: actionResult({ stdout: "[1, 2", stdoutTruncated: true }) }));
    await page.send(call(1, "rerun", ["--item", "huge"]));
    expect(page.posted.at(-1)).toEqual({
      version: 2, type: "error", reply_to: 1, stage: "run", truncated: true,
      message: "rerun's output was larger than 8 MiB, the most Prompting Time passes to a page, so none of it was sent.",
    });

    // A confirm action run for real is cut off the same way.
    await page.send(call(2, "delete", ["run-1"]));
    expect(page.posted.at(-1)).toMatchObject({ type: "error", reply_to: 2, stage: "run", truncated: true });

    // A refusal still says how the plugin refused.
    await page.send(call(3, "rerun", ["--item", "huge-refused"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 3, message: "fixture-runs: too much", stage: "run", exit_code: 3, stderr: "fixture-runs: too much\n" });
  });

  it("runs at most a few of a page's calls at once and refuses the rest until one ends", async () => {
    const page = setup();
    for (let id = 1; id <= MAX_CALLS_IN_FLIGHT; id += 1) await page.send(call(id, "rerun", ["hang"]));
    expect(page.posted).toEqual([]);
    await page.send(call(100, "rerun"));
    expect(page.posted).toEqual([{ version: 2, type: "error", reply_to: 100, message: `Prompting Time runs at most ${MAX_CALLS_IN_FLIGHT} of a page's calls at once; call again when one ends.` }]);
    expect(page.pluginRequest).toHaveBeenCalledTimes(MAX_CALLS_IN_FLIGHT);

    page.release();
    for (let round = 0; round < 10; round += 1) await Promise.resolve();
    expect(page.posted).toHaveLength(MAX_CALLS_IN_FLIGHT + 1);
    await page.send(call(101, "rerun"));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "result", reply_to: 101, json: "queued again\n" });
  });

  it("refuses an action the manifest does not declare, with an error and nothing run", async () => {
    const page = setup();
    for (const [id, name] of [[1, "rm-everything"], [2, "Status"], [3, "constructor"]] as const) await page.send(call(id, name));
    expect(page.posted).toEqual([
      { version: 2, type: "error", reply_to: 1, message: "Fixture runs declares no rm-everything action." },
      { version: 2, type: "error", reply_to: 2, message: "Fixture runs declares no Status action." },
      { version: 2, type: "error", reply_to: 3, message: "Fixture runs declares no constructor action." },
    ]);
    expect(page.pluginRequest).not.toHaveBeenCalled();
    expect(page.confirm).not.toHaveBeenCalled();
  });

  it("runs an action marked confirm only through the confirmation, and never with the page's own confirm flags", async () => {
    let answer: ConfirmOutcome = { stage: "run", result: actionResult({ stdout: "deleted run-74c91b1d\n" }) };
    const page = setup(async () => answer);
    await page.send(call(1, "delete", ["--item", "run-74c91b1d"]));
    expect(page.confirm).toHaveBeenCalledExactlyOnceWith("delete", ["--item", "run-74c91b1d"]);
    expect(page.pluginRequest).not.toHaveBeenCalled();
    expect(page.posted).toEqual([{ version: 2, type: "result", reply_to: 1, json: "deleted run-74c91b1d\n" }]);

    answer = { stage: "cancelled" };
    await page.send(call(2, "delete", ["--item", "run-74c91b1d"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 2, message: "Nothing ran: the confirmation was cancelled.", stage: "cancelled" });

    for (const [id, args] of [[3, ["--item", "run-1", "--expect", "d".repeat(64)]], [4, ["--print"]], [5, [`--expect=${"d".repeat(64)}`]]] as const) {
      await page.send(call(id, "delete", [...args]));
      expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: id, message: "delete asks to confirm first, so Prompting Time adds --print and --expect itself." });
    }
    expect(page.confirm).toHaveBeenCalledTimes(2);
    expect(page.pluginRequest).not.toHaveBeenCalled();
  });

  it("tells the page whether a confirm action was refused at its preview or when run, with the plugin's exit code and stderr", async () => {
    let answer: ConfirmOutcome = { stage: "preview", result: actionResult({ exitCode: 10, stderr: "fixture-runs: run-1 is still running\n" }) };
    const page = setup(async () => answer);
    await page.send(call(1, "delete", ["--item", "run-1"]));
    expect(page.posted.at(-1)).toEqual({
      version: 2, type: "error", reply_to: 1, message: "fixture-runs: run-1 is still running", stage: "preview", exit_code: 10, stderr: "fixture-runs: run-1 is still running\n",
    });

    answer = { stage: "run", result: actionResult({ exitCode: 12, stdout: "nothing deleted\n" }) };
    await page.send(call(2, "delete", ["--item", "run-1"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 2, message: "nothing deleted", stage: "run", exit_code: 12, stderr: "" });

    // A preview that exited 0 but ended the confirmation is no result.
    answer = { stage: "preview", result: actionResult({ stdout: "printed nothing to confirm\n" }) };
    await page.send(call(3, "delete", ["--item", "run-1"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 3, message: "printed nothing to confirm", stage: "preview", exit_code: 0, stderr: "" });

    answer = { stage: "preview", error: "Prompting Time could not start fixture-runs." };
    await page.send(call(4, "delete", ["--item", "run-1"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 4, message: "Prompting Time could not start fixture-runs.", stage: "preview" });
  });

  it("sends the opened route once the page is ready, and again when it loads again", async () => {
    const page = setup();
    page.host.open("/runs/run-74c91b1d");
    expect(page.posted).toEqual([]);
    await page.send({ version: 2, type: "ready" });
    expect(page.posted).toEqual([{ version: 2, type: "open", route: "/runs/run-74c91b1d" }]);

    page.host.open("/providers");
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "open", route: "/providers" });
    await page.send({ version: 2, type: "ready" });
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "open", route: "/providers" });
    expect(page.posted).toHaveLength(3);
  });

  it("forwards the plugin's changed files once the page is ready, and nothing after it goes", async () => {
    const page = setup();
    page.host.filesChanged(["runs/run-1/run.json"]);
    expect(page.posted).toEqual([]);
    await page.send({ version: 2, type: "ready" });
    page.host.filesChanged(["runs/run-1/run.json", "status.json"]);
    page.host.filesChanged(null);
    page.host.filesChanged([]);
    expect(page.posted).toEqual([
      { version: 2, type: "changed", files: ["runs/run-1/run.json", "status.json"] },
      { version: 2, type: "changed", files: null },
      { version: 2, type: "changed", files: [] },
    ]);

    page.host.dispose();
    page.host.filesChanged(null);
    page.host.open("/runs/run-1");
    await page.send(call(9, "status"));
    expect(page.posted).toHaveLength(3);
    expect(page.pluginRequest).not.toHaveBeenCalled();
  });
});
