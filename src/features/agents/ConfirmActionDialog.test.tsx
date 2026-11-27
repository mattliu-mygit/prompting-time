import { fireEvent, screen, waitFor } from "@testing-library/dom";
import { describe, expect, it, vi } from "vitest";
import type { PluginActionRequest, PluginActionResult } from "../../bridge/types";
import { actionResult, fixturePlugin } from "../../test/plugins";
import { axeViolations, render } from "../../test/render";
import { ConfirmActionDialog } from "./ConfirmActionDialog";

const first = "1".repeat(64);
const second = "2".repeat(64);
const request = (event: string) => JSON.stringify({ event, body: "Pre-approving to unblock", comments: [{ path: "src/lib.rs", line: 12, body: "AI comment (vetted)\n\n**nit:** fixture" }] }, null, 2);
const preview = (fingerprint: string, event = "APPROVE") => actionResult({ stdout: `fingerprint: ${fingerprint}\n`, fingerprint, preview: request(event) });

/** Renders the confirmation for the fixture's submit of run-1, answering requests in order. */
function renderConfirmation(...answers: PluginActionResult[]) {
  const pluginRequest = vi.fn<(request: PluginActionRequest) => Promise<PluginActionResult>>();
  for (const answer of answers) pluginRequest.mockResolvedValueOnce(answer);
  const onClose = vi.fn();
  render(() => (
    <ConfirmActionDialog
      plugin={fixturePlugin()}
      action="submit"
      args={["run-1"]}
      actions={{ pluginRequest, savePluginSettings: vi.fn() }}
      onClose={onClose}
    />
  ));
  return { pluginRequest, onClose };
}

const call = (confirmedFingerprint: string | null, args = ["run-1"]) => ({ plugin: "whooper", action: "submit", args, confirmedFingerprint });

describe("confirming a plugin action", () => {
  it("shows exactly what the preview saved, and Cancel runs nothing for real", async () => {
    const { pluginRequest, onClose } = renderConfirmation(preview(first));
    const dialog = await screen.findByRole("alertdialog", { name: "Submit review" });
    const shown = await screen.findByLabelText("What it will send");
    expect(shown.textContent).toBe(request("APPROVE"));
    expect(dialog).toHaveTextContent("Posts the chosen comments to GitHub as one review.");
    expect(pluginRequest).toHaveBeenCalledExactlyOnceWith(call(null));
    expect(await axeViolations()).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ stage: "cancelled" }));
    expect(pluginRequest).toHaveBeenCalledTimes(1);
  });

  it("runs for real only with the fingerprint of the request it showed", async () => {
    const posted = actionResult({ stdout: "posted the review: https://example.invalid/review/1\n" });
    const { pluginRequest, onClose } = renderConfirmation(preview(first), posted);
    const confirm = await screen.findByRole("button", { name: "Submit review" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);

    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ stage: "run", result: posted }));
    expect(pluginRequest.mock.calls.map(([sent]) => sent)).toEqual([call(null), call(first)]);
  });

  it("shows the new request and asks again when it changed after the preview (exit 13)", async () => {
    const changed = actionResult({ exitCode: 13, stderr: "whooper-submit: nothing posted: the drafts changed" });
    const posted = actionResult({ stdout: "posted the review\n" });
    const { pluginRequest, onClose } = renderConfirmation(preview(first), changed, preview(second, "COMMENT"), posted);
    const confirm = await screen.findByRole("button", { name: "Submit review" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);

    await waitFor(() => expect(screen.getByLabelText("What it will send").textContent).toBe(request("COMMENT")));
    expect(screen.getByText(/changed after you saw it/)).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Submit review" }));

    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ stage: "run", result: posted }));
    expect(pluginRequest.mock.calls.map(([sent]) => sent)).toEqual([call(null), call(first), call(null), call(second)]);
  });

  it("counts a cancel after the request changed as a cancel", async () => {
    const changed = actionResult({ exitCode: 13, stderr: "whooper-submit: nothing posted: the drafts changed" });
    const { onClose } = renderConfirmation(preview(first), changed, preview(second, "COMMENT"));
    const confirm = await screen.findByRole("button", { name: "Submit review" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);
    await waitFor(() => expect(screen.getByLabelText("What it will send").textContent).toBe(request("COMMENT")));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ stage: "cancelled" }));
  });

  it("closes with the run's refusal when the plugin refuses the confirmed request", async () => {
    const refused = actionResult({ exitCode: 11, stderr: "whooper-submit: nothing posted: the run was deleted" });
    const { onClose } = renderConfirmation(preview(first), refused);
    const confirm = await screen.findByRole("button", { name: "Submit review" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);
    expect(await screen.findByRole("alert")).toHaveTextContent("Submit review stopped with exit code 11. Whooper said:");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ stage: "run", result: refused }));
  });

  it("closes with the reason when Prompting Time could not run the preview", async () => {
    const pluginRequest = vi.fn<(request: PluginActionRequest) => Promise<PluginActionResult>>().mockRejectedValue(new Error("Whooper is not installed."));
    const onClose = vi.fn();
    render(() => <ConfirmActionDialog plugin={fixturePlugin()} action="submit" args={["run-1"]} actions={{ pluginRequest, savePluginSettings: vi.fn() }} onClose={onClose} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Whooper is not installed.");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ stage: "preview", error: "Whooper is not installed." }));
  });

  it.each([10, 11, 12, 1])("names exit %i and shows the plugin's own words, with no way to send", async (exitCode) => {
    const refused = actionResult({ exitCode, stderr: "whooper-submit: nothing posted: fixture reason" });
    const { pluginRequest, onClose } = renderConfirmation(refused);
    expect(await screen.findByRole("alert")).toHaveTextContent(`Submit review stopped with exit code ${exitCode}. Whooper said:`);
    expect(screen.getByLabelText("What Whooper said")).toHaveTextContent("fixture reason");
    expect(screen.getByRole("button", { name: "Submit review" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledExactlyOnceWith({ stage: "preview", result: refused }));
    expect(pluginRequest).toHaveBeenCalledTimes(1);
  });
});
