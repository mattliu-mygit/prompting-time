import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "../../app/App";
import { createAppStore, type AppApi } from "../../app/store";
import type { ConversationSummary } from "../../bridge/types";

afterEach(() => { vi.unstubAllGlobals(); Reflect.deleteProperty(Element.prototype, "scrollIntoView"); });

it("navigates palette, parent and root through App without changing draft or sending", async () => {
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
  Object.defineProperty(Element.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  const conversation: ConversationSummary = {
    id: "c", title: "Compiler", routingProfile: "bestFit", workspaceId: null, archived: false, projectRoot: null,
    currentRunId: "run", provider: "codex", runStatus: "running", rollupStatus: "active", agentsTruncated: false,
    agents: [
      { id: "root", parentId: null, label: "Root", status: "running", provider: "codex", summary: null },
      { id: "child", parentId: "root", label: "Reviewer", status: "running", provider: "codex", summary: "Checking" },
      { id: "grandchild", parentId: "child", label: "Researcher", status: "completed", provider: "claude", summary: "Checked schema" },
    ],
  };
  const api = {
    getBootstrap: vi.fn().mockResolvedValue({ providers: [] }),
    listConversations: vi.fn().mockResolvedValue({ items: [conversation], nextCursor: null }),
    listenToAppEvents: vi.fn().mockResolvedValue(() => {}),
    loadTimeline: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadApprovals: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    submitMessage: vi.fn(), steerRun: vi.fn(),
  } as unknown as AppApi;
  const store = createAppStore(api);
  render(<App store={store} />);
  const message = await screen.findByRole("textbox", { name: "Message" });
  fireEvent.change(message, { target: { value: "Keep **draft**" } });
  async function openPalette() {
    fireEvent.click(screen.getByRole("button", { name: /Search/ }));
    return screen.findByRole("combobox");
  }
  await openPalette();
  expect(screen.getByRole("option", { name: "Go to parent agent" })).toHaveAttribute("aria-disabled", "true");
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "Researcher" } });
  fireEvent.click(screen.getByRole("option", { name: "Researcher Compiler" }));
  await waitFor(() => expect(store.getSnapshot().selectedAgentId).toBe("grandchild"));
  expect(screen.getByRole("region", { name: "Selected agent" })).toHaveTextContent("Checked schema");
  await openPalette();
  fireEvent.click(screen.getByRole("option", { name: "Go to parent agent" }));
  await waitFor(() => expect(store.getSnapshot().selectedAgentId).toBe("child"));
  fireEvent.click(within(screen.getByRole("navigation", { name: "Agent ancestry" })).getByRole("button", { name: "Root" }));
  await openPalette();
  expect(screen.getByRole("option", { name: "Go to parent agent" })).toHaveAttribute("aria-disabled", "true");
  fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  fireEvent.click(within(screen.getByRole("navigation", { name: "Agent ancestry" })).getByRole("button", { name: "Compiler" }));
  expect(store.getSnapshot().selectedAgentId).toBeNull();
  expect(message).toHaveValue("Keep **draft**");
  expect(api.submitMessage).not.toHaveBeenCalled();
  expect(api.steerRun).not.toHaveBeenCalled();
  store.dispose();
});
