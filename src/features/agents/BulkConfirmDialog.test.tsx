import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { describe, expect, it, vi } from "vitest";
import type { PluginActionRequest, PluginActionResult } from "../../bridge/types";
import { actionResult, fixtureV2Plugin } from "../../test/plugins";
import { axeViolations, render } from "../../test/render";
import { BulkConfirmDialog } from "./BulkConfirmDialog";

const ITEMS = [
  { title: "Review this stack of PRs", args: ["--item", "run-a"] },
  { title: "Score last week's sessions", args: ["--item", "run-b"] },
  { title: "Tighten upload retries", args: ["--item", "run-c"] },
];
const fingerprint = (id: string, round = 1) => `${id}-${round}`.padEnd(64, "0");
const preview = (id: string, round = 1) => actionResult({ stdout: `fingerprint: ${fingerprint(id, round)}\n`, fingerprint: fingerprint(id, round), preview: `delete ${id} (round ${round})` });
const idOf = (sent: PluginActionRequest) => sent.args[1]!;

/**
 * Renders one confirmation for Delete run on ITEMS. `answer` answers each request; by default a
 * preview shows "delete <id>" and a run with its fingerprint deletes.
 */
function renderBulk(answer: (sent: PluginActionRequest) => PluginActionResult = (sent) => (sent.confirmedFingerprint === null ? preview(idOf(sent)) : actionResult({ stdout: "deleted\n" }))) {
  const pluginRequest = vi.fn(async (sent: PluginActionRequest) => answer(sent));
  const onClose = vi.fn();
  render(() => <BulkConfirmDialog plugin={fixtureV2Plugin()} action="delete" items={ITEMS} actions={{ pluginRequest, savePluginSettings: vi.fn() }} onClose={onClose} />);
  const calls = () => pluginRequest.mock.calls.map(([sent]) => [idOf(sent), sent.confirmedFingerprint]);
  return { pluginRequest, onClose, calls };
}
const list = () => screen.findByRole("list", { name: "What it will send" });
const items = async () => within(await list()).getAllByRole("listitem").map((item) => item.textContent);
const confirm = async () => {
  const button = within(screen.getByRole("alertdialog", { name: "Delete run" })).getByRole("button", { name: "Delete run" });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
};

describe("confirming a bulk plugin action once", () => {
  it("previews every item in turn and lists what each will send or why it won't run; Cancel runs nothing", async () => {
    const { onClose, calls } = renderBulk((sent) => (idOf(sent) === "run-b" ? actionResult({ exitCode: 10, stderr: "fixture-runs: no run run-b\n" }) : preview(idOf(sent))));
    expect(await items()).toEqual([
      "Review this stack of PRsdelete run-a (round 1)",
      "Score last week's sessionswon't run: fixture-runs: no run run-b",
      "Tighten upload retriesdelete run-c (round 1)",
    ]);
    expect(screen.getByRole("alertdialog", { name: "Delete run" })).toHaveTextContent("3 items: Fixture runs will send exactly this for 2 items; 1 item won't run.");
    expect(calls()).toEqual([["run-a", null], ["run-b", null], ["run-c", null]]);
    expect(await axeViolations()).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ ran: 0, failed: 1, cancelled: 2, failure: "fixture-runs: no run run-b" }));
    expect(calls()).toHaveLength(3);
  });

  it("runs each ready item with its own fingerprint on one Confirm, skipping those that won't run", async () => {
    const { onClose, calls } = renderBulk((sent) => {
      if (sent.confirmedFingerprint !== null) return idOf(sent) === "run-c" ? actionResult({ exitCode: 4, stderr: "fixture-runs: run-c is running\n" }) : actionResult({ stdout: "deleted\n" });
      return idOf(sent) === "run-b" ? actionResult({ exitCode: 10, stderr: "fixture-runs: no run run-b\n" }) : preview(idOf(sent));
    });
    await list();
    await confirm();
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ ran: 1, failed: 2, cancelled: 0, failure: "fixture-runs: run-c is running" }));
    expect(calls().slice(3)).toEqual([["run-a", fingerprint("run-a")], ["run-c", fingerprint("run-c")]]);
  });

  it("previews again only the items whose request changed (exit 13) and asks once more for them", async () => {
    const round = new Map<string, number>();
    const { onClose, calls } = renderBulk((sent) => {
      const id = idOf(sent);
      if (sent.confirmedFingerprint === null) {
        round.set(id, (round.get(id) ?? 0) + 1);
        return preview(id, round.get(id));
      }
      return id === "run-b" && sent.confirmedFingerprint === fingerprint("run-b", 1) ? actionResult({ exitCode: 13, stderr: "fixture-runs: the run changed\n" }) : actionResult({ stdout: "deleted\n" });
    });
    await list();
    await confirm();
    await waitFor(async () => expect(await items()).toEqual(["Score last week's sessionsdelete run-b (round 2)"]));
    expect(screen.getByRole("alertdialog", { name: "Delete run" })).toHaveTextContent("Nothing was sent for this item");
    expect(onClose).not.toHaveBeenCalled();
    await confirm();
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ ran: 3, failed: 0, cancelled: 0, failure: null }));
    expect(calls().slice(3)).toEqual([
      ["run-a", fingerprint("run-a")], ["run-b", fingerprint("run-b")], ["run-c", fingerprint("run-c")],
      ["run-b", null], ["run-b", fingerprint("run-b", 2)],
    ]);
  });

  it("offers only Close when no item's preview succeeded", async () => {
    const { onClose } = renderBulk(() => actionResult({ exitCode: 10, stderr: "fixture-runs: gone\n" }));
    await list();
    expect(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete run" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ ran: 0, failed: 3, cancelled: 0, failure: "fixture-runs: gone" }));
  });
});
