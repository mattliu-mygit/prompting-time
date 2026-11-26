import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BridgeError } from "../bridge/api";
import type { AppEvent, ConversationSummary, ListChildConversationsRequest, LoadConversationRequest, MenuBarCommand, PluginActionRequest, TimelineItem } from "../bridge/types";
import { axeViolations, render, settle } from "../test/render";
import { App } from "./App";
import { createAppStore, type AppApi, type AppStore } from "./store";
import { actionResult, fixturePlugin, fixtureReviews, fixtureSettings, fixtureShown, fixtureStatus, fixtureV2Plugin, fixtureV2Status, noPlugins, pageActions } from "../test/plugins";

const click = (element: Element) => {
  fireEvent.click(element);
  flush();
};
const press = (key: string, init: KeyboardEventInit = {}) => {
  fireEvent.keyDown(window, { key, metaKey: true, ...init });
  flush();
};
const narrowQuery = (query: string) => query === "(max-width: 72rem)";
let stores: AppStore[] = [];

function mediaQueries(matches: (query: string) => boolean = () => false) {
  vi.stubGlobal("matchMedia", vi.fn((query: string) => ({
    matches: matches(query), media: query, onchange: null,
    addListener: vi.fn(), removeListener: vi.fn(), addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: vi.fn(),
  })));
}

/** Renders the app over a store built from `api`; the store is disposed after the test. */
function renderApp(api: AppApi) {
  const store = createAppStore(api);
  stores.push(store);
  const view = render(() => <App store={store} />);
  return { store, ...view };
}

/** The chat sections of these tests' conversations, as a past launch left them open; Needs you is open by default. */
const OPEN_CHATS = { "chats:no-folder": true, "chats:/tmp/synthetic-project": true, "chats:/tmp/other": true, "chats:/projects/compiler": true, "chats:/projects/renderer": true };

beforeEach(() => {
  window.localStorage.setItem("prompting-time.sidebar-sections", JSON.stringify(OPEN_CHATS));
  mediaQueries();
  Object.defineProperty(Element.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
});
afterEach(() => {
  stores.forEach((store) => store.dispose());
  stores = [];
  vi.unstubAllGlobals();
  Reflect.deleteProperty(Element.prototype, "scrollIntoView");
});

describe("conversation hierarchy", () => {
  function hierarchyApi() {
    const root = createdConversation({ id: "compiler", title: "Compiler", hasChildren: true,
      currentRunId: "run-compiler", provider: "codex", runStatus: "completed", rollupStatus: "completed" });
    const unavailableReason = "This provider exposes recorded child activity, not an independently controllable chat.";
    const child = createdConversation({ ...root, id: "reviewer", parentId: root.id, title: "Parser reviewer",
      summary: "Checked parser ownership", capabilities: { canSend: false, canInterrupt: false,
        canArchive: false, canRoute: false, unavailableReason } });
    const grandchild = createdConversation({ ...child, id: "researcher", parentId: child.id,
      title: "Schema researcher", hasChildren: false });
    const nodes = [root, child, grandchild];
    const captured = new Map(nodes.map(node => [node.id, `Captured ${node.id} activity`]));
    const api = {
      ...createApi({
        getBootstrap: vi.fn().mockResolvedValue({ providers: [
          { id: "codex", installed: true, available: true, version: "1", diagnostic: null, capabilities: ["steering", "interruption"] },
          { id: "claude", installed: true, available: true, version: "2", diagnostic: null, capabilities: ["interruption"] },
        ] }),
        listConversations: vi.fn().mockResolvedValue({ items: [root], nextCursor: null }),
        loadConversation: vi.fn(async ({ conversationId }: LoadConversationRequest) => {
          const node = nodes.find(item => item.id === conversationId);
          if (!node) throw new Error("Conversation unavailable");
          return node;
        }),
        loadTimeline: vi.fn(async ({ conversationId }) => {
          const content = captured.get(conversationId)!;
          const item: TimelineItem = { id: `event-${conversationId}`, conversationId,
            runId: root.currentRunId!, agentId: `execution-${conversationId}`, sequence: "1",
            kind: "message", presentation: "normal", role: "assistant", content,
            contentBytes: String(content.length), truncated: false, provider: "codex", operation: null };
          return { items: [item], nextCursor: null, approvals: [], approvalsTruncated: false, activeCompaction: null, approvalsNextCursor: null };
        }),
      }),
      listChildConversations: vi.fn(async ({ parentId }: ListChildConversationsRequest) => ({
        items: nodes.filter(node => node.parentId === parentId), nextCursor: null,
      })),
      loadConversationPath: vi.fn(async ({ conversationId }: LoadConversationRequest) => ({
        items: nodes.slice(0, nodes.findIndex(node => node.id === conversationId) + 1),
        truncated: false, ownerConversationId: root.id,
      })),
    };
    return { api, root, child, grandchild, unavailableReason };
  }

  it("opens root, child, and grandchild through the same conversation timeline", async () => {
    const { api } = hierarchyApi();
    const { store } = renderApp(api);
    await screen.findByText("Captured compiler activity");
    click(await screen.findByRole("treeitem", { name: /Parser reviewer/ }));
    await screen.findByText("Captured reviewer activity");
    expect(store.getSnapshot().selectedConversationId).toBe("reviewer");
    expect(screen.queryByText("Captured compiler activity")).not.toBeInTheDocument();
    expect(screen.queryByRole("treeitem", { name: /Schema researcher/ })).not.toBeInTheDocument();
    click(screen.getByRole("button", { name: "Expand Parser reviewer" }));
    click(await screen.findByRole("treeitem", { name: /Schema researcher/ }));
    await screen.findByText("Captured researcher activity");
    expect(screen.queryByText("Captured reviewer activity")).not.toBeInTheDocument();
    expect(vi.mocked(api.loadTimeline).mock.calls.map(([request]) => request.conversationId)).toEqual(["compiler", "reviewer", "researcher"]);
  });

  it("goes back up through the ancestry and focuses the new timeline", async () => {
    const { api } = hierarchyApi();
    const { store } = renderApp(api);
    await screen.findByText("Captured compiler activity");
    await store.selectConversation("researcher");
    await settle();
    const ancestry = await screen.findByRole("navigation", { name: "Conversation ancestry" });
    click(within(ancestry).getByRole("button", { name: "Parser reviewer" }));
    await screen.findByText("Captured reviewer activity");
    await waitFor(() => expect(screen.getByRole("heading", { name: "Timeline" })).toHaveFocus());
  });

  it("keeps the current conversation and explains a failed path lookup", async () => {
    const { api } = hierarchyApi();
    api.loadConversationPath.mockRejectedValue(new Error("Conversation no longer available"));
    const { store } = renderApp(api);
    await screen.findByText("Captured compiler activity");
    await store.selectConversation("missing-child");
    await settle();
    expect(screen.getByRole("alert")).toHaveTextContent("Conversation no longer available");
    expect(screen.getByText("Captured compiler activity")).toBeVisible();
  });
});

describe("All work and ⌘J", () => {
  const waitingApi = () => {
    const first = createdConversation({ id: "first", title: "Evaluation rows", provider: "codex", runStatus: "waiting", rollupStatus: "needsAttention", currentRunId: "run-first" });
    const working = createdConversation({ id: "working", title: "Parser cleanup", provider: "claude", runStatus: "running", rollupStatus: "active", currentRunId: "run-working" });
    const second = createdConversation({ id: "second", title: "Tag names", projectRoot: "/tmp/other", provider: "codex", runStatus: "running", rollupStatus: "needsAttention", currentRunId: "run-second" });
    return createApi({ listConversations: vi.fn().mockResolvedValue({ items: [working, first, second], nextCursor: null }) });
  };

  it("lists roots waiting on you first under Needs you, then by project", async () => {
    renderApp(waitingApi());
    const needsYou = await screen.findByRole("tree", { name: "Chats waiting on you" });
    expect(within(needsYou).getAllByRole("treeitem").map((row) => row.getAttribute("aria-label"))).toEqual([
      "Evaluation rows, Codex, waiting for you", "Tag names, Codex, waiting for you",
    ]);
    expect(screen.getAllByRole("tree").map((tree) => tree.getAttribute("aria-labelledby") && document.getElementById(tree.getAttribute("aria-labelledby")!)?.textContent))
      .toEqual(["Chats waiting on you", "Chats · synthetic-project"]);
  });

  it("jumps to the next root waiting on you with ⌘J or Ctrl+J, wrapping around", async () => {
    const { store } = renderApp(waitingApi());
    await screen.findByRole("tree", { name: "Chats waiting on you" });
    expect(store.getSnapshot().selectedConversationId).toBe("working");
    press("j");
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("first"));
    press("j", { metaKey: false, ctrlKey: true });
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("second"));
    press("j");
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("first"));
    press("j", { repeat: true });
    press("j", { isComposing: true });
    await settle();
    expect(store.getSnapshot().selectedConversationId).toBe("first");
  });

  it("scrolls the row ⌘J selects into view when it is outside the visible part of its section", async () => {
    const { store } = renderApp(waitingApi());
    await screen.findByRole("tree", { name: "Chats waiting on you" });
    const reveal = vi.mocked(Element.prototype.scrollIntoView);
    reveal.mockClear();
    press("j");
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("first"));
    await settle();
    // The browser scrolls the row's section only when the row is not already in view.
    expect(reveal).toHaveBeenCalledWith({ block: "nearest" });
    expect(reveal.mock.contexts.at(-1)).toBe(screen.getByRole("treeitem", { name: "Evaluation rows, Codex, waiting for you" }));
  });

  it("does nothing on ⌘J when nothing waits on you", async () => {
    const { store } = renderApp(createApi());
    await screen.findByRole("treeitem", { name: /Auth refactor/ });
    press("j");
    await settle();
    expect(store.getSnapshot().selectedConversationId).toBe("c1");
  });
});

describe("plugin pages", () => {
  const OPEN = "20261126T150000Z-101";
  const SUBMITTED = "20261125T100000Z-77";
  const FAILED = "20261124T090000Z-55";
  const request = JSON.stringify({ event: "APPROVE", body: "", comments: [] }, null, 2);
  const fingerprint = "9c".repeat(32);

  /** Whooper with invented reviews; its page's actions answer as Whooper's commands would. */
  function pagesApi() {
    const pluginRequest = vi.fn(async (sent: PluginActionRequest) => {
      switch (sent.action) {
        case "status": return actionResult({ stdout: JSON.stringify(fixtureStatus("2026-11-26T23:45:00Z", { reviews: fixtureReviews() })) });
        case "show-review": return actionResult({ stdout: JSON.stringify(fixtureShown(sent.args[0]!)) });
        case "submit": return sent.confirmedFingerprint === null
          ? actionResult({ stdout: `printed the request; nothing posted\nfingerprint: ${fingerprint}\n`, fingerprint, preview: request })
          : actionResult({ stdout: "posted the review: https://example.invalid/review/1\n" });
        default: return actionResult({ stdout: JSON.stringify(fixtureSettings) });
      }
    });
    return { pluginRequest, api: createApi({ listPlugins: vi.fn(async () => [fixturePlugin({ actions: pageActions })]), pluginRequest }) };
  }

  function frame() {
    const element = document.querySelector<HTMLIFrameElement>("iframe.plugin-frame");
    if (!element?.contentWindow) throw new Error("no plugin frame");
    return { element, window: element.contentWindow, posted: vi.spyOn(element.contentWindow, "postMessage") };
  }

  async function fromPage(source: Window, data: unknown) {
    window.dispatchEvent(new MessageEvent("message", { data, source }));
    await settle();
    await settle();
  }

  it("opens a review from Needs you in the plugin's sandboxed page and submits it through the confirmation", async () => {
    const { api, pluginRequest } = pagesApi();
    renderApp(api);
    await settle();
    await settle();

    fireEvent.click(await screen.findByRole("treeitem", { name: /^separate cache configuration, widgets#101 · @sam-fixture · 2 findings/ }));
    await settle();
    const page = frame();
    expect(page.element).toHaveAttribute("sandbox", "allow-scripts");
    expect(page.element).toHaveAttribute("src", "pt-plugin://whooper/index.html");
    expect(screen.getByRole("heading", { level: 1, name: "separate cache configuration" })).toBeVisible();

    await fromPage(window, { version: 1, type: "ready", id: 1 });
    expect(page.posted).not.toHaveBeenCalled();
    await fromPage(page.window, { version: 1, type: "ready", id: 1 });
    expect(page.posted).toHaveBeenCalledWith(expect.objectContaining({ type: "status", reply_to: 1 }), "*");
    expect(page.posted).toHaveBeenCalledWith(expect.objectContaining({ type: "review", review: expect.objectContaining({ run_id: OPEN }) }), "*");

    await fromPage(page.window, { version: 1, type: "submit", id: 2, run_id: OPEN });
    const dialog = await screen.findByRole("alertdialog", { name: "Submit review" });
    expect(within(dialog).getByLabelText("What it will send")).toHaveTextContent('"event": "APPROVE"');
    expect(pluginRequest).not.toHaveBeenCalledWith(expect.objectContaining({ action: "submit", confirmedFingerprint: fingerprint }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Submit review" }));
    await settle();
    await settle();
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "submit", args: [OPEN], confirmedFingerprint: fingerprint });
    expect(page.posted).toHaveBeenLastCalledWith({ version: 1, type: "submit-result", reply_to: 2, run_id: OPEN, posted: true, message: "posted the review: https://example.invalid/review/1" }, "*");
    expect(screen.queryByRole("alertdialog", { name: "Submit review" })).not.toBeInTheDocument();
    // The next item in Needs you opens, and the notice names both.
    expect(document.querySelector(".app-notice")).toHaveTextContent("Submitted widgets#101 · now showing gadgets#55");
    expect(screen.getByRole("heading", { level: 1, name: "move the projection" })).toBeInTheDocument();
  });

  it("keeps the page loaded and sends it the next finished review the user opens", async () => {
    const { api } = pagesApi();
    renderApp(api);
    await settle();
    await settle();

    fireEvent.click(await screen.findByRole("treeitem", { name: /^separate cache configuration, widgets#101/ }));
    await settle();
    const page = frame();
    await fromPage(page.window, { version: 1, type: "ready", id: 1 });
    expect(page.posted).toHaveBeenCalledWith(expect.objectContaining({ type: "review", review: expect.objectContaining({ run_id: OPEN }) }), "*");

    // The page is ready, so opening another finished review sends it from the frame's effect.
    const search = screen.getByRole("searchbox", { name: "Search PRs and chats" });
    fireEvent.input(search, { target: { value: "#77" } });
    flush();
    fireEvent.keyDown(search, { key: "Enter" });
    flush();
    await settle();
    await settle();
    expect(screen.getByRole("heading", { level: 1, name: "add the tags package" })).toBeInTheDocument();
    expect(document.querySelector("iframe.plugin-frame")).toBe(page.element);
    expect(page.posted).toHaveBeenCalledWith(expect.objectContaining({ type: "review", review: expect.objectContaining({ run_id: SUBMITTED }) }), "*");
  });

  it("searches PRs and chats from the sidebar: Enter opens the first result, and Escape clears", async () => {
    const { api } = pagesApi();
    renderApp(api);
    await screen.findByRole("treeitem", { name: /^separate cache configuration/ });
    const search = screen.getByRole("searchbox", { name: "Search PRs and chats" });

    fireEvent.input(search, { target: { value: "#55" } });
    flush();
    const results = screen.getByRole("region", { name: "Search results" });
    expect(within(results).getAllByRole("treeitem").map((row) => row.getAttribute("aria-label"))).toEqual(["move the projection, gadgets#55 · review failed"]);
    expect(screen.queryByRole("button", { name: /^Needs you/ })).not.toBeInTheDocument();
    fireEvent.keyDown(search, { key: "Enter" });
    flush();
    await settle();
    expect(screen.getByRole("heading", { level: 1, name: "move the projection" })).toBeInTheDocument();

    fireEvent.input(search, { target: { value: "auth" } });
    flush();
    fireEvent.keyDown(search, { key: "Enter" });
    flush();
    await settle();
    expect(screen.getByRole("heading", { level: 1, name: "Auth refactor" })).toBeInTheDocument();

    fireEvent.keyDown(search, { key: "Escape" });
    flush();
    expect(search).toHaveValue("");
    expect(screen.queryByRole("region", { name: "Search results" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Needs you/ })).toHaveAttribute("aria-expanded", "true");
  });

  it("opens a failed review on Prompting Time's own screen instead of the page, and a finished one in the page", async () => {
    const { api, pluginRequest } = pagesApi();
    renderApp(api);
    await settle();
    await settle();

    fireEvent.click(await screen.findByRole("treeitem", { name: /^move the projection, gadgets#55 · review failed/ }));
    await settle();
    await settle();
    expect(document.querySelector("iframe.plugin-frame")).toBeNull();
    const failed = screen.getByRole("main", { name: "Review" });
    expect(within(failed).getByRole("heading", { name: "Review failed" })).toBeInTheDocument();
    expect(within(failed).getByLabelText("Why it failed")).toHaveTextContent("the review run exited 1 without findings; see runs/20261124T090000Z-55/stderr.log");
    expect(within(failed).getByRole("button", { name: "Retry review" })).toBeInTheDocument();
    expect(within(failed).getByRole("button", { name: "Ask why" })).toBeInTheDocument();
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "show-review", args: [FAILED], confirmedFingerprint: null });
    expect(screen.queryByText(/has no findings to show/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("treeitem", { name: /^separate cache configuration, widgets#101 · @sam-fixture · 2 findings/ }));
    await settle();
    expect(frame().element).toHaveAttribute("src", "pt-plugin://whooper/index.html");
    expect(screen.queryByRole("heading", { name: "Review failed" })).not.toBeInTheDocument();
  });

  it("lists every review on the agent's Runs tab and opens a past one with what was submitted", async () => {
    const { api } = pagesApi();
    renderApp(api);
    await settle();
    await settle();

    expect(screen.queryByRole("button", { name: /^Past reviews/ })).not.toBeInTheDocument();
    click(within(await screen.findByRole("region", { name: "Background agents" })).getByRole("button", { name: "Whooper" }));
    const runs = within(screen.getByRole("tabpanel", { name: "Runs" }));
    expect(runs.getAllByRole("row").slice(1).map((row) => row.querySelector("td:nth-child(2)")?.firstChild?.textContent))
      .toEqual(["Review widgets#101", "Review widgets#102", "Review gadgets#77", "Review widgets#101", "Review gadgets#55"]);

    fireEvent.click(runs.getByRole("button", { name: "Review gadgets#77" }));
    await settle();
    const page = frame();
    await fromPage(page.window, { version: 1, type: "ready", id: 1 });
    expect(page.posted).toHaveBeenCalledWith(expect.objectContaining({ type: "review", review: expect.objectContaining({ run_id: SUBMITTED }) }), "*");
    expect(page.posted).toHaveBeenLastCalledWith(expect.objectContaining({ type: "submit-result", run_id: SUBMITTED, posted: true, message: expect.stringMatching(/^commented /) }), "*");
  });
});

describe("background agents", () => {
  const nextPoll = () => {
    const next = new Date();
    next.setHours(23, 45, 0, 0);
    return next;
  };
  const shortTime = (date: Date) => date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

  /** A fake bridge with Whooper installed; `status` answers its status action. */
  function agentsApi(plugin = fixturePlugin(), status: () => unknown = () => fixtureStatus(nextPoll().toISOString())) {
    let handler: ((event: AppEvent) => void) | null = null;
    const api = createApi({
      listPlugins: vi.fn(async () => [plugin]),
      pluginRequest: vi.fn(async (request: PluginActionRequest) => request.action === "status"
        ? actionResult({ stdout: JSON.stringify(status()) })
        : actionResult({ stdout: JSON.stringify(fixtureSettings) })),
      listenToAppEvents: vi.fn(async (listener: (event: AppEvent) => void) => {
        handler = listener;
        return () => {};
      }),
    });
    return { api, emit: (event: AppEvent) => handler?.(event) };
  }

  it("pins each agent above the list, outside it, with its status in words", async () => {
    renderApp(agentsApi().api);
    const agents = await screen.findByRole("region", { name: "Background agents" });
    expect(await within(agents).findByText(`Running · next poll ${shortTime(nextPoll())}`)).toBeVisible();
    expect(within(agents).getByRole("button", { name: "Whooper" })).toHaveAccessibleDescription(`Running · next poll ${shortTime(nextPoll())}`);
    // All is well, so no dot: red, orange, and green each mean something else.
    expect(agents.querySelector(".agent-pin-status .pt-status-dot")).toHaveAttribute("data-status", "idle");
    const list = screen.getByRole("treeitem", { name: /Auth refactor/ });
    expect(agents.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The rows scroll inside their section; the agents and the search stay above every section.
    expect(list.closest(".work-section-body")).not.toBeNull();
    expect(agents.closest(".all-work")).toBeNull();
    expect(screen.getByRole("searchbox", { name: "Search PRs and chats" }).closest(".all-work")).toBeNull();
    expect(await axeViolations()).toEqual([]);
  });

  it("reads the agent again when a pluginChanged event arrives", async () => {
    let reviewing: string | null = null;
    const fake = agentsApi(fixturePlugin(), () => fixtureStatus(nextPoll().toISOString(),
      reviewing ? { round: { running: true, started_at: null, reviewing } } : {}));
    renderApp(fake.api);
    await screen.findByText(/^Running · next poll/);
    reviewing = "acme/widgets#101";
    fake.emit({ kind: "pluginChanged", sequence: "1", plugin: "whooper", files: ["round.lock/round.json"] });

    const words = await screen.findByText("Reviewing acme/widgets#101");
    expect(words.closest(".agent-pin-status")?.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "running");
  });

  it("shows what is missing on an agent that cannot start", async () => {
    const stopped = fixturePlugin({
      process: { kind: "notRunning", exit: null },
      prerequisites: [{ command: "slackdump", purpose: "Reads the review asks.", needed: true, path: null }],
    });
    const { api } = agentsApi(stopped);
    renderApp(api);
    const words = await screen.findByText("Missing slackdump");
    expect(words.closest(".agent-pin-status")?.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "error");
    expect(within(screen.getByRole("region", { name: "Background agents" })).getByRole("button", { name: "Start" })).toBeDisabled();
    expect(api.pluginRequest).not.toHaveBeenCalled();
  });

  it("opens an agent's screen from its row, its settings on a tab, and goes back to a conversation from All work", async () => {
    renderApp(agentsApi().api);
    const name = within(await screen.findByRole("region", { name: "Background agents" })).getByRole("button", { name: "Whooper" });
    click(name);

    expect(await screen.findByRole("heading", { name: "Whooper", level: 1 })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Runs" })).toHaveAttribute("aria-selected", "true");
    click(screen.getByRole("tab", { name: "Settings" }));
    expect(await screen.findByRole("spinbutton", { name: "Poll every" })).toHaveValue(15);
    expect(name).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("treeitem", { name: /Auth refactor/ })).toHaveAttribute("aria-selected", "false");
    click(screen.getByRole("button", { name: "Show inspector" }));
    expect(within(screen.getByRole("complementary", { name: "Inspector" })).getByText("Asks you first")).toBeInTheDocument();

    click(screen.getByRole("treeitem", { name: /Auth refactor/ }));
    await waitFor(() => expect(screen.queryByRole("spinbutton", { name: "Poll every" })).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { name: "Auth refactor", level: 1 })).toBeInTheDocument();
  });

  it("installs from a release archive only with its SHA-256, then opens the new agent", async () => {
    const { api } = agentsApi();
    const installed = fixturePlugin({ source: { kind: "archive", path: "/synthetic/release/whooper-0.1.0.tar.gz", sha256: "c".repeat(64) } });
    api.listPlugins = vi.fn().mockResolvedValue([]);
    api.pickPluginArchive = vi.fn().mockResolvedValue("/synthetic/release/whooper-0.1.0.tar.gz");
    api.installPlugin = vi.fn(async () => {
      api.listPlugins = vi.fn().mockResolvedValue([installed]);
      return installed;
    });
    renderApp(api);
    expect(await screen.findByText("No background agents")).toBeInTheDocument();
    click(screen.getByRole("button", { name: "Add background agent" }));
    click(await screen.findByRole("menuitem", { name: "Install from a release archive…" }));

    const dialog = await screen.findByRole("alertdialog", { name: "Install from a release archive" });
    expect(dialog).toHaveTextContent("/synthetic/release/whooper-0.1.0.tar.gz");
    const install = within(dialog).getByRole("button", { name: "Install" });
    expect(install).toBeDisabled();
    fireEvent.input(within(dialog).getByRole("textbox", { name: "SHA-256" }), { target: { value: ` ${"C".repeat(64)} ` } });
    flush();
    click(install);

    expect(await screen.findByRole("heading", { name: "Whooper", level: 1 })).toBeInTheDocument();
    expect(api.installPlugin).toHaveBeenCalledExactlyOnceWith({ source: { kind: "archive", path: "/synthetic/release/whooper-0.1.0.tar.gz", sha256: "c".repeat(64) } });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
  });

  it("runs a plugin folder in place from the palette once an agent is installed, and keeps a refused install in its dialog", async () => {
    const { api } = agentsApi();
    api.pickPluginFolder = vi.fn().mockResolvedValue("/synthetic/whooper");
    api.installPlugin = vi.fn().mockRejectedValue(new BridgeError("plugin-manifest-invalid", "The plugin's manifest is invalid: fixture reason", null));
    renderApp(api);
    await screen.findByRole("region", { name: "Background agents" });
    expect(screen.queryByRole("button", { name: "Add background agent" })).not.toBeInTheDocument();
    press("k");
    const search = await screen.findByRole("combobox", { name: "Search conversations, reviews, and commands" });
    fireEvent.input(search, { target: { value: "Run a plugin folder in place" } });
    flush();
    fireEvent.keyDown(search, { key: "Enter" });
    flush();
    const dialog = await screen.findByRole("alertdialog", { name: "Run a plugin folder in place" });
    click(within(dialog).getByRole("button", { name: "Install" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("The plugin's manifest is invalid: fixture reason");
    expect(api.installPlugin).toHaveBeenCalledExactlyOnceWith({ source: { kind: "folder", path: "/synthetic/whooper" } });
    click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
  });
});

describe("agent controls and blocking causes", () => {
  const docker = { id: "docker", summary: "Docker isn't running", scope: "all" };
  /** Whooper with a review in progress and two reviews waiting on Docker; its actions succeed. */
  function controlsApi() {
    const reviews = [
      ...fixtureReviews(),
      { ...fixtureReviews()[1]!, run_id: "waiting-60", repo: "acme/gadgets", pr: 60, title: "speed up the gadget index", outcome: "waiting", cause: docker },
      { ...fixtureReviews()[1]!, run_id: "waiting-61", repo: "acme/gadgets", pr: 61, title: "drop the legacy upload path", outcome: "waiting", cause: docker },
    ];
    const status = fixtureStatus("2026-11-26T23:45:00Z", { reviews, blocked: { ...docker, since: "2026-11-26T14:00:00Z", waiting: 2 } });
    const pluginRequest = vi.fn(async (sent: PluginActionRequest) => {
      switch (sent.action) {
        case "status": return actionResult({ stdout: JSON.stringify(status) });
        case "retry": return actionResult({ stdout: `queued ${sent.args[0]} for the next round\n` });
        case "round": return actionResult({ stdout: "polled: 2 retries\nround done\n" });
        case "pause": return actionResult({ stdout: "paused\n" });
        default: return actionResult({ stdout: JSON.stringify(fixtureSettings) });
      }
    });
    const plugin = fixturePlugin({ actions: [...pageActions, { name: "pause", title: "Pause", description: "Pauses the schedule.", confirm: false }, { name: "resume", title: "Resume", description: "Resumes the schedule.", confirm: false }] });
    const api = createApi({
      listPlugins: vi.fn(async () => [plugin]),
      pluginRequest,
      stopPlugin: vi.fn(async () => fixturePlugin({ process: { kind: "notRunning", exit: null } })),
    });
    return { api, pluginRequest };
  }

  it("runs Pause and Poll now from the agent's pinned row, and asks before Stop with the reviews it ends", async () => {
    const { api, pluginRequest } = controlsApi();
    renderApp(api);
    const agents = await screen.findByRole("region", { name: "Background agents" });
    const words = await within(agents).findByRole("button", { name: "Can't review · Docker isn't running" });
    expect(words.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "error");
    expect(agents.querySelector(".agent-pin")).toHaveAttribute("data-blocked");

    click(within(agents).getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "pause", args: [], confirmedFingerprint: null }));
    await waitFor(() => expect(within(agents).getByRole("button", { name: "Poll now" })).toBeEnabled());
    click(within(agents).getByRole("button", { name: "Poll now" }));
    await waitFor(() => expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "round", args: [], confirmedFingerprint: null }));

    await waitFor(() => expect(within(agents).getByRole("button", { name: "Poll now" })).toBeEnabled());
    click(screen.getByRole("button", { name: "Whooper actions" }));
    expect((await screen.findAllByRole("menuitem")).map((item) => item.textContent)).toEqual(["Stop", "Settings", "Open logs"]);
    click(screen.getByRole("menuitem", { name: "Stop" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Stop Whooper?" });
    expect(dialog).toHaveTextContent("widgets#102 · acme/widgets#102");
    expect(api.stopPlugin).not.toHaveBeenCalled();
    click(within(dialog).getByRole("button", { name: "Stop Whooper" }));
    await waitFor(() => expect(api.stopPlugin).toHaveBeenCalledExactlyOnceWith({ plugin: "whooper" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
  });

  it("opens the cause from the pinned row's red words, and offers Start in place of Pause once stopped", async () => {
    const { api } = controlsApi();
    const stopped = fixturePlugin({ actions: [...pageActions, { name: "pause", title: "Pause", description: "Pauses the schedule.", confirm: false }, { name: "resume", title: "Resume", description: "Resumes the schedule.", confirm: false }], process: { kind: "notRunning", exit: null } });
    api.startPlugin = vi.fn(async () => stopped);
    renderApp(api);
    const agents = await screen.findByRole("region", { name: "Background agents" });
    const words = await within(agents).findByRole("button", { name: "Can't review · Docker isn't running" });
    click(words);
    await settle();
    expect(screen.getByRole("main", { name: "Blocked reviews" })).toBeInTheDocument();
    expect(words).toHaveAttribute("aria-current", "true");

    api.listPlugins = vi.fn(async () => [stopped]);
    await stores[0]!.refreshPlugins();
    await settle();
    expect(within(agents).getByText("Stopped")).toBeVisible();
    expect(within(agents).queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    expect(within(agents).getByRole("button", { name: "Poll now" })).toBeDisabled();
    click(within(agents).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(api.startPlugin).toHaveBeenCalledExactlyOnceWith({ plugin: "whooper" }));
    click(screen.getByRole("button", { name: "Whooper actions" }));
    expect((await screen.findAllByRole("menuitem")).map((item) => item.textContent)).toEqual(["Settings", "Open logs"]);
  });

  it("opens a blocking cause from its folded Waiting section and retries each review it stopped, then offers Run now", async () => {
    const { api, pluginRequest } = controlsApi();
    renderApp(api);
    click(await screen.findByRole("button", { name: "Waiting · Docker isn't running 2" }));
    click(await screen.findByRole("treeitem", { name: /^Docker isn't running, Blocks every review · since .*, 2 reviews waiting$/ }));
    await settle();
    // The header says the red dot's words too.
    expect(document.querySelector(".shell-meta")).toHaveTextContent("Whooper · Blocks every review · 2 reviews waiting");
    expect(document.querySelector(".shell-meta .pt-status-dot")).toHaveAttribute("data-status", "error");

    const cause = screen.getByRole("main", { name: "Blocked reviews" });
    expect(screen.getByRole("heading", { level: 1, name: "Docker isn't running" })).toBeInTheDocument();
    expect(within(cause).getByRole("heading", { name: "acme/gadgets · 2" })).toBeInTheDocument();
    fireEvent.click(within(cause).getByRole("button", { name: "Retry all 2" }));
    expect(await within(cause).findByText("Queued 2 reviews for Whooper's next round.")).toBeInTheDocument();
    expect(pluginRequest.mock.calls.filter(([sent]) => sent.action === "retry").map(([sent]) => sent.args)).toEqual([["waiting-60"], ["waiting-61"]]);
    fireEvent.click(within(cause).getByRole("button", { name: "Run now" }));
    expect(await within(cause).findByLabelText("What the round printed")).toHaveTextContent("round done");

    fireEvent.click(within(cause).getByRole("button", { name: "Open logs" }));
    await settle();
    expect(screen.getByRole("tab", { name: "Logs" })).toHaveAttribute("aria-selected", "true");
    expect(api.readPluginLog).toHaveBeenCalledWith({ plugin: "whooper" });
  });
});

describe("menu bar", () => {
  const docker = { id: "docker", summary: "Docker isn't running", scope: "all" };
  /** Whooper blocked by Docker, with a review ready to approve and one failed; its controls succeed, and `choose` makes a choice in the menu bar. */
  function menuBarApi() {
    const status = fixtureStatus("2026-11-26T23:45:00Z", { reviews: fixtureReviews(), blocked: { ...docker, since: null, waiting: 0 } });
    const pluginRequest = vi.fn(async (sent: PluginActionRequest) => (sent.action === "status"
      ? actionResult({ stdout: JSON.stringify(status) })
      : sent.action === "show-review" ? actionResult({ stdout: JSON.stringify(fixtureShown(sent.args[0]!)) }) : actionResult({ stdout: "done\n" })));
    const plugin = fixturePlugin({ actions: [...pageActions, { name: "pause", title: "Pause", description: "Pauses the schedule.", confirm: false }, { name: "resume", title: "Resume", description: "Resumes the schedule.", confirm: false }] });
    let handler: (command: MenuBarCommand) => void = () => {};
    const api = createApi({
      listPlugins: vi.fn(async () => [plugin]),
      pluginRequest,
      listenToMenuBar: vi.fn(async (next: (command: MenuBarCommand) => void) => {
        handler = next;
        return () => {};
      }),
    });
    const lastRequest = () => vi.mocked(api.updateMenuBar).mock.lastCall?.[0];
    const choose = (command: MenuBarCommand) => {
      handler(command);
      flush();
    };
    return { api, pluginRequest, lastRequest, choose };
  }

  it("shows the pinned agent's words and buttons and how many reviews need you, and runs its choices as the sidebar does", async () => {
    const { api, pluginRequest, lastRequest, choose } = menuBarApi();
    renderApp(api);
    await screen.findByRole("button", { name: "Can't review · Docker isn't running" });
    await waitFor(() => expect(lastRequest()).toEqual({
      visible: true,
      // widgets#101 is ready to approve and gadgets#55 failed for a reason of its own.
      reviewsNeedingYou: 2,
      agents: [{
        plugin: "whooper", title: "Whooper", status: "Can't review · Docker isn't running", cause: "docker", alert: true,
        controls: [{ action: "pause", label: "Pause", enabled: true }, { action: "poll", label: "Poll now", enabled: true }],
      }],
    }));
    const sent = vi.mocked(api.updateMenuBar).mock.calls.length;

    choose({ kind: "agent", plugin: "whooper", action: "poll" });
    await waitFor(() => expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "round", args: [], confirmedFingerprint: null }));
    // While the round runs, the menu says so, as the row does.
    expect(vi.mocked(api.updateMenuBar).mock.calls.slice(sent).some(([request]) => request.agents[0]?.controls[1]?.label === "Polling…")).toBe(true);

    choose({ kind: "openCause", plugin: "whooper", cause: "docker" });
    expect(screen.getByRole("main", { name: "Blocked reviews" })).toBeInTheDocument();
    choose({ kind: "openNeedsYou" });
    expect(screen.getByRole("main", { name: "Review" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "separate cache configuration" })).toBeInTheDocument();
    choose({ kind: "openAgent", plugin: "whooper" });
    expect(screen.getByRole("main", { name: "Background agent" })).toBeInTheDocument();
  });

  it("turns the menu bar extra off and on from the palette, and remembers the choice", async () => {
    const { api, lastRequest } = menuBarApi();
    renderApp(api);
    await waitFor(() => expect(lastRequest()?.visible).toBe(true));
    const runCommand = async (label: string) => {
      press("k");
      const input = await screen.findByRole("combobox", { name: "Search conversations, reviews, and commands" });
      fireEvent.input(input, { target: { value: label } });
      flush();
      fireEvent.keyDown(input, { key: "Enter" });
      flush();
    };
    await runCommand("Hide background agents from the menu bar");
    await waitFor(() => expect(lastRequest()?.visible).toBe(false));
    expect(window.localStorage.getItem("prompting-time.menu-bar")).toBe("off");
    await runCommand("Show background agents in the menu bar");
    await waitFor(() => expect(lastRequest()?.visible).toBe(true));
    expect(window.localStorage.getItem("prompting-time.menu-bar")).toBe("on");
  });
});

describe("status format 2", () => {
  const fingerprint = "d".repeat(64);
  const failedTitle = "Review this stack of PRs · acme/widgets#60";
  /** The invented format-2 plugin: Rerun succeeds, and Delete previews, then deletes with the fingerprint. */
  function v2Api() {
    const pluginRequest = vi.fn(async (sent: PluginActionRequest) => {
      if (sent.action === "status") return actionResult({ stdout: JSON.stringify(fixtureV2Status()) });
      if (sent.action === "delete") {
        return sent.confirmedFingerprint === null
          ? actionResult({ stdout: `printed the request; nothing deleted\nfingerprint: ${fingerprint}\n`, fingerprint, preview: "delete run-74c91b1d" })
          : actionResult({ stdout: "deleted\n" });
      }
      return actionResult({ stdout: "queued again\n" });
    });
    let handler: (command: MenuBarCommand) => void = () => {};
    let appEvents: ((event: AppEvent) => void) | null = null;
    const api = createApi({
      listPlugins: vi.fn(async () => [fixtureV2Plugin()]),
      pluginRequest,
      listenToMenuBar: vi.fn(async (next: (command: MenuBarCommand) => void) => {
        handler = next;
        return () => {};
      }),
      listenToAppEvents: vi.fn(async (listener: (event: AppEvent) => void) => {
        appEvents = listener;
        return () => {};
      }),
    });
    const choose = (command: MenuBarCommand) => {
      handler(command);
      flush();
    };
    return { api, pluginRequest, choose, emit: (event: AppEvent) => appEvents?.(event), lastRequest: () => vi.mocked(api.updateMenuBar).mock.lastCall?.[0] };
  }
  const selected = () => document.querySelector('.all-work [role=treeitem][aria-selected="true"]')?.getAttribute("aria-label");
  /** The plugin's page frame, with a spy on what Prompting Time posts to it. */
  function pageFrame() {
    const element = document.querySelector<HTMLIFrameElement>("iframe.plugin-frame");
    if (!element?.contentWindow) throw new Error("no plugin frame");
    return { element, window: element.contentWindow, posted: vi.spyOn(element.contentWindow, "postMessage") };
  }
  async function fromPage(source: Window, data: unknown) {
    window.dispatchEvent(new MessageEvent("message", { data, source }));
    await settle();
    await settle();
  }

  it("steps through its blockers and Needs you items with Command-J, and counts the items in the menu bar", async () => {
    const { api, choose, lastRequest } = v2Api();
    renderApp(api);
    await screen.findByRole("treeitem", { name: /^Codex is signed out/ });
    await waitFor(() => expect(lastRequest()).toMatchObject({
      reviewsNeedingYou: 2,
      agents: [{ plugin: "fixture-runs", status: "Idle · last run 2 min ago", cause: null, alert: true }],
    }));

    press("j");
    expect(selected()).toMatch(/^Codex is signed out/);
    expect(screen.getByRole("main", { name: "Background agent" })).toBeInTheDocument();
    expect(screen.getByTitle("Fixture runs's page")).toHaveAttribute("src", "pt-plugin://fixture-runs/index.html");
    press("j");
    expect(selected()).toMatch(new RegExp(`^${failedTitle.replaceAll(".", "\\.")}`));
    press("j");
    expect(selected()).toMatch(/^Tighten upload retries/);
    press("j");
    expect(selected()).toMatch(/^Codex is signed out/);

    // The menu bar's count opens the first item, not a blocker.
    choose({ kind: "openNeedsYou" });
    expect(selected()).toMatch(new RegExp(`^${failedTitle.replaceAll(".", "\\.")}`));
  });

  it("runs a row's action with --item and the item's id, and one marked confirm only after the confirmation", async () => {
    const { api, pluginRequest } = v2Api();
    renderApp(api);
    fireEvent.click(await screen.findByRole("button", { name: `Actions for ${failedTitle}` }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Rerun" }));
    await waitFor(() => expect(pluginRequest).toHaveBeenCalledWith({ plugin: "fixture-runs", action: "rerun", args: ["--item", "run-74c91b1d"], confirmedFingerprint: null }));
    await waitFor(() => expect(document.querySelector(".app-notice")).toHaveTextContent(`Rerun · ${failedTitle}`));

    fireEvent.click(screen.getByRole("button", { name: `Actions for ${failedTitle}` }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete run" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete run" });
    expect(await within(dialog).findByLabelText("What it will send")).toHaveTextContent("delete run-74c91b1d");
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "fixture-runs", action: "delete", args: ["--item", "run-74c91b1d"], confirmedFingerprint: null });
    expect(pluginRequest).not.toHaveBeenCalledWith(expect.objectContaining({ action: "delete", confirmedFingerprint: fingerprint }));

    const confirm = within(dialog).getByRole("button", { name: "Delete run" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);
    await waitFor(() => expect(pluginRequest).toHaveBeenCalledWith({ plugin: "fixture-runs", action: "delete", args: ["--item", "run-74c91b1d"], confirmedFingerprint: fingerprint }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
  });

  it("opens a clicked row in the plugin's sandboxed page at its route, and sends Command-J's next row to the same page", async () => {
    const { api } = v2Api();
    renderApp(api);
    fireEvent.click(await screen.findByRole("treeitem", { name: new RegExp(`^${failedTitle.replaceAll(".", "\\.")}`) }));
    await settle();
    const page = pageFrame();
    expect(page.element).toHaveAttribute("sandbox", "allow-scripts");
    expect(page.element).toHaveAttribute("src", "pt-plugin://fixture-runs/index.html");
    expect(selected()).toMatch(new RegExp(`^${failedTitle.replaceAll(".", "\\.")}`));
    // Nothing goes to the page before it says it is ready, nor for a message from another window.
    expect(page.posted).not.toHaveBeenCalled();
    await fromPage(window, { version: 2, type: "ready" });
    expect(page.posted).not.toHaveBeenCalled();
    await fromPage(page.window, { version: 2, type: "ready" });
    expect(page.posted).toHaveBeenCalledExactlyOnceWith({ version: 2, type: "open", route: "/runs/run-74c91b1d" }, "*");

    press("j");
    await settle();
    expect(selected()).toMatch(/^Tighten upload retries/);
    expect(document.querySelector("iframe.plugin-frame")).toBe(page.element);
    expect(page.posted).toHaveBeenLastCalledWith({ version: 2, type: "open", route: "/runs/run-review-2" }, "*");

    // Opening the same row again shows it again, after the page may have moved on.
    fireEvent.click(screen.getByRole("treeitem", { name: /^Tighten upload retries/ }));
    await settle();
    expect(page.posted).toHaveBeenCalledTimes(3);
    expect(page.posted).toHaveBeenLastCalledWith({ version: 2, type: "open", route: "/runs/run-review-2" }, "*");
  });

  it("runs the page's calls to declared actions only, a confirm action only after the confirmation, and tells the page what changed", async () => {
    const { api, pluginRequest, emit } = v2Api();
    renderApp(api);
    fireEvent.click(await screen.findByRole("treeitem", { name: /^Codex is signed out/ }));
    await settle();
    const page = pageFrame();
    await fromPage(page.window, { version: 2, type: "ready" });
    expect(page.posted).toHaveBeenLastCalledWith({ version: 2, type: "open", route: "/providers" }, "*");

    await fromPage(page.window, { version: 2, type: "call", id: 1, action: "status", args: [] });
    expect(page.posted).toHaveBeenLastCalledWith({ version: 2, type: "result", reply_to: 1, json: fixtureV2Status() }, "*");

    await fromPage(page.window, { version: 2, type: "call", id: 2, action: "rm-everything", args: [] });
    expect(page.posted).toHaveBeenLastCalledWith({ version: 2, type: "error", reply_to: 2, message: "Fixture runs declares no rm-everything action." }, "*");
    expect(pluginRequest).not.toHaveBeenCalledWith(expect.objectContaining({ action: "rm-everything" }));

    await fromPage(page.window, { version: 2, type: "call", id: 3, action: "delete", args: ["--item", "run-74c91b1d"] });
    const dialog = await screen.findByRole("alertdialog", { name: "Delete run" });
    expect(await within(dialog).findByLabelText("What it will send")).toHaveTextContent("delete run-74c91b1d");
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "fixture-runs", action: "delete", args: ["--item", "run-74c91b1d"], confirmedFingerprint: null });
    expect(pluginRequest).not.toHaveBeenCalledWith(expect.objectContaining({ action: "delete", confirmedFingerprint: fingerprint }));
    expect(page.posted).not.toHaveBeenCalledWith(expect.objectContaining({ reply_to: 3 }), "*");
    const confirm = within(dialog).getByRole("button", { name: "Delete run" });
    await waitFor(() => expect(confirm).toBeEnabled());
    fireEvent.click(confirm);
    await settle();
    await settle();
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "fixture-runs", action: "delete", args: ["--item", "run-74c91b1d"], confirmedFingerprint: fingerprint });
    expect(page.posted).toHaveBeenLastCalledWith({ version: 2, type: "result", reply_to: 3, json: "deleted\n" }, "*");

    // Cancelling runs nothing for real and tells the page.
    await fromPage(page.window, { version: 2, type: "call", id: 4, action: "delete", args: ["--item", "run-review-2"] });
    fireEvent.click(within(await screen.findByRole("alertdialog", { name: "Delete run" })).getByRole("button", { name: "Cancel" }));
    await settle();
    expect(page.posted).toHaveBeenLastCalledWith({ version: 2, type: "error", reply_to: 4, message: "Nothing ran: the confirmation was cancelled." }, "*");
    expect(pluginRequest).not.toHaveBeenCalledWith(expect.objectContaining({ args: ["--item", "run-review-2"], confirmedFingerprint: fingerprint }));

    emit({ kind: "pluginChanged", sequence: "1", plugin: "fixture-runs", files: ["runs/run-74c91b1d/run.json"] });
    emit({ kind: "pluginChanged", sequence: "2", plugin: "whooper", files: ["round.lock/round.json"] });
    await settle();
    expect(page.posted).toHaveBeenLastCalledWith({ version: 2, type: "changed", files: ["runs/run-74c91b1d/run.json"] }, "*");
  });
});

describe("review header", () => {
  it("says why a review that says approve once CI passes is not safe to approve", async () => {
    const unsafe = { ...fixtureReviews()[0]!, safe_to_approve: false };
    const findings = { status: "reviewed", skip_reason: "", approval: { verdict: "approve_once_ci_passes", reason: "Invented reason." }, findings: [
      { id: "1", path: "src/a.rs", line: 3, side: "RIGHT", title: "A nit", draft: "**nit:** invented", kind: "nit" },
      { id: "2", path: "src/a.rs", line: 9, side: "RIGHT", title: "A question", draft: "**question:** invented", kind: "question" },
    ] };
    const pluginRequest = vi.fn(async (sent: PluginActionRequest) => (sent.action === "status"
      ? actionResult({ stdout: JSON.stringify(fixtureStatus("2026-11-26T23:45:00Z", { reviews: [unsafe] })) })
      : actionResult({ stdout: JSON.stringify(fixtureShown(sent.args[0]!, { findings })) })));
    renderApp(createApi({ listPlugins: vi.fn(async () => [fixturePlugin({ actions: pageActions })]), pluginRequest }));
    click(await screen.findByRole("treeitem", { name: /^separate cache configuration/ }));
    const meta = () => document.querySelector(".shell-meta");
    await waitFor(() => expect(meta()).toHaveTextContent("Whooper review · acme/widgets#101 · approve once CI passes · has a question"));
    // Orange, as the row would be outside Needs a look: the review needs a look.
    expect(meta()!.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "attention");
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "show-review", args: [unsafe.run_id], confirmedFingerprint: null });
  });

  it("puts a green dot before the words of a review that is ready to approve", async () => {
    const pluginRequest = vi.fn(async () => actionResult({ stdout: JSON.stringify(fixtureStatus("2026-11-26T23:45:00Z", { reviews: [fixtureReviews()[0]!] })) }));
    renderApp(createApi({ listPlugins: vi.fn(async () => [fixturePlugin({ actions: pageActions })]), pluginRequest }));
    click(await screen.findByRole("treeitem", { name: /^separate cache configuration/ }));
    const meta = () => document.querySelector(".shell-meta");
    await waitFor(() => expect(meta()).toHaveTextContent("Whooper review · acme/widgets#101 · approve once CI passes"));
    expect(meta()!.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "success");
  });
});

describe("triage", () => {
  const docker = { id: "docker", summary: "Docker isn't running", scope: "all" };
  const notice = () => document.querySelector<HTMLElement>(".app-notice");
  /** Whooper with reviews to approve, to look at, failed, removed, and waiting on Docker; skip and remove succeed. */
  function triageApi() {
    const reviews = [
      ...fixtureReviews(),
      { ...fixtureReviews()[0]!, run_id: "look-103", pr: 103, title: "cap the upload retry delay", verdict: "look_first", safe_to_approve: false, findings: 1, started_at: "2026-11-26T12:00:00Z", ended_at: "2026-11-26T12:20:00Z" },
      { ...fixtureReviews()[0]!, run_id: "removed-8", pr: 8, title: "keep focus in the date picker", removed_by_user: true, needs_you: false, started_at: "2026-11-26T10:00:00Z", ended_at: "2026-11-26T10:20:00Z" },
      { ...fixtureReviews()[1]!, run_id: "waiting-60", repo: "acme/gadgets", pr: 60, title: "speed up the gadget index", outcome: "waiting", cause: docker },
      { ...fixtureReviews()[1]!, run_id: "waiting-61", repo: "acme/gadgets", pr: 61, title: "drop the legacy upload path", outcome: "waiting", cause: docker },
    ];
    const pluginRequest = vi.fn(async (sent: PluginActionRequest) => {
      switch (sent.action) {
        case "status": return actionResult({ stdout: JSON.stringify(fixtureStatus("2026-11-26T23:45:00Z", { reviews })) });
        case "show-review": return actionResult({ stdout: JSON.stringify(fixtureShown(sent.args[0]!)) });
        case "skip": case "remove": return actionResult({ stdout: `${sent.action} ${sent.args.join(" ")}\n` });
        default: return actionResult({ stdout: JSON.stringify(fixtureSettings) });
      }
    });
    const plugin = fixturePlugin({ actions: [...pageActions,
      { name: "skip", title: "Skip a review", description: "Marks a review done without submitting it.", confirm: false },
      { name: "remove", title: "Remove a review", description: "Takes a review out of the lists; its files stay.", confirm: false }] });
    return { pluginRequest, api: createApi({ listPlugins: vi.fn(async () => [plugin]), pluginRequest }) };
  }
  const calls = (pluginRequest: ReturnType<typeof triageApi>["pluginRequest"], action: string) => pluginRequest.mock.calls.filter(([sent]) => sent.action === action).map(([sent]) => sent.args);

  it("opens the next item in Needs you after Skip and names it, and Undo brings the review back", async () => {
    const { api, pluginRequest } = triageApi();
    renderApp(api);
    click(await screen.findByRole("treeitem", { name: /^separate cache configuration/ }));
    await settle();
    const reveal = vi.mocked(Element.prototype.scrollIntoView);
    reveal.mockClear();
    click(screen.getByRole("button", { name: "Skip" }));
    await waitFor(() => expect(notice()).toHaveTextContent("Skipped widgets#101 · now showing widgets#103"));
    expect(calls(pluginRequest, "skip")).toEqual([["20261126T150000Z-101"]]);
    expect(screen.getByRole("heading", { level: 1, name: "cap the upload retry delay" })).toBeInTheDocument();
    // The next item's row scrolls into view if its section shows it lower down.
    await settle();
    expect(reveal.mock.contexts).toContain(screen.getByRole("treeitem", { name: /^cap the upload retry delay/ }));

    click(within(notice()!).getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(screen.getByRole("heading", { level: 1, name: "separate cache configuration" })).toBeInTheDocument());
    expect(calls(pluginRequest, "skip")).toEqual([["20261126T150000Z-101"], ["20261126T150000Z-101", "--undo"]]);
    expect(notice()).toBeNull();
  });

  it("removes a review from its row with Undo, and from the open review's header, which opens the next item", async () => {
    const { api, pluginRequest } = triageApi();
    renderApp(api);
    click(await screen.findByRole("button", { name: "Remove widgets#103" }));
    await waitFor(() => expect(notice()).toHaveTextContent("Removed widgets#103"));
    expect(screen.getByRole("heading", { level: 1, name: "Auth refactor" })).toBeInTheDocument();
    click(within(notice()!).getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(calls(pluginRequest, "remove")).toEqual([["look-103"], ["look-103", "--undo"]]));

    click(screen.getByRole("treeitem", { name: /^separate cache configuration/ }));
    await settle();
    click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(notice()).toHaveTextContent("Removed widgets#101 · now showing widgets#103"));
    expect(calls(pluginRequest, "remove").at(-1)).toEqual(["20261126T150000Z-101"]);
    expect(screen.getByRole("heading", { level: 1, name: "cap the upload retry delay" })).toBeInTheDocument();
  });

  it("restores a removed review, which search finds, from its page", async () => {
    const { api, pluginRequest } = triageApi();
    renderApp(api);
    await screen.findByRole("treeitem", { name: /^separate cache configuration/ });
    expect(screen.queryByRole("treeitem", { name: /keep focus in the date picker/ })).not.toBeInTheDocument();
    const search = screen.getByRole("searchbox", { name: "Search PRs and chats" });
    fireEvent.input(search, { target: { value: "keep focus" } });
    flush();
    fireEvent.keyDown(search, { key: "Enter" });
    flush();
    await settle();
    const banner = screen.getByText(/its files stay on disk, and search still finds it/).closest<HTMLElement>("[role=note]")!;
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
    click(within(banner).getByRole("button", { name: "Restore" }));
    await waitFor(() => expect(notice()).toHaveTextContent("Restored widgets#8 to your lists."));
    expect(calls(pluginRequest, "remove")).toEqual([["removed-8", "--undo"]]);
  });

  it("asks before Remove all on a cause's screen, says the files stay and Whooper won't retry them, and removes each with Undo", async () => {
    const { api, pluginRequest } = triageApi();
    renderApp(api);
    click(await screen.findByRole("button", { name: "Waiting · Docker isn't running 2" }));
    click(screen.getByRole("treeitem", { name: /^Docker isn't running, Blocks every review.*, 2 reviews waiting$/ }));
    await settle();
    click(screen.getByRole("button", { name: "Remove all" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove 2 reviews?" });
    expect(dialog).toHaveTextContent("Whooper won't retry them");
    expect(dialog).toHaveTextContent("files stay on disk");
    expect(calls(pluginRequest, "remove")).toEqual([]);
    click(within(dialog).getByRole("button", { name: "Remove 2 reviews" }));
    await waitFor(() => expect(notice()).toHaveTextContent("Removed 2 reviews. Their files stay on disk."));
    expect(calls(pluginRequest, "remove")).toEqual([["waiting-60"], ["waiting-61"]]);
    click(within(notice()!).getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(calls(pluginRequest, "remove")).toHaveLength(4));
    expect(calls(pluginRequest, "remove").slice(2)).toEqual([["waiting-60", "--undo"], ["waiting-61", "--undo"]]);
  });
});

describe("command palette", () => {
  async function openWorkspace(items = [
    createdConversation({ id: "a", title: "Alpha", projectRoot: "/projects/compiler" }),
    createdConversation({ id: "b", title: "Beta", projectRoot: "/projects/renderer" }),
  ]) {
    const api = createApi({ listConversations: vi.fn().mockResolvedValue({ items, nextCursor: null }) });
    const view = renderApp(api);
    await screen.findByRole("treeitem", { name: /Alpha/ });
    return view;
  }
  const combobox = () => screen.findByRole("combobox", { name: "Search conversations, reviews, and commands" });

  it("opens with ⌘K, searches conversations by project path, and opens the chosen one", async () => {
    const { store } = await openWorkspace();
    press("k");
    const input = await combobox();
    expect(screen.getByRole("option", { name: /Alpha.*Current/ })).toBeVisible();
    fireEvent.input(input, { target: { value: "renderer" } });
    flush();
    expect(screen.queryByRole("option", { name: /Alpha/ })).not.toBeInTheDocument();
    fireEvent.keyDown(input, { key: "Enter" });
    flush();
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("b"));
    expect(screen.queryByRole("combobox", { name: "Search conversations, reviews, and commands" })).not.toBeInTheDocument();
  });

  it("toggles closed with Ctrl+K and ignores composition and held keys", async () => {
    await openWorkspace();
    press("k", { isComposing: true });
    press("k", { repeat: true });
    await settle();
    expect(screen.queryByRole("combobox", { name: "Search conversations, reviews, and commands" })).not.toBeInTheDocument();
    press("k", { metaKey: false, ctrlKey: true });
    await combobox();
    press("k", { metaKey: false, ctrlKey: true });
    await waitFor(() => expect(screen.queryByRole("combobox", { name: "Search conversations, reviews, and commands" })).not.toBeInTheDocument());
  });

  it("runs layout commands after closing and names disabled ones", async () => {
    await openWorkspace();
    click(screen.getByRole("button", { name: "Search (⌘K)" }));
    const input = await combobox();
    expect(screen.getByRole("option", { name: /Go to parent conversation/ })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("option", { name: /Next item waiting on you/ })).toHaveAttribute("aria-disabled", "true");
    fireEvent.input(input, { target: { value: "show inspector" } });
    flush();
    fireEvent.keyDown(input, { key: "Enter" });
    flush();
    expect(await screen.findByRole("complementary", { name: "Inspector" })).toBeVisible();
  });

  it("does not open while the workspace is loading", async () => {
    let finish!: (value: { items: ConversationSummary[]; nextCursor: null }) => void;
    renderApp(createApi({ listConversations: vi.fn(() => new Promise<{ items: ConversationSummary[]; nextCursor: null }>((resolve) => { finish = resolve; })) }));
    press("k");
    await settle();
    expect(screen.queryByRole("combobox", { name: "Search conversations, reviews, and commands" })).not.toBeInTheDocument();
    finish({ items: [createdConversation({ id: "a", title: "Alpha" })], nextCursor: null });
    await screen.findByRole("treeitem", { name: /Alpha/ });
    press("k");
    expect(await combobox()).toBeVisible();
  });
});

describe("App", () => {
  it("renders the shell with All work, the queue, and provider details in the inspector", async () => {
    renderApp(createApi());
    expect(await screen.findByRole("treeitem", { name: /Auth refactor/ })).toBeVisible();
    expect(screen.getByRole("heading", { level: 1, name: "Auth refactor" })).toBeVisible();
    expect(within(screen.getByRole("banner")).getByText(/Codex · queued/)).toBeVisible();
    click(screen.getByRole("button", { name: "Show inspector" }));
    expect(screen.getByRole("complementary", { name: "All work" })).toBeVisible();
    expect(screen.getByRole("main", { name: "Conversation workspace" })).toBeVisible();
    expect(screen.getByRole("complementary", { name: "Inspector" })).toBeVisible();
    expect(screen.getByText("1 queued")).toBeVisible();
    expect(await screen.findByText("Codex 0.144.1")).toBeVisible();
    expect(screen.getByText(/Claude unavailable/)).toBeVisible();
  });

  it("reads its store once, even when the prop is written as a call", async () => {
    const api = createApi();
    render(() => <App store={createAppStore(api)} />);
    expect(await screen.findByRole("treeitem", { name: /Auth refactor/ })).toBeVisible();
    expect(api.listConversations).toHaveBeenCalledOnce();
  });

  it("filters by status and hides and shows All work", async () => {
    renderApp(createApi());
    await screen.findByRole("treeitem", { name: /Auth refactor/ });
    const filter = screen.getByRole("button", { name: "Filter conversations: All statuses" });
    filter.focus();
    click(filter);
    const menu = await screen.findByRole("menu", { name: "Filter conversations: All statuses" });
    expect(screen.getByRole("menuitemradio", { name: "All statuses" })).toHaveAttribute("aria-checked", "true");
    for (let step = 0; step < 6; step += 1) fireEvent.keyDown(menu, { key: "ArrowDown" });
    await waitFor(() => expect(screen.getByRole("menuitemradio", { name: "Completed" })).toHaveAttribute("data-highlighted"));
    fireEvent.keyDown(menu, { key: "Enter" });
    await waitFor(() => expect(screen.queryByRole("treeitem", { name: /Auth refactor/ })).not.toBeInTheDocument());
    expect(screen.getByText("Nothing needs you right now.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Filter conversations: Completed" })).toBeVisible();
    click(screen.getByRole("button", { name: "Hide All work" }));
    expect(screen.getByRole("button", { name: "Show All work" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("complementary", { name: "All work" })).not.toBeInTheDocument();
    click(screen.getByRole("button", { name: "Show All work" }));
    click(screen.getByRole("button", { name: "Filter conversations: Completed" }));
    expect(await screen.findByRole("menuitemradio", { name: "Completed" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "All statuses" })).toHaveAttribute("aria-checked", "false");
  });

  it("starts with the inspector closed and keeps explicit toggles across conversation changes", async () => {
    const api = createApi();
    const other = createdConversation({ id: "c2", title: "Other conversation" });
    const initial = await api.listConversations({ cursor: null, limit: 40 });
    api.listConversations = vi.fn().mockResolvedValue({ items: [...initial.items, other], nextCursor: null });
    const { store } = renderApp(api);
    await screen.findByRole("treeitem", { name: /Auth refactor/ });
    expect(screen.getByRole("button", { name: "Show inspector" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
    expect(api.loadDiagnostics).not.toHaveBeenCalled();
    await store.selectConversation("c2");
    await settle();
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
    click(screen.getByRole("button", { name: "Show inspector" }));
    expect(screen.getByRole("complementary", { name: "Inspector" })).toBeVisible();
    await store.selectConversation("c1");
    await settle();
    expect(screen.getByRole("button", { name: "Hide inspector" })).toHaveAttribute("aria-expanded", "true");
    click(screen.getByRole("button", { name: "Hide inspector" }));
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
  });

  it("shows a recoverable synchronization error", async () => {
    renderApp(createApi({ listConversations: vi.fn().mockRejectedValueOnce(new Error("Bridge disconnected")).mockResolvedValue({ items: [], nextCursor: null }) }));
    expect(await screen.findByText("Bridge disconnected")).toBeVisible();
    click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No conversation selected")).toBeVisible();
  });

  it("keeps the startup diagnostic and recovery action when services are unavailable", async () => {
    renderApp(createApi({
      getBootstrap: vi.fn().mockResolvedValueOnce({
        providers: [],
        startupDiagnostic: { code: "storage-error", message: "Cannot open database", action: "Check Application Support permissions and restart" },
      }).mockResolvedValue({ providers: [] }),
      listConversations: vi.fn()
        .mockRejectedValueOnce(new BridgeError("startup-unavailable", "Application services are unavailable.", null))
        .mockResolvedValue({ items: [], nextCursor: null }),
    }));
    expect(await screen.findByText("Application services are unavailable.")).toBeVisible();
    expect(screen.getByText("Cannot open database")).toBeVisible();
    expect(screen.getByText("Check Application Support permissions and restart")).toBeVisible();
    click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No conversation selected")).toBeVisible();
    expect(screen.queryByText("Cannot open database")).not.toBeInTheDocument();
  });

  it("has no detectable axe violations with every pane open", async () => {
    renderApp(createApi());
    await screen.findByRole("heading", { name: "Timeline" });
    click(screen.getByRole("button", { name: "Show inspector" }));
    await screen.findByText("Codex 0.144.1");
    expect(await axeViolations({ page: true })).toEqual([]);
  });

  it("gives focus back to the inspector trigger when the inspector closes", async () => {
    renderApp(createApi());
    await screen.findByRole("heading", { name: "Timeline" });
    click(screen.getByRole("button", { name: "Show inspector" }));
    click(screen.getByRole("button", { name: "Close inspector" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Show inspector" })).toHaveFocus());
  });

  it("moves focus into the narrow inspector overlay, keeps Tab inside, and restores the trigger", async () => {
    mediaQueries(narrowQuery);
    renderApp(createApi());
    await screen.findByRole("heading", { name: "Timeline" });
    const trigger = screen.getByRole("button", { name: "Show inspector" });
    click(trigger);
    const close = await screen.findByRole("button", { name: "Close inspector" });
    await waitFor(() => expect(close).toHaveFocus());
    expect(screen.getByRole("main", { name: "Conversation workspace" })).toHaveAttribute("inert");
    press("k");
    await settle();
    expect(screen.queryByRole("combobox", { name: "Search conversations, reviews, and commands" })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("complementary", { name: "Inspector" }), { key: "Tab", shiftKey: true });
    expect(document.activeElement).not.toBe(trigger);
    fireEvent.keyDown(screen.getByRole("complementary", { name: "Inspector" }), { key: "Escape" });
    flush();
    await waitFor(() => expect(screen.getByRole("button", { name: "Show inspector" })).toHaveFocus());
    expect(screen.getByRole("main", { name: "Conversation workspace" })).not.toHaveAttribute("inert");
  });
});

describe("composer and drafts", () => {
  const input = (element: Element, value: string) => {
    fireEvent.input(element, { target: { value } });
    flush();
  };
  async function openDrafts(overrides: Partial<AppApi> = {}) {
    const conversations = [createdConversation({ id: "a", title: "Draft A" }), createdConversation({ id: "b", title: "Draft B" })];
    const api = createApi({
      listConversations: vi.fn().mockResolvedValue({ items: conversations, nextCursor: null }),
      loadConversation: vi.fn(async ({ conversationId }) => conversations.find(({ id }) => id === conversationId)!),
      ...overrides,
    });
    const view = renderApp(api);
    await screen.findByRole("textbox", { name: "Message" });
    return { ...view, api, select: async (id: string) => { await view.store.selectConversation(id); await settle(); } };
  }

  it("keeps independent raw Markdown drafts across switching and refresh", async () => {
    const { store, select } = await openDrafts();
    const draft = "    code\n\n**raw Markdown**\n";
    input(screen.getByLabelText("Message"), draft);
    await select("b");
    expect(screen.getByLabelText("Message")).toHaveValue("");
    input(screen.getByLabelText("Message"), "B draft");
    await select("a");
    expect(screen.getByLabelText("Message")).toHaveValue(draft);
    await store.retry();
    await settle();
    expect(screen.getByLabelText("Message")).toHaveValue(draft);
    await select("b");
    expect(screen.getByLabelText("Message")).toHaveValue("B draft");
  });

  it("keeps a reopened send pending and clears only its accepted draft", async () => {
    let finish!: (value: Awaited<ReturnType<AppApi["submitMessage"]>>) => void;
    const submitMessage = vi.fn<AppApi["submitMessage"]>(() => new Promise((resolve) => { finish = resolve; }));
    const { select } = await openDrafts({ submitMessage });
    input(screen.getByLabelText("Message"), "A draft");
    click(screen.getByRole("button", { name: "Send" }));
    await select("b");
    input(screen.getByLabelText("Message"), "B draft");
    await select("a");
    expect(screen.getByLabelText("Message")).toHaveValue("A draft");
    expect(screen.getByLabelText("Message")).toBeDisabled();
    fireEvent.keyDown(screen.getByLabelText("Message"), { key: "Enter" });
    expect(submitMessage).toHaveBeenCalledTimes(1);
    await select("b");
    finish({ runId: "run", status: "queued", provider: "codex", duplicate: false, routingExplanation: "test", thinkingDecision: { preference: { kind: "auto" }, requestedEffort: "high", reason: "fixture" } });
    await settle();
    expect(screen.getByLabelText("Message")).toHaveValue("B draft");
    await select("a");
    expect(screen.getByLabelText("Message")).toHaveValue("");
  });

  it.each(["outcome-unknown", "invalid-request"])("keeps a failed draft after switching away and retries %s with the right command identity", async (code) => {
    let fail!: (reason: Error) => void;
    const submitMessage = vi.fn<AppApi["submitMessage"]>(() => new Promise((_, reject) => { fail = reject; }));
    const { select } = await openDrafts({ submitMessage });
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "codex" } });
    flush();
    input(screen.getByLabelText("Message"), "A draft");
    click(screen.getByRole("button", { name: "Send" }));
    await select("b");
    fail(new BridgeError(code, "Send failed", null));
    await settle();
    await select("a");
    expect(screen.getByLabelText("Message")).toHaveValue("A draft");
    expect(screen.getByRole("alert")).toHaveTextContent("Send failed");
    click(screen.getByRole("button", { name: code === "outcome-unknown" ? "Retry send" : "Send" }));
    await waitFor(() => expect(submitMessage).toHaveBeenCalledTimes(2));
    const [first, retry] = submitMessage.mock.calls.map(([request]) => request);
    if (code === "outcome-unknown") expect(retry).toEqual(first);
    else expect(retry!.commandId).not.toBe(first!.commandId);
  });

  it("keeps child Enter and controls from submitting to the parent", async () => {
    const api = createApi();
    const { store } = renderApp(api);
    input(await screen.findByRole("textbox", { name: "Message" }), "Parent draft");
    await store.selectConversation("child-1");
    await settle();
    expect(screen.getByText("Recorded child activity is read-only.")).toBeVisible();
    expect(screen.queryByRole("textbox", { name: "Message" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Send" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Interrupt/ })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("main", { name: "Conversation workspace" }), { key: "Enter" });
    store.setDraft("child-1", "Unsupported child draft");
    expect(await store.submitDraft("child-1", null, null)).toBe(false);
    expect(api.submitMessage).not.toHaveBeenCalled();
    expect(store.getSnapshot().draftsById.c1?.text).toBe("Parent draft");
  });

  it("returns to the parent with its provider choice, preview, and exact draft", async () => {
    const api = createApi({ getBootstrap: vi.fn().mockResolvedValue({ providers: [
      { id: "codex", installed: true, available: true, version: "1", diagnostic: null, capabilities: ["interruption"] },
      { id: "claude", installed: true, available: true, version: "2", diagnostic: null, capabilities: ["interruption"] },
    ] }), listConversations: vi.fn().mockResolvedValue({ items: [createdConversation({ id: "c1", title: "Auth refactor", hasChildren: true })], nextCursor: null }) });
    const { store } = renderApp(api);
    const draft = "    indented code\n\nKeep **parent draft**\n";
    input(await screen.findByRole("textbox", { name: "Message" }), draft);
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "claude" } });
    flush();
    click(screen.getByRole("button", { name: "Preview Markdown" }));
    await store.selectConversation("child-1");
    await settle();
    click(within(screen.getByRole("navigation", { name: "Conversation ancestry" })).getByRole("button", { name: "Auth refactor" }));
    await screen.findByRole("region", { name: "Message preview" });
    expect(screen.getByRole("combobox", { name: "Provider" })).toHaveValue("claude");
    expect(screen.getByRole("region", { name: "Message preview" })).toHaveTextContent("parent draft");
    click(screen.getByRole("button", { name: "Edit Markdown" }));
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue(draft);
    expect(api.submitMessage).not.toHaveBeenCalled();
  });

  it.each([false, true])("gives back app interaction after canceling an interruption (preview: %s)", async (preview) => {
    renderApp(createApi({ getBootstrap: vi.fn().mockResolvedValue({ providers: [
      { id: "codex", installed: true, available: true, version: "1", diagnostic: null, capabilities: ["interruption"] },
      { id: "claude", installed: true, available: true, version: "2", diagnostic: null, capabilities: ["interruption"] },
    ] }) }));
    const provider = await screen.findByRole("combobox", { name: "Provider" });
    if (preview) {
      input(screen.getByRole("textbox", { name: "Message" }), "**keep draft**");
      click(screen.getByRole("button", { name: "Preview Markdown" }));
      click(screen.getByRole("button", { name: "Interrupt Codex" }));
    } else {
      fireEvent.change(provider, { target: { value: "claude" } });
      flush();
    }
    const dialog = await screen.findByRole("alertdialog", { name: preview ? "Interrupt Codex" : "Interrupt Codex to switch provider" });
    expect(document.querySelector(".app-shell")).toHaveAttribute("inert");
    expect(document.querySelector(".app-shell")).not.toContainElement(dialog);
    press("k");
    await settle();
    expect(screen.queryByRole("combobox", { name: "Search conversations, reviews, and commands" })).not.toBeInTheDocument();
    click(within(dialog).getByRole("button", { name: "Keep Codex running" }));
    await waitFor(() => expect(document.querySelector(".app-shell")).not.toHaveAttribute("inert"));
    await waitFor(() => expect(provider).toHaveFocus());
    if (preview) expect(screen.getByRole("region", { name: "Message preview" })).toHaveTextContent("keep draft");
  });
});

describe("creating and archiving", () => {
  function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason: Error) => void;
    const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  }

  async function openChooser(overrides: Partial<AppApi> = {}, inspectorOpen = false) {
    const api = createApi(overrides);
    const view = renderApp(api);
    const trigger = await screen.findByRole("button", { name: "New chat" });
    if (inspectorOpen) click(screen.getByRole("button", { name: "Show inspector" }));
    click(trigger);
    const dialog = screen.getByRole("dialog", { name: "New chat" });
    return { ...view, api, trigger, dialog, choose: within(dialog).getByRole("button", { name: "Another folder…" }) };
  }

  it("chooses a Git folder and creates an empty Best fit conversation from an empty install", async () => {
    const { api, dialog, choose, store } = await openChooser({ listConversations: vi.fn().mockResolvedValue({ items: [], nextCursor: null }) });
    expect(choose).toHaveFocus();
    expect(within(dialog).queryByLabelText("Title")).not.toBeInTheDocument();
    expect(dialog).toHaveAccessibleDescription("Where should the agent work?");
    click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith({
      title: "synthetic-project", objective: "", constraints: [],
      workspace: { kind: "isolated", path: "/tmp/synthetic-project" }, routingProfile: "bestFit",
    }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(store.getSnapshot().selectedConversationId).toBe("new-1");
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(api.submitMessage).not.toHaveBeenCalled();
  });

  it("lists the recent chats' folders first, and creates in one without the picker", async () => {
    const items = [
      createdConversation({ id: "a", title: "Alpha", projectRoot: "/projects/compiler" }),
      createdConversation({ id: "b", title: "Beta", projectRoot: "/projects/renderer" }),
      createdConversation({ id: "c", title: "Gamma", projectRoot: "/projects/compiler" }),
      createdConversation({ id: "d", title: "Delta", projectRoot: null }),
    ];
    const { api, dialog } = await openChooser({ listConversations: vi.fn().mockResolvedValue({ items, nextCursor: null }), inspectProject: vi.fn().mockResolvedValue({ isGit: true }) });
    expect(within(dialog).getAllByRole("button").map((button) => button.textContent)).toEqual([
      "compiler/projects/compiler", "renderer/projects/renderer", "Another folder…", "No folder· for questions that don't need code", "Advanced", "Cancel",
    ]);
    expect(within(dialog).getByRole("button", { name: /^compiler/ })).toHaveFocus();
    expect(within(dialog).queryByLabelText("In a Git folder, work in")).not.toBeInTheDocument();
    click(within(dialog).getByRole("button", { name: /^renderer/ }));
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith({
      title: "renderer", objective: "", constraints: [], workspace: { kind: "isolated", path: "/projects/renderer" }, routingProfile: "bestFit",
    }));
    expect(api.pickProjectDirectory).not.toHaveBeenCalled();
  });

  it("gives focus back to Another folder when the native picker is canceled", async () => {
    const { api, choose, dialog } = await openChooser({ pickProjectDirectory: vi.fn().mockResolvedValue(null) });
    click(choose);
    await waitFor(() => expect(choose).toBeEnabled());
    await waitFor(() => expect(choose).toHaveFocus());
    expect(api.inspectProject).not.toHaveBeenCalled();
    expect(api.createConversation).not.toHaveBeenCalled();
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });

  it.each(["picker", "preflight", "preparation"] as const)("recovers from %s errors without falling back", async (stage) => {
    const failing = vi.fn().mockRejectedValueOnce(new Error("Directory is unavailable"));
    const overrides: Partial<AppApi> = stage === "picker"
      ? { pickProjectDirectory: failing.mockResolvedValue("/tmp/synthetic-project") }
      : stage === "preflight"
        ? { inspectProject: failing.mockResolvedValue({ isGit: true }) }
        : { createConversation: failing.mockResolvedValue(createdConversation()) };
    const { api, dialog, choose } = await openChooser(overrides);
    click(choose);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Directory is unavailable");
    expect(api.createConversation).toHaveBeenCalledTimes(stage === "preparation" ? 1 : 0);
    click(choose);
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(api.createConversation).toHaveBeenLastCalledWith(expect.objectContaining({ workspace: { kind: "isolated", path: "/tmp/synthetic-project" } }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("uses a non-Git folder directly despite the isolation default", async () => {
    const { api, choose } = await openChooser({ inspectProject: vi.fn().mockResolvedValue({ isGit: false }) });
    click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith(expect.objectContaining({ workspace: { kind: "direct", path: "/tmp/synthetic-project" } })));
  });

  it.each(["balanced", "usageBalance"] as const)("honors Current checkout and the %s routing choice", async (routingProfile) => {
    const { api, choose, dialog } = await openChooser();
    click(within(dialog).getByRole("button", { name: "Advanced" }));
    fireEvent.change(within(dialog).getByLabelText("In a Git folder, work in"), { target: { value: "direct" } });
    fireEvent.change(within(dialog).getByLabelText("Routing profile"), { target: { value: routingProfile } });
    flush();
    click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith(expect.objectContaining({ routingProfile, workspace: { kind: "direct", path: "/tmp/synthetic-project" } })));
  });

  it.each([["/tmp/synthetic-project///", "synthetic-project"], ["/", "New chat"]])("titles %s as %s", async (path, title) => {
    const { api, choose } = await openChooser({ pickProjectDirectory: vi.fn().mockResolvedValue(path) });
    click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith(expect.objectContaining({ title })));
  });

  it("creates without a folder and sends only the ordinary first message", async () => {
    const submitMessage = vi.fn().mockResolvedValue({});
    const { api, dialog } = await openChooser({ submitMessage });
    click(within(dialog).getByRole("button", { name: /^No folder/ }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(api.createConversation).toHaveBeenCalledWith({ title: "New chat", objective: "", constraints: [], workspace: { kind: "projectless" }, routingProfile: "bestFit" });
    expect(api.pickProjectDirectory).not.toHaveBeenCalled();
    expect(api.inspectProject).not.toHaveBeenCalled();
    fireEvent.input(await screen.findByRole("textbox", { name: "Message" }), { target: { value: "Explain the synthetic project" } });
    flush();
    click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(submitMessage).toHaveBeenCalledWith({
      conversationId: "new-1", text: "Explain the synthetic project", providerOverride: null, commandId: expect.any(String),
      thinking: { kind: "auto" }, contextBudget: { kind: "tokens", tokens: 300000 },
    }));
  });

  it.each(["picker", "preflight"] as const)("ignores a late %s result after the app goes away", async (stage) => {
    const picker = deferred<string | null>();
    const preflight = deferred<{ isGit: boolean }>();
    const { api, choose, unmount } = await openChooser(stage === "picker"
      ? { pickProjectDirectory: vi.fn().mockReturnValue(picker.promise) }
      : { inspectProject: vi.fn().mockReturnValue(preflight.promise) });
    click(choose);
    if (stage === "preflight") await waitFor(() => expect(api.inspectProject).toHaveBeenCalledTimes(1));
    unmount();
    picker.resolve("/tmp/synthetic-project");
    preflight.resolve({ isGit: true });
    await settle();
    expect(api.createConversation).not.toHaveBeenCalled();
    if (stage === "picker") expect(api.inspectProject).not.toHaveBeenCalled();
  });

  it.each(["folder", "projectless"] as const)("admits only one same-tick operation starting with %s", async (intent) => {
    const picker = deferred<string | null>();
    const creation = deferred<ConversationSummary>();
    const { api, choose, dialog } = await openChooser({
      pickProjectDirectory: vi.fn().mockReturnValue(picker.promise),
      createConversation: vi.fn().mockReturnValue(creation.promise),
    });
    const projectless = within(dialog).getByRole("button", { name: /^No folder/ });
    for (const button of intent === "folder" ? [choose, choose, projectless] : [projectless, projectless, choose]) {
      button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    }
    expect(api.pickProjectDirectory).toHaveBeenCalledTimes(intent === "folder" ? 1 : 0);
    expect(api.createConversation).toHaveBeenCalledTimes(intent === "projectless" ? 1 : 0);
    picker.resolve("/tmp/synthetic-project");
    await settle();
    expect(api.createConversation).toHaveBeenCalledTimes(1);
    creation.resolve(createdConversation());
    await settle();
  });

  it("keeps options and Cancel disabled through the picker, the folder check, and creation", async () => {
    const picker = deferred<string | null>();
    const preflight = deferred<{ isGit: boolean }>();
    const creation = deferred<ConversationSummary>();
    const { choose, dialog } = await openChooser({
      pickProjectDirectory: vi.fn().mockReturnValue(picker.promise),
      inspectProject: vi.fn().mockReturnValue(preflight.promise),
      createConversation: vi.fn().mockReturnValue(creation.promise),
    });
    click(within(dialog).getByRole("button", { name: "Advanced" }));
    click(choose);
    const checkBusy = () => {
      expect(dialog).toHaveAttribute("aria-busy", "true");
      for (const control of within(dialog).getAllByRole("button")) expect(control).toBeDisabled();
      for (const control of within(dialog).getAllByRole("combobox")) expect(control).toBeDisabled();
      fireEvent.keyDown(dialog, { key: "Escape" });
      flush();
      expect(dialog).toBeInTheDocument();
      expect(fireEvent.keyDown(dialog, { key: "Tab" })).toBe(false);
    };
    checkBusy();
    picker.resolve("/tmp/synthetic-project");
    await settle();
    checkBusy();
    preflight.resolve({ isGit: true });
    await settle();
    checkBusy();
    creation.resolve(createdConversation());
    await settle();
  });

  it("keeps Tab inside the chooser, over only the Advanced controls shown, and gives focus back on Cancel", async () => {
    const { dialog, choose, trigger } = await openChooser();
    const advanced = within(dialog).getByRole("button", { name: "Advanced" });
    const cancel = within(dialog).getByRole("button", { name: "Cancel" });
    expect(advanced).toHaveAttribute("aria-expanded", "false");
    expect(within(dialog).queryByLabelText("In a Git folder, work in")).not.toBeInTheDocument();
    choose.focus();
    fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true });
    expect(cancel).toHaveFocus();
    fireEvent.keyDown(dialog, { key: "Tab" });
    expect(choose).toHaveFocus();
    click(advanced);
    expect(advanced).toHaveAttribute("aria-expanded", "true");
    expect(within(dialog).getByLabelText("In a Git folder, work in")).toBeVisible();
    click(cancel);
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("closes a narrow inspector overlay and focuses the new conversation's message", async () => {
    mediaQueries(narrowQuery);
    const { dialog } = await openChooser({}, true);
    expect(document.querySelector(".command-center")).toHaveAttribute("inert");
    click(within(dialog).getByRole("button", { name: /^No folder/ }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
    expect(document.querySelector(".command-center")).not.toHaveAttribute("inert");
  });

  it("keeps an existing conversation's Balanced routing after creating a Best fit one", async () => {
    const saved = createdConversation({ id: "saved", title: "Saved Balanced", routingProfile: "balanced" });
    const { store, dialog } = await openChooser({ listConversations: vi.fn().mockResolvedValue({ items: [saved], nextCursor: null }) });
    click(within(dialog).getByRole("button", { name: /^No folder/ }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(store.getSnapshot().conversationsById.saved!.routingProfile).toBe("balanced");
    await store.selectConversation("saved");
    await settle();
    expect(screen.getByRole("option", { name: "Auto · Balanced" })).toBeInTheDocument();
  });

  it("asks before archiving the current conversation, then clears the workspace", async () => {
    const archiveConversation = vi.fn().mockResolvedValue(undefined);
    renderApp(createApi({ archiveConversation }));
    const more = await screen.findByRole("button", { name: "More" });
    click(more);
    click(await screen.findByRole("menuitem", { name: "Archive conversation" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Archive Auth refactor" });
    expect(document.querySelector(".command-center")).toHaveAttribute("inert");
    expect(archiveConversation).not.toHaveBeenCalled();
    click(within(dialog).getByRole("button", { name: "Confirm archive" }));
    await waitFor(() => expect(archiveConversation).toHaveBeenCalledWith({ conversationId: "c1" }));
    expect(await screen.findByText("No conversation selected")).toBeVisible();
  });

  it("closes the archive confirmation on Escape or Keep conversation without archiving", async () => {
    const archiveConversation = vi.fn();
    renderApp(createApi({ archiveConversation }));
    const more = await screen.findByRole("button", { name: "More" });
    click(more);
    click(await screen.findByRole("menuitem", { name: "Archive conversation" }));
    fireEvent.keyDown(await screen.findByRole("alertdialog", { name: "Archive Auth refactor" }), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    click(more);
    click(await screen.findByRole("menuitem", { name: "Archive conversation" }));
    click(within(await screen.findByRole("alertdialog", { name: "Archive Auth refactor" })).getByRole("button", { name: "Keep conversation" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(document.querySelector(".command-center")).not.toHaveAttribute("inert");
    expect(archiveConversation).not.toHaveBeenCalled();
  });

  it("resizes All work by dragging its edge or with the arrow keys, within 220 to 520 pixels, and keeps the width across launches", async () => {
    const first = renderApp(createApi());
    const edge = await screen.findByRole("separator", { name: "Resize All work" });
    const pane = () => screen.getByRole("complementary", { name: "All work" });
    expect(edge).toHaveAttribute("aria-valuenow", "300");
    expect(pane().style.getPropertyValue("--sidebar-width")).toBe("300px");
    fireEvent.keyDown(edge, { key: "ArrowRight" });
    flush();
    await settle();
    expect(edge).toHaveAttribute("aria-valuenow", "316");
    expect(window.localStorage.getItem("prompting-time.sidebar-width")).toBe("316");
    fireEvent.pointerDown(edge, { clientX: 100, pointerId: 1 });
    fireEvent.pointerMove(edge, { clientX: 500, pointerId: 1 });
    flush();
    expect(pane().style.getPropertyValue("--sidebar-width")).toBe("520px");
    fireEvent.pointerMove(edge, { clientX: -400, pointerId: 1 });
    flush();
    expect(edge).toHaveAttribute("aria-valuenow", "220");
    fireEvent.pointerMove(edge, { clientX: 150, pointerId: 1 });
    fireEvent.pointerUp(edge, { clientX: 150, pointerId: 1 });
    flush();
    expect(window.localStorage.getItem("prompting-time.sidebar-width")).toBe("366");

    first.unmount();
    renderApp(createApi());
    expect(await screen.findByRole("separator", { name: "Resize All work" })).toHaveAttribute("aria-valuenow", "366");
  });

  it("keeps Search in the header and moves New chat there when All work is hidden", async () => {
    renderApp(createApi());
    await screen.findByRole("heading", { level: 1, name: "Auth refactor" });
    expect(screen.getAllByRole("button", { name: "New chat" })).toHaveLength(1);
    expect(screen.getByRole("complementary", { name: "All work" })).toContainElement(screen.getByRole("button", { name: "New chat" }));
    expect(screen.getByRole("banner")).toContainElement(screen.getByRole("button", { name: "Search (⌘K)" }));
    click(screen.getByRole("button", { name: "Hide All work" }));
    expect(screen.getAllByRole("button", { name: "Search (⌘K)" })).toHaveLength(1);
    const create = screen.getByRole("button", { name: "New chat" });
    expect(screen.getByRole("banner")).toContainElement(create);
    click(create);
    click(within(screen.getByRole("dialog", { name: "New chat" })).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "New chat" })).toHaveFocus());
  });
});

describe("palette and the composer", () => {
  async function openWorkspace(items = [
    createdConversation({ id: "a", title: "Alpha", projectRoot: "/projects/compiler" }),
    createdConversation({ id: "b", title: "Beta", projectRoot: "/projects/renderer" }),
  ]) {
    const view = renderApp(createApi({ listConversations: vi.fn().mockResolvedValue({ items, nextCursor: null }) }));
    await screen.findByRole("button", { name: "New chat" });
    return view;
  }
  async function openPalette() {
    click(screen.getByRole("button", { name: "Search (⌘K)" }));
    return screen.findByRole("combobox", { name: "Search conversations, reviews, and commands" });
  }
  const choose = (search: HTMLElement, text: string) => {
    fireEvent.input(search, { target: { value: text } });
    flush();
    fireEvent.keyDown(search, { key: "Enter" });
    flush();
  };

  it("opens over the message with ⌘K, and Escape gives back focus, selection, and the raw draft", async () => {
    await openWorkspace();
    const message = screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement;
    fireEvent.input(message, { target: { value: "Raw **draft**\n\nkeep" } });
    flush();
    message.focus();
    message.setSelectionRange(4, 9);
    press("k");
    const search = await screen.findByRole("combobox", { name: "Search conversations, reviews, and commands" });
    await waitFor(() => expect(search).toHaveFocus());
    fireEvent.keyDown(search, { key: "Escape" });
    await waitFor(() => expect(message).toHaveFocus());
    expect(message).toHaveValue("Raw **draft**\n\nkeep");
    expect([message.selectionStart, message.selectionEnd]).toEqual([4, 9]);
  });

  it("opens a conversation from search with its own draft and focuses its message", async () => {
    const { store } = await openWorkspace();
    fireEvent.input(screen.getByRole("textbox", { name: "Message" }), { target: { value: "Alpha draft" } });
    flush();
    choose(await openPalette(), "renderer");
    await waitFor(() => expect(store.getSnapshot().selectedConversationId).toBe("b"));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("");
    choose(await openPalette(), "Alpha");
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("Alpha draft"));
  });

  it.each(["Focus message", "Alpha"])("leaves preview and focuses the raw message through %s", async (command) => {
    await openWorkspace();
    fireEvent.input(screen.getByRole("textbox", { name: "Message" }), { target: { value: "**preserved**" } });
    flush();
    click(screen.getByRole("button", { name: "Preview Markdown" }));
    choose(await openPalette(), command);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("**preserved**");
  });

  it("opens the chooser from New chat and focuses the new message after creating", async () => {
    await openWorkspace();
    choose(await openPalette(), "New chat");
    const dialog = await screen.findByRole("dialog", { name: "New chat" });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: /^compiler/ })).toHaveFocus());
    expect(screen.queryByRole("combobox", { name: "Search conversations, reviews, and commands" })).not.toBeInTheDocument();
    press("k");
    await settle();
    expect(screen.queryByRole("combobox", { name: "Search conversations, reviews, and commands" })).not.toBeInTheDocument();
    click(within(dialog).getByRole("button", { name: /^No folder/ }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
  });

  it("disables Focus message in an empty workspace", async () => {
    await openWorkspace([]);
    await openPalette();
    expect(screen.getByRole("option", { name: /Focus message/ })).toHaveAttribute("aria-disabled", "true");
  });
});

function createdConversation(overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return {
    contextBudget: { kind: "tokens", tokens: 300000 }, runContextBudget: null, thinkingPreference: { kind: "auto" }, thinkingDecision: null, thinkingConfiguration: null,
    id: "new-1", title: "synthetic-project", routingProfile: "bestFit", workspaceId: "w-1",
    parentId: null, hasChildren: false, summary: null,
    capabilities: { canSend: true, canInterrupt: true, canArchive: true, canRoute: true, unavailableReason: null },
    archived: false, projectRoot: "/tmp/synthetic-project", currentRunId: null, provider: null,
    runStatus: null, rollupStatus: null,
    ...overrides,
  };
}


function createApi(overrides: Partial<AppApi> = {}): AppApi {
  const root = createdConversation({ id: "c1", title: "Auth refactor", projectRoot: null, workspaceId: null,
    hasChildren: true, currentRunId: "run-1", provider: "codex", runStatus: "queued", rollupStatus: "active" });
  const child = createdConversation({ ...root, id: "child-1", title: "Reviewer", parentId: "c1",
    hasChildren: false, summary: "Reviewing", provider: "claude", runStatus: "running",
    capabilities: { canSend: false, canInterrupt: false, canArchive: false, canRoute: false,
      unavailableReason: "Recorded child activity is read-only." } });
  return {
    ...noPlugins(),
    setContextBudget: vi.fn(), setThinkingPreference: vi.fn(),
    getBootstrap: vi.fn().mockResolvedValue({
      providers: [
        {
          id: "codex",
          installed: true,
          available: true,
          version: "0.144.1",
          diagnostic: null,
          capabilities: [],
        },
        {
          id: "claude",
          installed: true,
          available: false,
          version: "2.1.205",
          diagnostic: "Claude protocol gate has not passed.",
          capabilities: [],
        },
      ],
    }),
    listConversations: vi.fn().mockResolvedValue({ items: [root], nextCursor: null }),
    loadConversation: vi.fn(async ({ conversationId }) => conversationId === child.id ? child : root),
    listChildConversations: vi.fn(async ({ parentId }) => ({ items: parentId === root.id ? [child] : [], nextCursor: null })),
    loadConversationPath: vi.fn(async ({ conversationId }) => ({
      items: conversationId === child.id ? [root, child] : [root], truncated: false, ownerConversationId: root.id,
    })),
    loadTimeline: vi.fn().mockResolvedValue({
      items: [], nextCursor: null, approvals: [], approvalsTruncated: false, activeCompaction: null, approvalsNextCursor: null,
    }),
    loadDiagnostics: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadEventDetail: vi.fn(),
    loadApprovals: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadApprovalDetail: vi.fn(),
    loadApprovalQuestions: vi.fn(),
    submitMessage: vi.fn(),
    steerRun: vi.fn(),
    respondToApproval: vi.fn(),
    interruptRun: vi.fn(),
    inspectWorkspace: vi.fn().mockResolvedValue({
      workspace: { mode: "projectless", changes: [], truncated: false },
      executionPath: "/tmp/prompting-time/conversation-1",
      ownedWorktree: false,
      cleanup: { eligible: false, blocker: "notOwned" },
      currentRun: { id: "run-1", provider: "codex", status: "queued" },
      routing: null,
      handoff: null,
      activeDescendantCount: 1,
    }),
    inspectProject: vi.fn().mockResolvedValue({ isGit: true }),
    pickProjectDirectory: vi.fn().mockResolvedValue("/tmp/synthetic-project"),
    listRunAudits: vi.fn().mockResolvedValue({ items: [], nextCursor: null }),
    loadRunAudit: vi.fn(),
    createConversation: vi.fn().mockResolvedValue(createdConversation()),
    archiveConversation: vi.fn(),
    listenToAppEvents: vi.fn().mockResolvedValue(() => {}),
    ...overrides,
  };
}
