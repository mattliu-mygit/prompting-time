import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ConversationSummary, ProviderInstallation } from "../../bridge/types";
import { BridgeError } from "../../bridge/api";
import type { ConversationActions } from "../../app/store";
import { Composer } from "./Composer";

const providers: ProviderInstallation[] = [
  { id: "codex", installed: true, available: true, version: "1", diagnostic: null, capabilities: ["steering", "interruption"] },
  { id: "claude", installed: true, available: true, version: "2", diagnostic: null, capabilities: ["interruption"] },
];

function conversation(overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return {
    id: "conversation-1", title: "Work", workspaceId: null, archived: false,
    projectRoot: null, routingProfile: "balanced", currentRunId: null, provider: null, runStatus: null,
    rollupStatus: null, agents: [], agentsTruncated: false, ...overrides,
  };
}

function actions(overrides: Partial<ConversationActions> = {}): ConversationActions {
  return {
    loadTimeline: vi.fn(), loadEventDetail: vi.fn(), loadApprovals: vi.fn(),
    loadApprovalDetail: vi.fn(), loadApprovalQuestions: vi.fn(),
    submitMessage: vi.fn().mockResolvedValue({ runId: "run-2", status: "queued", provider: "codex", duplicate: false, routingExplanation: "Continuity" }),
    steerRun: vi.fn().mockResolvedValue(undefined), respondToApproval: vi.fn(),
    interruptRun: vi.fn().mockResolvedValue(undefined), inspectWorkspace: vi.fn(), ...overrides,
  };
}

describe("Composer", () => {
  it("exposes in-progress work and restores focus after a successful interrupt switch commit", async () => {
    let finishInterrupt!: () => void;
    const interruptRun = vi.fn(() => new Promise<void>((resolve) => { finishInterrupt = resolve; }));
    const api = actions({ interruptRun });
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    fireEvent.click(screen.getByRole("button", { name: "Interrupt and switch to Claude" }));
    expect(screen.getByRole("region", { name: "Message composer" })).toHaveAttribute("aria-busy", "true");
    await act(async () => finishInterrupt());
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(screen.getByRole("region", { name: "Message composer" })).toHaveAttribute("aria-busy", "false");
  });

  it("shows Auto with its active profile and excludes unavailable manual routes", () => {
    render(<Composer conversation={conversation()} providers={[providers[0]!, { ...providers[1]!, available: false, diagnostic: "Sign in first" }]} routingProfile="balanced" actions={actions()} onMutation={vi.fn()} />);
    expect(screen.getByRole("option", { name: "Auto · Balanced" })).toBeVisible();
    expect(screen.getByRole("option", { name: /Claude — Sign in first/ })).toBeDisabled();
  });

  it("reuses one command ID when retrying a failed logical send", async () => {
    const submitMessage = vi.fn()
      .mockRejectedValueOnce(new BridgeError("outcome-unknown", "transport disconnected", null))
      .mockResolvedValueOnce({ runId: "run-2", status: "queued", provider: "codex", duplicate: true, routingExplanation: "Continuity" });
    const api = actions({ submitMessage });
    render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "Run tests" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("transport disconnected");
    fireEvent.click(screen.getByRole("button", { name: "Retry send" }));
    await waitFor(() => expect(submitMessage).toHaveBeenCalledTimes(2));
    expect(submitMessage.mock.calls[0]?.[0].commandId).toBe(submitMessage.mock.calls[1]?.[0].commandId);
    expect(submitMessage.mock.calls[0]?.[0].commandId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
  });

  it.each([
    "provider-unavailable",
    "invalid-request",
    "conversation-busy",
    "queue-full",
    "command-conflict",
    "storage-error",
    "runtime-error",
    "stale-approval",
    "internal",
  ])("uses a new command ID after semantic failure %s", async (code) => {
    const submitMessage = vi.fn()
      .mockRejectedValueOnce(new BridgeError(code, "Request rejected", null))
      .mockResolvedValueOnce({ runId: "run-2", status: "queued", provider: "codex", duplicate: false, routingExplanation: "Continuity" });
    render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={actions({ submitMessage })} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "Run tests" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Request rejected");
    expect(screen.getByRole("button", { name: "Send" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(submitMessage).toHaveBeenCalledTimes(2));
    expect(submitMessage.mock.calls[0]?.[0].commandId).not.toBe(submitMessage.mock.calls[1]?.[0].commandId);
  });

  it("requires explicit interruption before switching an active provider", async () => {
    const api = actions();
    const view = render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    const dialog = screen.getByRole("dialog", { name: "Interrupt Codex to switch provider" });
    expect(dialog).toBeVisible();
    expect(screen.getByRole("button", { name: "Interrupt and switch to Claude" })).toHaveFocus();
    expect(api.interruptRun).not.toHaveBeenCalled();
    expect(api.submitMessage).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Interrupt and switch to Claude" }));
    await waitFor(() => expect(api.interruptRun).toHaveBeenCalledWith({ runId: "run-1" }));
    expect(screen.getByRole("combobox", { name: "Provider" })).toHaveValue("claude");
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeDisabled();
    expect(screen.getByText(/Waiting for Codex to stop/)).toBeVisible();

    view.rerender(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "interrupted", rollupStatus: "interrupted" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeEnabled();
  });

  it("portals the interrupt modal outside an inert composer surface and restores interaction on close", () => {
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={actions()} onMutation={vi.fn()} />);
    const composer = screen.getByRole("region", { name: "Message composer" });
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    const dialog = screen.getByRole("dialog", { name: "Interrupt Codex to switch provider" });

    expect(composer).toHaveAttribute("inert");
    expect(composer).not.toContainElement(dialog);
    fireEvent.click(within(dialog).getByRole("button", { name: "Keep Codex running" }));
    expect(composer).not.toHaveAttribute("inert");
  });

  it("renders interrupt failure feedback inside the active dialog", async () => {
    const api = actions({ interruptRun: vi.fn().mockRejectedValue(new Error("Interrupt failed")) });
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    const dialog = screen.getByRole("dialog", { name: "Interrupt Codex to switch provider" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Interrupt and switch to Claude" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Interrupt failed");
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });

  it("closes a pending provider switch when the active run becomes terminal", async () => {
    const api = actions();
    const view = render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    expect(screen.getByRole("dialog")).toBeVisible();

    view.rerender(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "completed", rollupStatus: "completed" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Provider" })).toHaveFocus());
    expect(api.interruptRun).not.toHaveBeenCalled();
  });

  it("does not let a stale switch dialog interrupt a replacement run", async () => {
    const api = actions();
    const view = render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    expect(screen.getByRole("dialog")).toBeVisible();

    view.rerender(<Composer conversation={conversation({ currentRunId: "run-2", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Provider" })).toHaveFocus());
    expect(api.interruptRun).not.toHaveBeenCalled();
  });

  it("revalidates target availability before interrupting", async () => {
    const api = actions();
    const activeConversation = conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" });
    const view = render(<Composer conversation={activeConversation} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    expect(screen.getByRole("dialog")).toBeVisible();

    view.rerender(<Composer conversation={activeConversation} providers={[providers[0]!, { ...providers[1]!, available: false, diagnostic: "Sign in again" }]} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Interrupt and switch to Claude" }));

    expect(api.interruptRun).not.toHaveBeenCalled();
    expect(await screen.findByRole("alert")).toHaveTextContent("Claude is unavailable: Sign in again");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Provider" })).toHaveValue("auto");
  });

  it("restores focus to an enabled composer control when descendants remain active", async () => {
    const api = actions();
    const view = render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });

    view.rerender(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "completed", rollupStatus: "active" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeDisabled();
  });

  it("closes the switch confirmation with Escape and restores provider focus", async () => {
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={actions()} onMutation={vi.fn()} />);
    const select = screen.getByRole("combobox", { name: "Provider" });
    fireEvent.change(select, { target: { value: "claude" } });
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => expect(select).toHaveFocus());
  });

  it("steers only when the active provider reports steering", async () => {
    const api = actions();
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "Focus on tests" } });
    fireEvent.click(screen.getByRole("button", { name: "Steer Codex" }));
    await waitFor(() => expect(api.steerRun).toHaveBeenCalledWith({ runId: "run-1", text: "Focus on tests" }));
  });

  it("explains why a non-steerable active run cannot accept another message", () => {
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "claude", runStatus: "running" })} providers={providers} routingProfile="usageBalance" actions={actions()} onMutation={vi.fn()} />);
    expect(screen.getByText(/Claude cannot be steered/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("does not offer interruption when the active adapter lacks that capability", () => {
    const noInterrupt = providers.map((provider) => provider.id === "codex"
      ? { ...provider, capabilities: ["steering"] as ProviderInstallation["capabilities"] }
      : provider);
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={noInterrupt} routingProfile="balanced" actions={actions()} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    expect(screen.getByText(/does not report interruption support/i)).toBeVisible();
    expect(screen.getByRole("button", { name: "Interruption unavailable" })).toBeDisabled();
  });

  it("treats active descendants as an active turn boundary", () => {
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "completed", rollupStatus: "active" })} providers={providers} routingProfile="balanced" actions={actions()} onMutation={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeDisabled();
    expect(screen.getByText(/Active child agents must finish/i)).toBeVisible();
  });
});
