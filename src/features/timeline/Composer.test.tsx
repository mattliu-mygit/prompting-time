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
    loadDiagnostics: vi.fn(), loadTimeline: vi.fn(), loadEventDetail: vi.fn(), loadApprovals: vi.fn(),
    loadApprovalDetail: vi.fn(), loadApprovalQuestions: vi.fn(),
    submitMessage: vi.fn().mockResolvedValue({ runId: "run-2", status: "queued", provider: "codex", duplicate: false, routingExplanation: "Continuity" }),
    steerRun: vi.fn().mockResolvedValue(undefined), respondToApproval: vi.fn(),
    interruptRun: vi.fn().mockResolvedValue(undefined), inspectWorkspace: vi.fn(), ...overrides,
  };
}

describe("Composer", () => {
  it("explains the supported keyboard action without implying a queue", () => {
    const api = actions();
    const view = render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    expect(screen.getByText("Enter to send · Shift + Enter for newline · ⌘ / Ctrl + Enter also sends")).toBeVisible();
    view.rerender(<Composer conversation={conversation({ currentRunId: "run", runStatus: "running", provider: "codex" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    expect(screen.getByText("Enter to steer · Shift + Enter for newline · ⌘ / Ctrl + Enter also steers")).toBeVisible();
    view.rerender(<Composer conversation={conversation({ currentRunId: "run", runStatus: "running", provider: "claude" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    expect(screen.queryByText(/Enter to/)).not.toBeInTheDocument();
    expect(screen.queryByText(/queued/i)).not.toBeInTheDocument();
  });
  it.each([{}, { metaKey: true }, { ctrlKey: true }])("sends on Enter with %j", async (modifiers) => {
    const api = actions();
    render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "**Hello**" } });
    expect(fireEvent.keyDown(screen.getByLabelText("Message"), { key: "Enter", ...modifiers })).toBe(false);
    await waitFor(() => expect(api.submitMessage).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ text: "**Hello**" })));
  });

  it.each([
    { shiftKey: true }, { shiftKey: true, metaKey: true }, { shiftKey: true, ctrlKey: true },
    { altKey: true }, { altKey: true, ctrlKey: true }, { isComposing: true, metaKey: true }, { keyCode: 229, ctrlKey: true },
  ])("preserves native editing without sending for %j", (modifiers) => {
    const api = actions();
    render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "draft" } });
    expect(fireEvent.keyDown(screen.getByLabelText("Message"), { key: "Enter", ...modifiers })).toBe(true);
    expect(api.submitMessage).not.toHaveBeenCalled();
    expect(api.steerRun).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Message")).toHaveValue("draft");
  });

  it("suppresses repeated Enter and duplicate events while a send is pending", async () => {
    let finish!: (value: Awaited<ReturnType<ConversationActions["submitMessage"]>>) => void;
    const submitMessage = vi.fn(() => new Promise<Awaited<ReturnType<ConversationActions["submitMessage"]>>>((resolve) => { finish = resolve; }));
    render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={actions({ submitMessage })} onMutation={vi.fn()} />);
    const field = screen.getByLabelText("Message");
    fireEvent.change(field, { target: { value: "draft" } });
    expect(fireEvent.keyDown(field, { key: "Enter", repeat: true })).toBe(false);
    expect(submitMessage).not.toHaveBeenCalled();
    act(() => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    });
    expect(submitMessage).toHaveBeenCalledTimes(1);
    expect(field).toHaveValue("draft");
    await act(async () => finish({ runId: "run", status: "queued", provider: "codex", duplicate: false, routingExplanation: "test" }));
    fireEvent.change(field, { target: { value: "next draft" } });
    expect(fireEvent.keyDown(field, { key: "Enter", repeat: true })).toBe(false);
    expect(submitMessage).toHaveBeenCalledTimes(1);
    expect(field).toHaveValue("next draft");
  });

  it.each([
    { provider: "claude" as const, runStatus: "running" as const },
    { provider: "codex" as const, runStatus: "waiting" as const },
    { provider: "codex" as const, runStatus: "completed" as const, rollupStatus: "active" as const },
  ])("does not let Enter bypass active-run restrictions: %j", (state) => {
    const api = actions();
    render(<Composer conversation={conversation({ currentRunId: "run", ...state })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "draft" } });
    fireEvent.keyDown(screen.getByLabelText("Message"), { key: "Enter" });
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(api.submitMessage).not.toHaveBeenCalled();
    expect(api.steerRun).not.toHaveBeenCalled();
    expect(api.interruptRun).not.toHaveBeenCalled();
  });

  it("retains exact Markdown on ambiguous failure and gives whitespace edits a new command identity", async () => {
    const submitMessage = vi.fn().mockRejectedValue(new BridgeError("outcome-unknown", "Disconnected", null));
    render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={actions({ submitMessage })} onMutation={vi.fn()} />);
    const draft = "    indented code\n\n**bold**\n";
    const field = screen.getByLabelText("Message");
    fireEvent.change(field, { target: { value: draft } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("alert");
    expect(submitMessage).toHaveBeenLastCalledWith(expect.objectContaining({ text: draft }));
    expect(field).toHaveValue(draft);
    fireEvent.click(screen.getByRole("button", { name: "Retry send" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Retry send" })).toBeEnabled());
    expect(submitMessage.mock.calls[1]![0].commandId).toBe(submitMessage.mock.calls[0]![0].commandId);
    fireEvent.change(field, { target: { value: `${draft}\n` } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(submitMessage).toHaveBeenCalledTimes(3));
    expect(submitMessage.mock.calls[2]![0].commandId).not.toBe(submitMessage.mock.calls[0]![0].commandId);
    expect(submitMessage.mock.calls[2]![0].text).toBe(`${draft}\n`);
  });

  it.each([false, true])("clears accepted raw text and restores edit focus after %s steering", async (steering) => {
    const api = actions();
    render(<Composer conversation={conversation(steering ? { currentRunId: "run", runStatus: "running", provider: "codex" } : {})} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    const field = screen.getByLabelText("Message");
    const draft = "    code\n\n**direction**\n";
    fireEvent.change(field, { target: { value: draft } });
    const button = screen.getByRole("button", { name: steering ? "Steer Codex" : "Send" });
    button.focus();
    fireEvent.click(button);
    await waitFor(() => expect(steering ? api.steerRun : api.submitMessage).toHaveBeenCalledWith(expect.objectContaining({ text: draft })));
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveValue("");
  });

  it("does not retry accepted text when refreshing fails", async () => {
    const api = actions();
    render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn().mockRejectedValue(new BridgeError("outcome-unknown", "Refresh failed", null))} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "accepted" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("alert");
    expect(screen.getByLabelText("Message")).toHaveValue("");
    expect(screen.queryByRole("button", { name: "Retry send" })).not.toBeInTheDocument();
  });

  it("previews safe Markdown and returns to the exact source selection without submitting", () => {
    const api = actions();
    const view = render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    const field = screen.getByLabelText<HTMLTextAreaElement>("Message");
    const draft = "**bold**\n\n- item\n\n```js\nalert('code')\n```\n\n![remote](https://remote.invalid/image.png) [unsafe](javascript:alert%281%29)\n";
    fireEvent.change(field, { target: { value: draft } });
    field.focus();
    field.setSelectionRange(2, 6, "backward");
    fireEvent.click(screen.getByRole("button", { name: "Preview Markdown" }));
    const preview = screen.getByRole("region", { name: "Message preview" });
    expect(within(preview).getByText("bold").tagName).toBe("STRONG");
    expect(within(preview).getByRole("listitem")).toHaveTextContent("item");
    expect(within(preview).getByRole("button", { name: "Copy code" })).toBeVisible();
    expect(view.container.querySelector("img, script, iframe")).toBeNull();
    expect(within(preview).queryByRole("link")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Edit Markdown" }));
    expect(screen.getByLabelText("Message")).toHaveValue(draft);
    expect(field).toHaveFocus();
    expect([field.selectionStart, field.selectionEnd, field.selectionDirection]).toEqual([2, 6, "backward"]);
    expect(api.submitMessage).not.toHaveBeenCalled();
    expect(api.steerRun).not.toHaveBeenCalled();
  });

  it("resizes to measured content after editing and shrinking", () => {
    render(<Composer conversation={conversation()} providers={providers} routingProfile="balanced" actions={actions()} onMutation={vi.fn()} />);
    const field = screen.getByLabelText("Message");
    expect(field).toHaveAttribute("rows", "2");
    Object.defineProperty(field, "scrollHeight", { configurable: true, value: 160 });
    fireEvent.change(field, { target: { value: "long\n".repeat(8) } });
    expect(parseFloat(field.style.height)).toBeGreaterThanOrEqual(160);
    Object.defineProperty(field, "scrollHeight", { configurable: true, value: 48 });
    fireEvent.change(field, { target: { value: "" } });
    expect(parseFloat(field.style.height)).toBeLessThan(160);
  });

  it("retains a failed steer and excludes duplicate steering before React updates", async () => {
    let reject!: (reason: Error) => void;
    const steerRun = vi.fn(() => new Promise<void>((_resolve, fail) => { reject = fail; }));
    render(<Composer conversation={conversation({ currentRunId: "run", runStatus: "running", provider: "codex" })} providers={providers} routingProfile="balanced" actions={actions({ steerRun })} onMutation={vi.fn()} />);
    const field = screen.getByLabelText("Message");
    const draft = "    exact direction\n";
    fireEvent.change(field, { target: { value: draft } });
    act(() => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    expect(steerRun).toHaveBeenCalledExactlyOnceWith({ runId: "run", text: draft });
    expect(field).toHaveValue(draft);
    await act(async () => reject(new Error("Steer rejected")));
    expect(screen.getByRole("alert")).toHaveTextContent("Steer rejected");
    expect(field).toHaveValue(draft);
    expect(screen.getByRole("button", { name: "Steer Codex" })).toBeEnabled();
  });

  it("does not move focus to another conversation after an old send finishes", async () => {
    let finish!: (value: Awaited<ReturnType<ConversationActions["submitMessage"]>>) => void;
    const submitMessage = vi.fn(() => new Promise<Awaited<ReturnType<ConversationActions["submitMessage"]>>>((resolve) => { finish = resolve; }));
    const messageRef = { current: null };
    const onMutation = vi.fn();
    const api = actions({ submitMessage });
    const view = render(<Composer key="old" conversation={conversation()} providers={providers} routingProfile="balanced" actions={api} onMutation={onMutation} messageRef={messageRef} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "old draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    view.rerender(<Composer key="new" conversation={conversation({ id: "conversation-2" })} providers={providers} routingProfile="balanced" actions={api} onMutation={onMutation} messageRef={messageRef} />);
    const provider = screen.getByRole("combobox", { name: "Provider" });
    provider.focus();
    await act(async () => finish({ runId: "run", status: "queued", provider: "codex", duplicate: false, routingExplanation: "test" }));
    expect(provider).toHaveFocus();
  });
  it("can interrupt steerable Codex when the other provider is unavailable", async () => {
    const api = actions();
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={[providers[0]!, { ...providers[1]!, available: false }]} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Steer Codex" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Interrupt Codex" }));
    const dialog = screen.getByRole("dialog", { name: "Interrupt Codex" });
    expect(api.interruptRun).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Interrupt Codex" }));
    await waitFor(() => expect(api.interruptRun).toHaveBeenCalledExactlyOnceWith({ runId: "run-1" }));
  });

  it.each([
    { currentRunId: "run-2" },
    { provider: "claude" as const },
    { runStatus: "completed" as const, rollupStatus: "completed" as const },
  ])("dismisses an explicit interruption after its run changes: %j", async (change) => {
    const api = actions();
    const active = conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" });
    const view = render(<Composer conversation={active} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Interrupt Codex" }));
    const confirmation = within(screen.getByRole("dialog")).getByRole("button", { name: "Interrupt Codex" });
    view.rerender(<Composer conversation={{ ...active, ...change }} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    fireEvent.click(confirmation);
    expect(api.interruptRun).not.toHaveBeenCalled();
  });

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

  it("returns from preview before restoring fallback focus after interruption", async () => {
    const api = actions();
    render(<Composer conversation={conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" })} providers={providers} routingProfile="balanced" actions={api} onMutation={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "**keep this draft**" } });
    fireEvent.click(screen.getByRole("button", { name: "Preview Markdown" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    fireEvent.click(screen.getByRole("button", { name: "Interrupt and switch to Claude" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Message preview" })).not.toBeInTheDocument());
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus();
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("**keep this draft**");
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
