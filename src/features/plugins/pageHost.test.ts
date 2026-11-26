import { describe, expect, it, vi } from "vitest";
import type { PluginActionRequest, PluginActionResult } from "../../bridge/types";
import { actionResult, fixtureV2Plugin, fixtureV2Status } from "../../test/plugins";
import { createPageHost } from "./pageHost";
import { MAX_CALL_ARGS, MAX_CALL_ARGS_LENGTH, readPageMessageV2 } from "./protocol";

const call = (id: number, action: string, args: string[] = []) => ({ version: 2, type: "call", id, action, args });

function setup(confirm: (action: string, args: readonly string[]) => Promise<PluginActionResult | null> = async () => actionResult({ stdout: "deleted\n" })) {
  const posted: Record<string, unknown>[] = [];
  const pluginRequest = vi.fn(async (request: PluginActionRequest): Promise<PluginActionResult> => {
    switch (request.action) {
      case "status":
        return actionResult({ stdout: `${JSON.stringify(fixtureV2Status())}\n` });
      case "rerun":
        return request.args.includes("missing") ? actionResult({ exitCode: 2, stderr: "fixture-runs: no run missing\n" }) : actionResult({ stdout: "queued again\n" });
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
    /** Sends a message from the page and waits for what it starts. */
    async send(message: unknown) {
      host.receive(message);
      for (let round = 0; round < 10; round += 1) await Promise.resolve();
    },
  };
}

describe("page protocol 2 messages", () => {
  it("reads ready and call", () => {
    expect(readPageMessageV2({ version: 2, type: "ready" })).toEqual({ version: 2, type: "ready" });
    expect(readPageMessageV2(call(1, "rerun", ["--item", "run-1"]))).toEqual(call(1, "rerun", ["--item", "run-1"]));
    expect(readPageMessageV2(call(2, "status"))).toEqual(call(2, "status"));
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
      expect(readPageMessageV2(message), JSON.stringify(message)?.slice(0, 80)).toBeNull();
    }
    expect(readPageMessageV2(call(1, "rerun", Array.from({ length: MAX_CALL_ARGS }, () => "x")))).not.toBeNull();
    expect(readPageMessageV2(call(1, "rerun", ["x".repeat(MAX_CALL_ARGS_LENGTH)]))).not.toBeNull();
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

    await page.send(call(3, "rerun", ["--item", "missing"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 3, message: "fixture-runs: no run missing" });
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
    let answer: PluginActionResult | null = actionResult({ stdout: "deleted run-74c91b1d\n" });
    const page = setup(async () => answer);
    await page.send(call(1, "delete", ["--item", "run-74c91b1d"]));
    expect(page.confirm).toHaveBeenCalledExactlyOnceWith("delete", ["--item", "run-74c91b1d"]);
    expect(page.pluginRequest).not.toHaveBeenCalled();
    expect(page.posted).toEqual([{ version: 2, type: "result", reply_to: 1, json: "deleted run-74c91b1d\n" }]);

    answer = null;
    await page.send(call(2, "delete", ["--item", "run-74c91b1d"]));
    expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: 2, message: "Nothing ran: the confirmation was cancelled." });

    for (const [id, args] of [[3, ["--item", "run-1", "--expect", "d".repeat(64)]], [4, ["--print"]], [5, [`--expect=${"d".repeat(64)}`]]] as const) {
      await page.send(call(id, "delete", [...args]));
      expect(page.posted.at(-1)).toEqual({ version: 2, type: "error", reply_to: id, message: "delete asks to confirm first, so Prompting Time adds --print and --expect itself." });
    }
    expect(page.confirm).toHaveBeenCalledTimes(2);
    expect(page.pluginRequest).not.toHaveBeenCalled();
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
