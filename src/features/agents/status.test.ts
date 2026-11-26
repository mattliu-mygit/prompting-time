import { describe, expect, it } from "vitest";
import { fixturePlugin, fixtureStatus } from "../../test/plugins";
import { describeAgent } from "./status";

const status = (value: unknown) => ({ value, error: null });

describe("a background agent's row", () => {
  it("shows the words and dot of the plugin's status format 2", () => {
    expect(describeAgent(fixturePlugin(), status(fixtureStatus("Running · next poll 3:45 PM")))).toEqual({ meta: "Running · next poll 3:45 PM", status: "idle" });
    expect(describeAgent(fixturePlugin(), status(fixtureStatus("x", { agent: { words: "Can't review · Docker isn't running", dot: "error" } }))))
      .toEqual({ meta: "Can't review · Docker isn't running", status: "error" });
  });

  it("says what keeps an agent from running before anything else, in red", () => {
    const stopped = { kind: "notRunning" as const, exit: null };
    const missing = fixturePlugin({ process: stopped, prerequisites: [{ command: "slackdump", purpose: "Reads asks.", needed: true, path: null }] });
    expect(describeAgent(missing, status(fixtureStatus()))).toEqual({ meta: "Missing slackdump", status: "error" });
    expect(describeAgent(fixturePlugin({ process: stopped, settingsError: "get-settings failed" }), undefined))
      .toEqual({ meta: "Can't read its settings", status: "error" });
    expect(describeAgent(fixturePlugin({ process: { kind: "notRunning", exit: { code: 3, output: "no config" } } }), undefined))
      .toEqual({ meta: "Exited with code 3", status: "error" });
    expect(describeAgent(fixturePlugin({ process: stopped }), undefined)).toEqual({ meta: "Stopped", status: "idle" });
  });

  it("says Running when the status cannot be read, is not format 2, or the manifest declares no status format", () => {
    expect(describeAgent(fixturePlugin({ process: { kind: "runningElsewhere", pid: 7 } }), { value: null, error: "boom" }))
      .toEqual({ meta: "Running", status: "idle" });
    expect(describeAgent(fixturePlugin(), status({ version: 1, paused: true }))).toEqual({ meta: "Running", status: "idle" });
    expect(describeAgent(fixturePlugin({ status: null }), status(fixtureStatus("Paused")))).toEqual({ meta: "Running", status: "idle" });
  });
});
