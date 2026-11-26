import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { createSignal, flush, Show, type ComponentProps } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConversationSummary, ProviderInstallation, TimelineItem } from "../../bridge/types";
import { BridgeError } from "../../bridge/api";
import { createAppStore, createSnapshot, type AppApi, type AppStore, type ConversationActions } from "../../app/store";
import { render, settle } from "../../test/render";
import { Composer } from "./Composer";
import { Timeline } from "./Timeline";

let testStore: AppStore;
let currentActions: ConversationActions;
let saveThinking: AppApi["setThinkingPreference"];
let saveContextBudget: AppApi["setContextBudget"];

beforeEach(async () => {
  currentActions = actions();
  let savedConversations = [conversation(), conversation({ id: "conversation-2" })];
  saveThinking = vi.fn(async ({ conversationId, preference }) => {
    savedConversations = savedConversations.map(item => item.id === conversationId ? { ...item, thinkingPreference: preference } : item);
  });
  saveContextBudget = vi.fn(async ({ conversationId, preference }) => {
    savedConversations = savedConversations.map(item => item.id === conversationId ? { ...item, contextBudget: preference } : item);
  });
  testStore = createAppStore({
    ...currentActions,
    submitMessage: request => currentActions.submitMessage(request),
    steerRun: request => currentActions.steerRun(request),
    getBootstrap: vi.fn().mockResolvedValue({ providers, startupDiagnostic: null }),
    setThinkingPreference: request => saveThinking(request),
    setContextBudget: request => saveContextBudget(request),
    listConversations: vi.fn(async () => ({ items: savedConversations, nextCursor: null })),
    loadConversation: vi.fn(async ({ conversationId }) => savedConversations.find(item => item.id === conversationId)!), listChildConversations: vi.fn(), loadConversationPath: vi.fn(),
    listenToAppEvents: vi.fn().mockResolvedValue(() => {}), listRunAudits: vi.fn(),
    loadRunAudit: vi.fn(), createConversation: vi.fn(), archiveConversation: vi.fn(),
    inspectProject: vi.fn(), pickProjectDirectory: vi.fn(),
  });
  await testStore.initialize();
});
afterEach(() => testStore.dispose());

type ComposerMount = Omit<ComponentProps<typeof Composer>, "store" | "actions"> & { actions: ConversationActions; key?: string };

/** Renders the Composer as App does: a new one per conversation (or `key`), updated in place otherwise. */
function renderComposer(initial: ComposerMount) {
  currentActions = initial.actions;
  let onMutation = initial.onMutation;
  const [mounted, setMounted] = createSignal(initial);
  const view = render(() => (
    <Show when={mounted().key ?? mounted().conversation.id} keyed>
      {/* A keyed child must take the value to remount when it changes. */}
      {/* eslint-disable-next-line @typescript-eslint/no-unused-vars */}
      {(_key: string) => (
        <Composer
          conversation={mounted().conversation}
          providers={mounted().providers}
          routingProfile={mounted().routingProfile}
          actions={mounted().actions}
          store={testStore}
          onMutation={() => onMutation()}
        />
      )}
    </Show>
  ));
  return {
    ...view,
    rerender(next: ComposerMount) {
      currentActions = next.actions;
      onMutation = next.onMutation;
      setMounted(next);
      flush();
    },
  };
}

/** The Composer for conversation-1 as the store has it, as App passes it. */
function renderConnectedComposer() {
  function Connected() {
    const snapshot = createSnapshot(testStore);
    return <Composer conversation={snapshot().conversationsById["conversation-1"]!} providers={providers} routingProfile="balanced" actions={currentActions} store={testStore} onMutation={vi.fn()} />;
  }
  render(() => <Connected />);
}

/** fireEvent, then Solid's queued updates, as React's fireEvent wraps act. */
const fire = new Proxy(fireEvent, {
  get(target, name: keyof typeof fireEvent) {
    return (element: Element | Window, init?: object) => {
      const result = (target[name] as (element: Element | Window, init?: object) => boolean)(element, init);
      flush();
      return result;
    };
  },
});

/** Runs a test step, then lets promises settle and applies Solid's updates. */
function act(run: () => unknown): Promise<void> {
  const result = run();
  if (result instanceof Promise) return result.then(() => settle());
  flush();
  return Promise.resolve();
}

const providers: ProviderInstallation[] = [
  { id: "codex", installed: true, available: true, version: "1", diagnostic: null, capabilities: ["steering", "interruption"] },
  { id: "claude", installed: true, available: true, version: "2", diagnostic: null, capabilities: ["interruption"] },
];

function conversation(overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return {
    contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null, thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
    id: "conversation-1", title: "Work", workspaceId: null, archived: false,
    parentId: null, hasChildren: false, summary: null,
    capabilities: { canSend: true, canInterrupt: true, canArchive: true, canRoute: true, unavailableReason: null },
    projectRoot: null, routingProfile: "balanced", currentRunId: null, provider: null, runStatus: null,
    rollupStatus: null, ...overrides,
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
  it("blocks send during context save, preserves the draft and visibly rolls back a failed save", async () => {
    let reject!: (reason: Error) => void;
    saveContextBudget = vi.fn(() => new Promise<void>((_, fail) => { reject = fail; }));
    renderConnectedComposer();
    fire.input(screen.getByLabelText("Message"), { target: { value: "keep draft" } });
    fire.change(screen.getByRole("combobox", { name: "Context budget" }), { target: { value: "500000" } });
    expect(saveContextBudget).toHaveBeenCalledExactlyOnceWith({ conversationId: "conversation-1", preference: { kind: "tokens", tokens: 500000 } });
    expect(screen.getByRole("combobox", { name: "Context budget" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    fire.keyDown(screen.getByLabelText("Message"), { key: "Enter" });
    expect(currentActions.submitMessage).not.toHaveBeenCalled();
    await act(async () => reject(new Error("Context save failed")));
    expect(screen.getByRole("alert")).toHaveTextContent("Context save failed");
    expect(screen.getByRole("combobox", { name: "Context budget" })).toHaveValue("300000");
    expect(screen.getByLabelText("Message")).toHaveValue("keep draft");
  });

  it("saves next-turn context while busy and sends unchanged steering", async () => {
    renderComposer({ conversation: conversation({ currentRunId: "run", runStatus: "running", provider: "codex" }), providers: providers, routingProfile: "balanced", actions: currentActions, onMutation: vi.fn() });
    expect(screen.getByText("Context budget · Next turn")).toBeVisible();
    fire.change(screen.getByRole("combobox", { name: "Context budget" }), { target: { value: "providerDefault" } });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Context budget" })).toBeEnabled());
    fire.input(screen.getByLabelText("Message"), { target: { value: "direction" } });
    fire.click(screen.getByRole("button", { name: "Steer Codex" }));
    await waitFor(() => expect(currentActions.steerRun).toHaveBeenCalledExactlyOnceWith({ conversationId: "conversation-1", runId: "run", text: "direction" }));
    expect(saveContextBudget).toHaveBeenCalledExactlyOnceWith({ conversationId: "conversation-1", preference: { kind: "providerDefault" } });
    expect(currentActions.submitMessage).not.toHaveBeenCalled();
  });
  it("offers the five context budgets with the saved 300k default and qualified help", () => {
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    const select = screen.getByRole("combobox", { name: "Context budget" });
    expect(within(select).getAllByRole("option").map(option => option.textContent)).toEqual(["200k", "300k", "400k", "500k", "Provider default"]);
    expect(select).toHaveValue("300000");
    fire.click(screen.getByRole("button", { name: "Composer help" }));
    expect(screen.getByText(/Providers may compact earlier/)).toBeVisible();
  });
  it("saves immediately, disables send and choice while pending, and restores choice on failure", async () => {
    let reject!: (reason: Error) => void;
    saveThinking = vi.fn(() => new Promise<void>((_, fail) => { reject = fail; }));
    renderConnectedComposer();
    fire.input(screen.getByLabelText("Message"), { target: { value: "keep draft" } });
    fire.change(screen.getByRole("combobox", { name: "Thinking" }), { target: { value: "manual:high" } });
    expect(saveThinking).toHaveBeenCalledExactlyOnceWith({ conversationId: "conversation-1", preference: { kind: "manual", level: "high" } });
    expect(screen.getByRole("combobox", { name: "Thinking" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByLabelText("Message")).toBeEnabled();
    await act(async () => reject(new Error("Preference save failed")));
    expect(screen.getByRole("alert")).toHaveTextContent("Preference save failed");
    expect(screen.getByRole("combobox", { name: "Thinking" })).toHaveValue("auto");
    expect(screen.getByLabelText("Message")).toHaveValue("keep draft");
  });

  it("allows next-turn preference changes while steering keeps the active request unchanged", async () => {
    renderComposer({ conversation: conversation({ currentRunId: "run", runStatus: "running", provider: "codex" }), providers: providers, routingProfile: "balanced", actions: currentActions, onMutation: vi.fn() });
    expect(screen.getByText("Thinking · Next turn")).toBeVisible();
    fire.change(screen.getByRole("combobox", { name: "Thinking" }), { target: { value: "manual:low" } });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Thinking" })).toBeEnabled());
    fire.input(screen.getByLabelText("Message"), { target: { value: "direction" } });
    fire.click(screen.getByRole("button", { name: "Steer Codex" }));
    await waitFor(() => expect(currentActions.steerRun).toHaveBeenCalledExactlyOnceWith({ conversationId: "conversation-1", runId: "run", text: "direction" }));
    expect(saveThinking).toHaveBeenCalledTimes(1);
    expect(currentActions.submitMessage).not.toHaveBeenCalled();
  });

  it("retries the frozen command even after refreshed metadata reveals an active run", async () => {
    const api = actions({ submitMessage: vi.fn().mockRejectedValue(new BridgeError("outcome-unknown", "Unknown outcome", null)) });
    const view = renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "review this" } });
    fire.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("button", { name: "Retry send" });
    const original = vi.mocked(api.submitMessage).mock.calls[0]![0];
    view.rerender({ conversation: conversation({ currentRunId: "accepted", runStatus: "running", provider: "codex" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.click(screen.getByRole("button", { name: "Retry send" }));
    await waitFor(() => expect(api.submitMessage).toHaveBeenCalledTimes(2));
    expect(vi.mocked(api.submitMessage).mock.calls[1]![0]).toEqual(original);
    expect(api.steerRun).not.toHaveBeenCalled();
  });
  it("offers saved Auto and common Thinking levels with honest pre-dispatch help", () => {
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    const thinking = screen.getByRole("combobox", { name: "Thinking" });
    expect(thinking).toHaveValue("auto");
    expect(within(thinking).getAllByRole("option").map(option => option.textContent)).toEqual(["Auto", "Provider default", "Low", "Medium", "High"]);
    expect(screen.queryByText(/configured High/)).not.toBeInTheDocument();
    fire.click(screen.getByRole("button", { name: "Composer help" }));
    expect(screen.getByText(/Auto chooses effort locally/)).toBeVisible();
  });

  it("distinguishes a reported configuration with no effort setting from an unreported configuration", () => {
    renderComposer({ conversation: conversation({ thinkingDecision: { preference: { kind: "providerDefault" }, requestedEffort: null, reason: "Native default." },
      thinkingConfiguration: { model: "fixture", supportedEfforts: [], configuredEffort: null } }), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    expect(screen.getByText("Requested Provider default · configured effort not set")).toBeVisible();
    expect(screen.queryByText(/configuration unreported/)).not.toBeInTheDocument();
  });

  it("filters extra effort levels by provider and preserves an unsupported saved choice", () => {
    const saved = conversation({ provider: "codex", thinkingPreference: { kind: "manual", level: "xhigh" },
      thinkingConfiguration: { model: "fixture-model", supportedEfforts: ["low", "medium", "high", "xhigh"], configuredEffort: "medium" },
      thinkingDecision: { preference: { kind: "auto" }, requestedEffort: "high", reason: "Engineering task." } });
    const view = renderComposer({ conversation: saved, providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    expect(screen.getByRole("option", { name: "Xhigh" })).toBeInTheDocument();
    expect(screen.getByText("Auto → High · configured Medium")).toBeVisible();
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    expect(screen.getByRole("option", { name: /Xhigh.*saved/ })).toBeInTheDocument();
    expect(screen.getByText(/Xhigh is not reported/)).toBeVisible();
    view.rerender({ conversation: { ...saved, thinkingPreference: { kind: "auto" } }, providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    expect(screen.queryByRole("option", { name: /Xhigh/ })).not.toBeInTheDocument();
  });

  it.each(["empty", "lifecycle", "tool", "message"] as const)("keeps the child completeness caveat visible with %s history", async kind => {
    const child = conversation({ id: "reviewer", parentId: "conversation-1", capabilities: {
      canSend: false, canInterrupt: false, canArchive: false, canRoute: false, unavailableReason: "Recorded activity only.",
    } });
    const items: TimelineItem[] = kind === "empty" ? [] : [{ id: "captured", sequence: "1", conversationId: child.id,
      runId: "run-1", agentId: "child-execution", kind, role: kind === "message" ? "assistant" : null,
      presentation: "normal", provider: "codex", content: "Captured activity", contentBytes: "17", truncated: false, operation: null }];
    const api = actions({ loadTimeline: vi.fn().mockResolvedValue({
      items, nextCursor: null, approvals: [], approvalsTruncated: false, activeCompaction: null, approvalsNextCursor: null,
    }) });
    currentActions = api;
    render(() => <><Timeline conversation={child} refreshVersion={0} actions={api} />
      <Composer conversation={child} providers={providers} routingProfile="balanced" actions={api} store={testStore} onMutation={vi.fn()} /></>);
    await waitFor(() => expect(screen.queryByText("Loading activity…")).not.toBeInTheDocument());
    const composer = within(screen.getByRole("region", { name: "Message composer" }));
    expect(composer.getByText("Captured activity may be incomplete.")).toBeVisible();
    expect(screen.getAllByText("Captured activity may be incomplete.")).toHaveLength(1);
    expect(composer.queryByRole("textbox")).not.toBeInTheDocument();
    expect(composer.queryByRole("combobox")).not.toBeInTheDocument();
    expect(composer.queryByRole("button")).not.toBeInTheDocument();
    if (kind === "message") expect(screen.getByText("Captured activity")).toBeVisible();
    if (kind === "empty") expect(screen.getByText("No recorded activity is available.")).toBeVisible();
  });

  it("does not present the recorded-child completeness caveat on a managed root", () => {
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    expect(screen.queryByText("Captured activity may be incomplete.")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Message" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Send" })).toBeVisible();
  });

  it("restores provider choice, preview and draft across a recorded child visit", () => {
    const api = actions();
    const view = renderComposer({ key: "parent", conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "**Parent draft**" } });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    fire.click(screen.getByRole("button", { name: "Preview Markdown" }));
    view.rerender({ key: "child", conversation: conversation({ id: "reviewer", parentId: "conversation-1", capabilities: { canSend: false, canInterrupt: false, canArchive: false, canRoute: false, unavailableReason: "Recorded activity only." } }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    view.rerender({ key: "parent", conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    expect(screen.getByRole("combobox", { name: "Provider" })).toHaveValue("claude");
    expect(screen.getByRole("region", { name: "Message preview" })).toHaveTextContent("Parent draft");
    fire.click(screen.getByRole("button", { name: "Edit Markdown" }));
    expect(screen.getByLabelText("Message")).toHaveValue("**Parent draft**");
  });
  it("explains recorded children without exposing parent controls", () => {
    const api = actions();
    renderComposer({ conversation: conversation({ id: "reviewer", parentId: "conversation-1", currentRunId: "parent-run", provider: "codex", runStatus: "running", capabilities: { canSend: false, canInterrupt: false, canArchive: false, canRoute: false, unavailableReason: "Recorded activity only." } }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    expect(screen.getByText("Recorded activity only.")).toBeVisible();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    fire.keyDown(screen.getByText("Recorded activity only."), { key: "Enter" });
    expect(api.submitMessage).not.toHaveBeenCalled();
    expect(api.steerRun).not.toHaveBeenCalled();
    expect(api.interruptRun).not.toHaveBeenCalled();
  });
  it("explains the supported keyboard action without implying a queue", () => {
    const api = actions();
    const view = renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    const help = screen.getByRole("button", { name: "Composer help" });
    expect(help).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText("Enter to send · Shift + Enter for newline · ⌘ / Ctrl + Enter also sends")).not.toBeVisible();
    fire.click(screen.getByRole("button", { name: "Composer help" }));
    expect(help).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Enter to send · Shift + Enter for newline · ⌘ / Ctrl + Enter also sends")).toBeVisible();
    expect(api.submitMessage).not.toHaveBeenCalled();
    view.rerender({ conversation: conversation({ currentRunId: "run", runStatus: "running", provider: "codex" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    expect(screen.getByText("Enter to steer · Shift + Enter for newline · ⌘ / Ctrl + Enter also steers")).toBeVisible();
    view.rerender({ conversation: conversation({ currentRunId: "run", runStatus: "running", provider: "claude" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    expect(screen.queryByText(/Enter to/)).not.toBeInTheDocument();
    expect(screen.queryByText(/queued/i)).not.toBeInTheDocument();
  });
  it("places the visible provider choice with the actions below the message and keeps routine routing help collapsed", () => {
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    const field = screen.getByLabelText("Message");
    const provider = screen.getByRole("combobox", { name: "Provider" });
    expect(field.compareDocumentPosition(provider) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(provider.closest(".composer-actions")).toContainElement(screen.getByRole("button", { name: "Send" }));
    expect(screen.getByText("Prompting Time will explain the selected route.")).not.toBeVisible();
    fire.click(screen.getByRole("button", { name: "Composer help" }));
    expect(screen.getByText("Prompting Time will explain the selected route.")).toBeVisible();
    const content = screen.getByText("Prompting Time will explain the selected route.").parentElement!;
    const footer = provider.closest(".composer-actions")!;
    expect(footer).not.toContainElement(content);
    expect(footer.nextElementSibling).toBe(content);
    expect(screen.getByRole("button", { name: "Composer help" })).toHaveAttribute("aria-controls", content.id);
    fire.click(screen.getByRole("button", { name: "Composer help" }));
    expect(content).not.toBeVisible();
  });
  it.each([{}, { metaKey: true }, { ctrlKey: true }])("sends on Enter with %j", async (modifiers) => {
    const api = actions();
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "**Hello**" } });
    expect(fire.keyDown(screen.getByLabelText("Message"), { key: "Enter", ...modifiers })).toBe(false);
    await waitFor(() => expect(api.submitMessage).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ text: "**Hello**" })));
  });

  it.each([
    { shiftKey: true }, { shiftKey: true, metaKey: true }, { shiftKey: true, ctrlKey: true },
    { altKey: true }, { altKey: true, ctrlKey: true }, { isComposing: true, metaKey: true }, { keyCode: 229, ctrlKey: true },
  ])("preserves native editing without sending for %j", (modifiers) => {
    const api = actions();
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "draft" } });
    expect(fire.keyDown(screen.getByLabelText("Message"), { key: "Enter", ...modifiers })).toBe(true);
    expect(api.submitMessage).not.toHaveBeenCalled();
    expect(api.steerRun).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Message")).toHaveValue("draft");
  });

  it("suppresses repeated Enter and duplicate events while a send is pending", async () => {
    let finish!: (value: Awaited<ReturnType<ConversationActions["submitMessage"]>>) => void;
    const submitMessage = vi.fn(() => new Promise<Awaited<ReturnType<ConversationActions["submitMessage"]>>>((resolve) => { finish = resolve; }));
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions({ submitMessage }), onMutation: vi.fn() });
    const field = screen.getByLabelText("Message");
    fire.input(field, { target: { value: "draft" } });
    expect(fire.keyDown(field, { key: "Enter", repeat: true })).toBe(false);
    expect(submitMessage).not.toHaveBeenCalled();
    act(() => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    });
    expect(submitMessage).toHaveBeenCalledTimes(1);
    expect(field).toHaveValue("draft");
    await act(async () => finish({ runId: "run", status: "queued", provider: "codex", duplicate: false, routingExplanation: "test", thinkingDecision: { preference: { kind: "auto" }, requestedEffort: "high", reason: "fixture" } }));
    fire.input(field, { target: { value: "next draft" } });
    expect(fire.keyDown(field, { key: "Enter", repeat: true })).toBe(false);
    expect(submitMessage).toHaveBeenCalledTimes(1);
    expect(field).toHaveValue("next draft");
  });

  it.each([
    { provider: "claude" as const, runStatus: "running" as const },
    { provider: "codex" as const, runStatus: "waiting" as const },
    { provider: "codex" as const, runStatus: "completed" as const, rollupStatus: "active" as const },
  ])("does not let Enter bypass active-run restrictions: %j", (state) => {
    const api = actions();
    renderComposer({ conversation: conversation({ currentRunId: "run", ...state }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "draft" } });
    fire.keyDown(screen.getByLabelText("Message"), { key: "Enter" });
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(api.submitMessage).not.toHaveBeenCalled();
    expect(api.steerRun).not.toHaveBeenCalled();
    expect(api.interruptRun).not.toHaveBeenCalled();
  });

  it("retains exact Markdown on ambiguous failure and gives whitespace edits a new command identity", async () => {
    const submitMessage = vi.fn().mockRejectedValue(new BridgeError("outcome-unknown", "Disconnected", null));
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions({ submitMessage }), onMutation: vi.fn() });
    const draft = "    indented code\n\n**bold**\n";
    const field = screen.getByLabelText("Message");
    fire.input(field, { target: { value: draft } });
    fire.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("alert");
    expect(submitMessage).toHaveBeenLastCalledWith(expect.objectContaining({ text: draft }));
    expect(field).toHaveValue(draft);
    await waitFor(() => expect(screen.getByRole("button", { name: "Retry send" })).toBeEnabled());
    fire.click(screen.getByRole("button", { name: "Retry send" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Retry send" })).toBeEnabled());
    expect(submitMessage.mock.calls[1]![0].commandId).toBe(submitMessage.mock.calls[0]![0].commandId);
    fire.input(field, { target: { value: `${draft}\n` } });
    fire.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(submitMessage).toHaveBeenCalledTimes(3));
    expect(submitMessage.mock.calls[2]![0].commandId).not.toBe(submitMessage.mock.calls[0]![0].commandId);
    expect(submitMessage.mock.calls[2]![0].text).toBe(`${draft}\n`);
  });

  it.each([false, true])("clears accepted raw text and restores edit focus after %s steering", async (steering) => {
    const api = actions();
    renderComposer({ conversation: conversation(steering ? { currentRunId: "run", runStatus: "running", provider: "codex" } : {}), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    const field = screen.getByLabelText("Message");
    const draft = "    code\n\n**direction**\n";
    fire.input(field, { target: { value: draft } });
    const button = screen.getByRole("button", { name: steering ? "Steer Codex" : "Send" });
    button.focus();
    fire.click(button);
    await waitFor(() => expect(steering ? api.steerRun : api.submitMessage).toHaveBeenCalledWith(expect.objectContaining({ text: draft })));
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveValue("");
  });

  it("does not retry accepted text when refreshing fails", async () => {
    const api = actions();
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn().mockRejectedValue(new BridgeError("outcome-unknown", "Refresh failed", null)) });
    fire.input(screen.getByLabelText("Message"), { target: { value: "accepted" } });
    fire.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("alert");
    expect(screen.getByLabelText("Message")).toHaveValue("");
    expect(screen.queryByRole("button", { name: "Retry send" })).not.toBeInTheDocument();
  });

  it("previews safe Markdown and returns to the exact source selection without submitting", () => {
    const api = actions();
    const view = renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    const field = screen.getByLabelText<HTMLTextAreaElement>("Message");
    const draft = "**bold**\n\n- item\n\n```js\nalert('code')\n```\n\n![remote](https://remote.invalid/image.png) [unsafe](javascript:alert%281%29)\n";
    fire.input(field, { target: { value: draft } });
    field.focus();
    field.setSelectionRange(2, 6, "backward");
    fire.click(screen.getByRole("button", { name: "Preview Markdown" }));
    const preview = screen.getByRole("region", { name: "Message preview" });
    expect(within(preview).getByText("bold").tagName).toBe("STRONG");
    expect(within(preview).getByRole("listitem")).toHaveTextContent("item");
    expect(within(preview).getByRole("button", { name: "Copy code" })).toBeVisible();
    expect(view.container.querySelector("img, script, iframe")).toBeNull();
    expect(within(preview).queryByRole("link")).not.toBeInTheDocument();
    fire.click(screen.getByRole("button", { name: "Edit Markdown" }));
    expect(screen.getByLabelText("Message")).toHaveValue(draft);
    expect(field).toHaveFocus();
    expect([field.selectionStart, field.selectionEnd, field.selectionDirection]).toEqual([2, 6, "backward"]);
    expect(api.submitMessage).not.toHaveBeenCalled();
    expect(api.steerRun).not.toHaveBeenCalled();
  });

  it("resizes to measured content after editing and shrinking", () => {
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    const field = screen.getByLabelText("Message");
    expect(field).toHaveAttribute("rows", "2");
    Object.defineProperty(field, "scrollHeight", { configurable: true, value: 160 });
    fire.input(field, { target: { value: "long\n".repeat(8) } });
    expect(parseFloat(field.style.height)).toBeGreaterThanOrEqual(160);
    Object.defineProperty(field, "scrollHeight", { configurable: true, value: 48 });
    fire.input(field, { target: { value: "" } });
    expect(parseFloat(field.style.height)).toBeLessThan(160);
  });

  it("defers width-driven resizing outside observer delivery and cancels pending work on unmount", () => {
    let notifyResize!: ResizeObserverCallback;
    let resizeFrame!: FrameRequestCallback;
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: ResizeObserverCallback) { notifyResize = callback; }
      observe() {}
      disconnect = disconnect;
    });
    const requestFrame = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      resizeFrame = callback;
      return 1;
    });
    const cancelFrame = vi.spyOn(window, "cancelAnimationFrame");
    try {
      const view = renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
      const field = screen.getByLabelText("Message");
      Object.defineProperty(field, "scrollHeight", { configurable: true, value: 160 });
      fire.input(field, { target: { value: "wrapped draft" } });
      const previousHeight = field.style.height;
      Object.defineProperty(field, "clientWidth", { configurable: true, value: 700 });
      Object.defineProperty(field, "scrollHeight", { configurable: true, value: 80 });
      act(() => notifyResize([], {} as ResizeObserver));
      expect(field.style.height).toBe(previousHeight);
      Object.defineProperty(field, "clientWidth", { configurable: true, value: 400 });
      Object.defineProperty(field, "scrollHeight", { configurable: true, value: 200 });
      act(() => notifyResize([], {} as ResizeObserver));
      expect(requestFrame).toHaveBeenCalledTimes(1);
      act(() => resizeFrame(0));
      expect(parseFloat(field.style.height)).toBeGreaterThanOrEqual(200);
      Object.defineProperty(field, "clientWidth", { configurable: true, value: 300 });
      act(() => notifyResize([], {} as ResizeObserver));
      expect(requestFrame).toHaveBeenCalledTimes(2);
      disconnect.mockClear();
      view.unmount();
      expect(disconnect).toHaveBeenCalledTimes(1);
      expect(cancelFrame).toHaveBeenCalledWith(1);
    } finally {
      requestFrame.mockRestore();
      cancelFrame.mockRestore();
      vi.unstubAllGlobals();
    }
  });

  it("retains a failed steer and excludes duplicate steering before React updates", async () => {
    let reject!: (reason: Error) => void;
    const steerRun = vi.fn(() => new Promise<void>((_resolve, fail) => { reject = fail; }));
    renderComposer({ conversation: conversation({ currentRunId: "run", runStatus: "running", provider: "codex" }), providers: providers, routingProfile: "balanced", actions: actions({ steerRun }), onMutation: vi.fn() });
    const field = screen.getByLabelText("Message");
    const draft = "    exact direction\n";
    fire.input(field, { target: { value: draft } });
    act(() => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    expect(steerRun).toHaveBeenCalledExactlyOnceWith({ conversationId: "conversation-1", runId: "run", text: draft });
    expect(field).toHaveValue(draft);
    await act(async () => reject(new Error("Steer rejected")));
    expect(screen.getByRole("alert")).toHaveTextContent("Steer rejected");
    expect(field).toHaveValue(draft);
    expect(screen.getByRole("button", { name: "Steer Codex" })).toBeEnabled();
  });

  it("does not move focus to another conversation after an old send finishes", async () => {
    let finish!: (value: Awaited<ReturnType<ConversationActions["submitMessage"]>>) => void;
    const submitMessage = vi.fn(() => new Promise<Awaited<ReturnType<ConversationActions["submitMessage"]>>>((resolve) => { finish = resolve; }));
    const onMutation = vi.fn();
    const api = actions({ submitMessage });
    const view = renderComposer({ key: "old", conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: onMutation });
    fire.input(screen.getByLabelText("Message"), { target: { value: "old draft" } });
    fire.click(screen.getByRole("button", { name: "Send" }));
    view.rerender({ key: "new", conversation: conversation({ id: "conversation-2" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: onMutation });
    const provider = screen.getByRole("combobox", { name: "Provider" });
    provider.focus();
    await act(async () => finish({ runId: "run", status: "queued", provider: "codex", duplicate: false, routingExplanation: "test", thinkingDecision: { preference: { kind: "auto" }, requestedEffort: "high", reason: "fixture" } }));
    expect(provider).toHaveFocus();
  });
  it("can interrupt steerable Codex when the other provider is unavailable", async () => {
    const api = actions();
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: [providers[0]!, { ...providers[1]!, available: false }], routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    expect(screen.getByRole("button", { name: "Steer Codex" })).toBeVisible();
    fire.click(screen.getByRole("button", { name: "Interrupt Codex" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Interrupt Codex" });
    expect(api.interruptRun).not.toHaveBeenCalled();
    fire.click(within(dialog).getByRole("button", { name: "Interrupt Codex" }));
    await waitFor(() => expect(api.interruptRun).toHaveBeenCalledExactlyOnceWith({ conversationId: "conversation-1", runId: "run-1" }));
  });

  it.each([
    { currentRunId: "run-2" },
    { provider: "claude" as const },
    { runStatus: "completed" as const, rollupStatus: "completed" as const },
  ])("dismisses an explicit interruption after its run changes: %j", async (change) => {
    const api = actions();
    const active = conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" });
    const view = renderComposer({ conversation: active, providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.click(screen.getByRole("button", { name: "Interrupt Codex" }));
    const confirmation = within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Interrupt Codex" });
    view.rerender({ conversation: { ...active, ...change }, providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    fire.click(confirmation);
    expect(api.interruptRun).not.toHaveBeenCalled();
  });

  it("exposes in-progress work and restores focus after a successful interrupt switch commit", async () => {
    let finishInterrupt!: () => void;
    const interruptRun = vi.fn(() => new Promise<void>((resolve) => { finishInterrupt = resolve; }));
    const api = actions({ interruptRun });
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    fire.click(await screen.findByRole("button", { name: "Interrupt and switch to Claude" }));
    expect(screen.getByRole("region", { name: "Message composer" })).toHaveAttribute("aria-busy", "true");
    await act(async () => finishInterrupt());
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(screen.getByRole("region", { name: "Message composer" })).toHaveAttribute("aria-busy", "false");
  });

  it("returns from preview before restoring fallback focus after interruption", async () => {
    const api = actions();
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "**keep this draft**" } });
    fire.click(screen.getByRole("button", { name: "Preview Markdown" }));
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    fire.click(await screen.findByRole("button", { name: "Interrupt and switch to Claude" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Message preview" })).not.toBeInTheDocument());
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus();
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("**keep this draft**");
  });

  it("shows Auto with its active profile and excludes unavailable manual routes", () => {
    renderComposer({ conversation: conversation(), providers: [providers[0]!, { ...providers[1]!, available: false, diagnostic: "Sign in first" }], routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    expect(screen.getByRole("option", { name: "Auto · Balanced" })).toBeVisible();
    expect(screen.getByRole("option", { name: /Claude — Sign in first/ })).toBeDisabled();
  });

  it("reuses one command ID when retrying a failed logical send", async () => {
    const submitMessage = vi.fn()
      .mockRejectedValueOnce(new BridgeError("outcome-unknown", "transport disconnected", null))
      .mockResolvedValueOnce({ runId: "run-2", status: "queued", provider: "codex", duplicate: true, routingExplanation: "Continuity" });
    const api = actions({ submitMessage });
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "Run tests" } });
    fire.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("transport disconnected");
    fire.click(screen.getByRole("button", { name: "Retry send" }));
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
    renderComposer({ conversation: conversation(), providers: providers, routingProfile: "balanced", actions: actions({ submitMessage }), onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "Run tests" } });
    fire.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Request rejected");
    expect(screen.getByRole("button", { name: "Send" })).toBeVisible();
    fire.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(submitMessage).toHaveBeenCalledTimes(2));
    expect(submitMessage.mock.calls[0]?.[0].commandId).not.toBe(submitMessage.mock.calls[1]?.[0].commandId);
  });

  it("requires explicit interruption before switching an active provider", async () => {
    const api = actions();
    const view = renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    const dialog = await screen.findByRole("alertdialog", { name: "Interrupt Codex to switch provider" });
    expect(dialog).toBeVisible();
    expect(api.interruptRun).not.toHaveBeenCalled();
    expect(api.submitMessage).not.toHaveBeenCalled();
    fire.click(await screen.findByRole("button", { name: "Interrupt and switch to Claude" }));
    await waitFor(() => expect(api.interruptRun).toHaveBeenCalledWith({ conversationId: "conversation-1", runId: "run-1" }));
    await act(async () => {});
    expect(screen.getByRole("combobox", { name: "Provider" })).toHaveValue("claude");
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeDisabled();
    expect(screen.getByText(/Waiting for Codex to stop/)).toBeVisible();

    view.rerender({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "interrupted", rollupStatus: "interrupted" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeEnabled();
  });

  it("portals the interrupt modal outside an inert composer surface and restores interaction on close", async () => {
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    const composer = screen.getByRole("region", { name: "Message composer" });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    const dialog = await screen.findByRole("alertdialog", { name: "Interrupt Codex to switch provider" });

    expect(composer).toHaveAttribute("inert");
    expect(composer).not.toContainElement(dialog);
    fire.click(within(dialog).getByRole("button", { name: "Keep Codex running" }));
    await waitFor(() => expect(composer).not.toHaveAttribute("inert"));
  });

  it("renders interrupt failure feedback inside the active dialog", async () => {
    const api = actions({ interruptRun: vi.fn().mockRejectedValue(new Error("Interrupt failed")) });
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    const dialog = await screen.findByRole("alertdialog", { name: "Interrupt Codex to switch provider" });
    fire.click(within(dialog).getByRole("button", { name: "Interrupt and switch to Claude" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Interrupt failed");
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });

  it("shows a Thinking save failure after a prior interruption failure", async () => {
    const api = actions({ interruptRun: vi.fn().mockRejectedValue(new Error("Interrupt failed")) });
    saveThinking = vi.fn().mockRejectedValue(new Error("Thinking save failed"));
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "keep draft" } });
    fire.click(screen.getByRole("button", { name: "Interrupt Codex" }));
    fire.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Interrupt Codex" }));
    expect(await within(await screen.findByRole("alertdialog")).findByRole("alert")).toHaveTextContent("Interrupt failed");
    fire.click(await screen.findByRole("button", { name: "Keep Codex running" }));
    fire.change(screen.getByRole("combobox", { name: "Thinking" }), { target: { value: "manual:high" } });
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("Thinking save failed"));
    expect(screen.queryByText("Interrupt failed")).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Thinking" })).toHaveValue("auto");
    expect(screen.getByLabelText("Message")).toHaveValue("keep draft");
  });

  it("closes a pending provider switch when the active run becomes terminal", async () => {
    const api = actions();
    const view = renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    expect(await screen.findByRole("alertdialog")).toBeVisible();

    view.rerender({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "completed", rollupStatus: "completed" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Provider" })).toHaveFocus());
    expect(api.interruptRun).not.toHaveBeenCalled();
  });

  it("does not let a stale switch dialog interrupt a replacement run", async () => {
    const api = actions();
    const view = renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    expect(await screen.findByRole("alertdialog")).toBeVisible();

    view.rerender({ conversation: conversation({ currentRunId: "run-2", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Provider" })).toHaveFocus());
    expect(api.interruptRun).not.toHaveBeenCalled();
  });

  it("revalidates target availability before interrupting", async () => {
    const api = actions();
    const activeConversation = conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" });
    const view = renderComposer({ conversation: activeConversation, providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    expect(await screen.findByRole("alertdialog")).toBeVisible();

    view.rerender({ conversation: activeConversation, providers: [providers[0]!, { ...providers[1]!, available: false, diagnostic: "Sign in again" }], routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.click(await screen.findByRole("button", { name: "Interrupt and switch to Claude" }));

    expect(api.interruptRun).not.toHaveBeenCalled();
    expect(await screen.findByRole("alert")).toHaveTextContent("Claude is unavailable: Sign in again");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(screen.getByRole("combobox", { name: "Provider" })).toHaveValue("auto");
  });

  it("restores focus to an enabled composer control when descendants remain active", async () => {
    const api = actions();
    const view = renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });

    view.rerender({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "completed", rollupStatus: "active" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });

    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeDisabled();
  });

  it("closes the switch confirmation with Escape and restores provider focus", async () => {
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    const select = screen.getByRole("combobox", { name: "Provider" });
    fire.change(select, { target: { value: "claude" } });
    fire.keyDown(await screen.findByRole("alertdialog"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    await waitFor(() => expect(select).toHaveFocus());
  });

  it("steers only when the active provider reports steering", async () => {
    const api = actions();
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: providers, routingProfile: "balanced", actions: api, onMutation: vi.fn() });
    fire.input(screen.getByLabelText("Message"), { target: { value: "Focus on tests" } });
    fire.click(screen.getByRole("button", { name: "Steer Codex" }));
    await waitFor(() => expect(api.steerRun).toHaveBeenCalledWith({ conversationId: "conversation-1", runId: "run-1", text: "Focus on tests" }));
  });

  it("explains why a non-steerable active run cannot accept another message", () => {
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "claude", runStatus: "running" }), providers: providers, routingProfile: "usageBalance", actions: actions(), onMutation: vi.fn() });
    expect(screen.getByText(/Claude cannot be steered/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("does not offer interruption when the active adapter lacks that capability", async () => {
    const noInterrupt = providers.map((provider) => provider.id === "codex"
      ? { ...provider, capabilities: ["steering"] as ProviderInstallation["capabilities"] }
      : provider);
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "running" }), providers: noInterrupt, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    fire.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    const dialog = await screen.findByRole("alertdialog", { name: "Interrupt Codex to switch provider" });
    expect(within(dialog).getByText(/does not report interruption support/i)).toBeVisible();
    expect(await screen.findByRole("button", { name: "Interruption unavailable" })).toBeDisabled();
  });

  it("treats active descendants as an active turn boundary", () => {
    renderComposer({ conversation: conversation({ currentRunId: "run-1", provider: "codex", runStatus: "completed", rollupStatus: "active" }), providers: providers, routingProfile: "balanced", actions: actions(), onMutation: vi.fn() });
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeDisabled();
    expect(screen.getByText(/Active child conversations must finish/i)).toBeVisible();
  });
});
