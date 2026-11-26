import { describe, expect, it } from "vitest";
import { fixturePlugin, fixtureStatus } from "../../test/plugins";
import { describeAgent, shortTime } from "./status";

const now = new Date(2026, 10, 26, 9, 0);
const at = (hours: number, minutes: number, day = 26) => new Date(2026, 10, day, hours, minutes).toISOString();
const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const status = (value: unknown) => ({ value, error: null });

describe("a background agent's row", () => {
  it("says when a running agent polls next, with the day when it is not today", () => {
    expect(describeAgent(fixturePlugin(), status(fixtureStatus(at(15, 45))), now)).toEqual({ meta: `running · next poll ${time(at(15, 45))}`, status: "success" });
    const monday = at(10, 0, 30);
    expect(shortTime(monday, now)).toBe(`${new Date(monday).toLocaleDateString([], { weekday: "short" })} ${time(monday)}`);
  });

  it("says what a round does, and that a paused agent is paused", () => {
    const reviewing = fixtureStatus(at(15, 45), { round: { running: true, started_at: at(8, 50), reviewing: "acme/widgets#101" } });
    expect(describeAgent(fixturePlugin(), status(reviewing), now)).toEqual({ meta: "reviewing acme/widgets#101", status: "running" });
    const polling = fixtureStatus(null as unknown as string, { round: { running: true, started_at: at(8, 50), reviewing: null } });
    expect(describeAgent(fixturePlugin(), status(polling), now).meta).toBe("polling now");
    expect(describeAgent(fixturePlugin(), status(fixtureStatus(at(15, 45), { paused: true })), now)).toEqual({ meta: "paused", status: "idle" });
  });

  it("shows a failed poll with its next try in red", () => {
    const failed = fixtureStatus(at(9, 34), {
      last_round: { started_at: at(8, 15), ended_at: at(8, 19), result: "poll_failed", selected: null, reviews: [] },
    });
    expect(describeAgent(fixturePlugin(), status(failed), now)).toEqual({ meta: `poll failed ${time(at(8, 19))} · next try ${time(at(9, 34))}`, status: "error" });
  });

  it("says what keeps an agent from running before anything else", () => {
    const stopped = { kind: "notRunning" as const, exit: null };
    const missing = fixturePlugin({ process: stopped, prerequisites: [{ command: "slackdump", purpose: "Reads asks.", needed: true, path: null }] });
    expect(describeAgent(missing, undefined, now)).toEqual({ meta: "missing slackdump", status: "attention" });
    expect(describeAgent(fixturePlugin({ process: stopped, settingsError: "get-settings failed" }), undefined, now))
      .toEqual({ meta: "can't read its settings", status: "error" });
    expect(describeAgent(fixturePlugin({ process: { kind: "notRunning", exit: { code: 3, output: "no config" } } }), undefined, now))
      .toEqual({ meta: "exited with code 3", status: "error" });
    expect(describeAgent(fixturePlugin({ process: stopped }), undefined, now)).toEqual({ meta: "not running", status: "idle" });
  });

  it("falls back to the process when the status cannot be read", () => {
    expect(describeAgent(fixturePlugin({ process: { kind: "runningElsewhere", pid: 7 } }), { value: null, error: "boom" }, now))
      .toEqual({ meta: "running", status: "success" });
    expect(describeAgent(fixturePlugin(), status({ version: 2 }), now)).toEqual({ meta: "running", status: "success" });
  });
});
