import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "../../app/App";
import { createAppStore, type AppApi } from "../../app/store";
import type { ConversationSummary } from "../../bridge/types";

const originalScrollIntoView = Object.getOwnPropertyDescriptor(Element.prototype, "scrollIntoView");
afterEach(() => {
  vi.unstubAllGlobals();
  if (originalScrollIntoView) Object.defineProperty(Element.prototype, "scrollIntoView", originalScrollIntoView);
  else Reflect.deleteProperty(Element.prototype, "scrollIntoView");
});

it("navigates a known descendant from palette to parent and root without changing the draft or sending", async () => {
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
  Object.defineProperty(Element.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  const root: ConversationSummary = {
    thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
    id: "root", parentId: null, title: "Compiler", hasChildren: true, summary: null,
    capabilities: { canSend: true, canInterrupt: true, canArchive: true, canRoute: true, unavailableReason: null },
    routingProfile: "balanced", workspaceId: null, archived: false, projectRoot: null,
    currentRunId: "run", provider: "codex", runStatus: "running", rollupStatus: "active",
  };
  const child: ConversationSummary = { ...root, id: "child", parentId: "root", title: "Reviewer",
    capabilities: { canSend: false, canInterrupt: false, canArchive: false, canRoute: false, unavailableReason: "Recorded activity only." } };
  const grandchild: ConversationSummary = { ...child, id: "grandchild", parentId: "child", title: "Researcher", hasChildren: false };
  const nodes: Record<string, ConversationSummary> = { root, child, grandchild };
  const api: AppApi = {
    setThinkingPreference: vi.fn(),
    getBootstrap: vi.fn().mockResolvedValue({ providers: [], startupDiagnostic: null }),
    listConversations: vi.fn().mockResolvedValue({ items: [root], nextCursor: null }),
    loadConversation: vi.fn(({ conversationId }) => Promise.resolve(nodes[conversationId]!)),
    listChildConversations: vi.fn(({ parentId }) => Promise.resolve({ items: parentId === "root" ? [child] : [grandchild], nextCursor: null })),
    loadConversationPath: vi.fn(({ conversationId }) => Promise.resolve({ items: conversationId === "grandchild" ? [root, child, grandchild] : [root, child], truncated: false, ownerConversationId: "root" })),
    listenToAppEvents: vi.fn().mockResolvedValue(() => {}),
    loadTimeline: vi.fn().mockResolvedValue({ items: [], nextCursor: null, approvals: [], approvalsTruncated: false, approvalsNextCursor: null }),
    loadApprovals: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadDiagnostics: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadEventDetail: vi.fn(), loadApprovalDetail: vi.fn(), loadApprovalQuestions: vi.fn(),
    submitMessage: vi.fn(), steerRun: vi.fn(), interruptRun: vi.fn(), respondToApproval: vi.fn(),
    inspectWorkspace: vi.fn(), inspectProject: vi.fn(), pickProjectDirectory: vi.fn(),
    createConversation: vi.fn(), archiveConversation: vi.fn(), listRunAudits: vi.fn(), loadRunAudit: vi.fn(),
  };
  const store = createAppStore(api);
  const view = render(<App store={store} />);
  const message = await screen.findByRole("textbox", { name: "Message" });
  fireEvent.change(message, { target: { value: "Keep **draft**" } });
  await act(async () => { await store.loadChildPage("root", true); await store.loadChildPage("child", true); });
  async function openPalette() {
    fireEvent.click(screen.getByRole("button", { name: /Search/ }));
    return screen.findByRole("combobox", { name: "Search conversations and commands" });
  }
  const search = await openPalette();
  expect(screen.getByRole("option", { name: "Go to parent conversation" })).toHaveAttribute("aria-disabled", "true");
  fireEvent.change(search, { target: { value: "Researcher" } });
  fireEvent.click(screen.getByRole("option", { name: /Researcher.*Reviewer/ }));
  await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("grandchild"));
  expect(screen.queryByRole("textbox", { name: "Message" })).not.toBeInTheDocument();
  await openPalette();
  fireEvent.click(screen.getByRole("option", { name: "Go to parent conversation" }));
  await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("child"));
  fireEvent.click(within(screen.getByRole("navigation", { name: "Conversation ancestry" })).getByRole("button", { name: "Compiler" }));
  await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("root"));
  expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("Keep **draft**");
  expect(api.submitMessage).not.toHaveBeenCalled();
  expect(api.steerRun).not.toHaveBeenCalled();
  expect(api.interruptRun).not.toHaveBeenCalled();
  view.unmount();
  store.dispose();
});
