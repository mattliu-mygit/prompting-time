import { fireEvent, screen, waitFor, within } from "@testing-library/dom";
import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BridgeError } from "../bridge/api";
import type { AppEvent, ConversationSummary, ListChildConversationsRequest, LoadConversationRequest, PluginActionRequest, TimelineItem } from "../bridge/types";
import { axeViolations, render, settle } from "../test/render";
import { App } from "./App";
import { createAppStore, type AppApi, type AppStore } from "./store";
import { actionResult, fixturePlugin, fixtureReviews, fixtureSettings, fixtureShown, fixtureStatus, noPlugins, pageActions } from "../test/plugins";

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

beforeEach(() => {
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
    const needsYou = await screen.findByRole("tree", { name: "Needs you" });
    expect(within(needsYou).getAllByRole("treeitem").map((row) => row.getAttribute("aria-label"))).toEqual([
      "Evaluation rows, Codex, waiting for you", "Tag names, Codex, waiting for you",
    ]);
    expect(screen.getAllByRole("tree").map((tree) => tree.getAttribute("aria-labelledby") && document.getElementById(tree.getAttribute("aria-labelledby")!)?.textContent))
      .toEqual(["Needs you", "synthetic-project"]);
  });

  it("jumps to the next root waiting on you with ⌘J or Ctrl+J, wrapping around", async () => {
    const { store } = renderApp(waitingApi());
    await screen.findByRole("tree", { name: "Needs you" });
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

    fireEvent.click(await screen.findByRole("treeitem", { name: /^separate cache configuration, 2 findings/ }));
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
    expect(within(failed).getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(within(failed).getByRole("button", { name: "Ask why" })).toBeInTheDocument();
    expect(pluginRequest).toHaveBeenCalledWith({ plugin: "whooper", action: "show-review", args: [FAILED], confirmedFingerprint: null });
    expect(screen.queryByText(/has no findings to show/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("treeitem", { name: /^separate cache configuration, 2 findings/ }));
    await settle();
    expect(frame().element).toHaveAttribute("src", "pt-plugin://whooper/index.html");
    expect(screen.queryByRole("heading", { name: "Review failed" })).not.toBeInTheDocument();
  });

  it("lists past reviews under the agent and opens one with what was submitted", async () => {
    const { api } = pagesApi();
    renderApp(api);
    await settle();
    await settle();

    fireEvent.click(await screen.findByRole("button", { name: /^Past reviews 4/ }));
    await settle();
    const rows = within(screen.getByRole("main", { name: "Past reviews" })).getAllByRole("button");
    expect(rows.map((row) => row.querySelector(".pt-list-row-title")?.textContent)).toEqual(["separate cache configuration", "add the tags package", "separate cache configuration", "move the projection"]);
    expect(rows[1]).toHaveTextContent("gadgets#77 · commented");

    fireEvent.click(rows[1]!);
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

  it("shows each agent as a quiet row after the work groups, from its process and status", async () => {
    renderApp(agentsApi().api);
    const row = await screen.findByRole("button", { name: `Whooper running · next poll ${shortTime(nextPoll())}` });
    expect(row.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "success");
    const heading = screen.getByRole("heading", { name: "Background agents" });
    const workHeading = screen.getByRole("heading", { name: "No project" });
    expect(workHeading.compareDocumentPosition(heading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("reads the agent again when a pluginChanged event arrives", async () => {
    let reviewing: string | null = null;
    const fake = agentsApi(fixturePlugin(), () => fixtureStatus(nextPoll().toISOString(),
      reviewing ? { round: { running: true, started_at: null, reviewing } } : {}));
    renderApp(fake.api);
    await screen.findByRole("button", { name: /^Whooper running/ });
    reviewing = "acme/widgets#101";
    fake.emit({ kind: "pluginChanged", sequence: "1", plugin: "whooper", files: ["round.lock/round.json"] });

    const row = await screen.findByRole("button", { name: "Whooper reviewing acme/widgets#101" });
    expect(row.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "running");
  });

  it("shows what is missing on an agent that cannot start", async () => {
    const stopped = fixturePlugin({
      process: { kind: "notRunning", exit: null },
      prerequisites: [{ command: "slackdump", purpose: "Reads the review asks.", needed: true, path: null }],
    });
    const { api } = agentsApi(stopped);
    renderApp(api);
    const row = await screen.findByRole("button", { name: "Whooper missing slackdump" });
    expect(row.querySelector(".pt-status-dot")).toHaveAttribute("data-status", "attention");
    expect(api.pluginRequest).not.toHaveBeenCalled();
  });

  it("opens an agent's settings from its row and goes back to a conversation from All work", async () => {
    renderApp(agentsApi().api);
    click(await screen.findByRole("button", { name: /^Whooper running/ }));

    expect(await screen.findByRole("heading", { name: "Whooper settings", level: 1 })).toBeInTheDocument();
    expect(await screen.findByRole("spinbutton", { name: "Poll every" })).toHaveValue(15);
    expect(screen.getByRole("button", { name: /^Whooper running/ })).toHaveAttribute("aria-current", "true");
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
    expect(await screen.findByText("None installed.")).toBeInTheDocument();
    click(screen.getByRole("button", { name: "Add background agent" }));
    click(await screen.findByRole("menuitem", { name: "Install from a release archive…" }));

    const dialog = await screen.findByRole("alertdialog", { name: "Install from a release archive" });
    expect(dialog).toHaveTextContent("/synthetic/release/whooper-0.1.0.tar.gz");
    const install = within(dialog).getByRole("button", { name: "Install" });
    expect(install).toBeDisabled();
    fireEvent.input(within(dialog).getByRole("textbox", { name: "SHA-256" }), { target: { value: ` ${"C".repeat(64)} ` } });
    flush();
    click(install);

    expect(await screen.findByRole("heading", { name: "Whooper settings", level: 1 })).toBeInTheDocument();
    expect(api.installPlugin).toHaveBeenCalledExactlyOnceWith({ source: { kind: "archive", path: "/synthetic/release/whooper-0.1.0.tar.gz", sha256: "c".repeat(64) } });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
  });

  it("runs a plugin folder in place, and keeps a refused install in its dialog", async () => {
    const { api } = agentsApi();
    api.pickPluginFolder = vi.fn().mockResolvedValue("/synthetic/whooper");
    api.installPlugin = vi.fn().mockRejectedValue(new BridgeError("plugin-manifest-invalid", "The plugin's manifest is invalid: fixture reason", null));
    renderApp(api);
    click(await screen.findByRole("button", { name: "Add background agent" }));
    click(await screen.findByRole("menuitem", { name: "Run a plugin folder in place…" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Run a plugin folder in place" });
    click(within(dialog).getByRole("button", { name: "Install" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("The plugin's manifest is invalid: fixture reason");
    expect(api.installPlugin).toHaveBeenCalledExactlyOnceWith({ source: { kind: "folder", path: "/synthetic/whooper" } });
    click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
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
  const combobox = () => screen.findByRole("combobox", { name: "Search conversations and commands" });

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
    expect(screen.queryByRole("combobox", { name: "Search conversations and commands" })).not.toBeInTheDocument();
  });

  it("toggles closed with Ctrl+K and ignores composition and held keys", async () => {
    await openWorkspace();
    press("k", { isComposing: true });
    press("k", { repeat: true });
    await settle();
    expect(screen.queryByRole("combobox", { name: "Search conversations and commands" })).not.toBeInTheDocument();
    press("k", { metaKey: false, ctrlKey: true });
    await combobox();
    press("k", { metaKey: false, ctrlKey: true });
    await waitFor(() => expect(screen.queryByRole("combobox", { name: "Search conversations and commands" })).not.toBeInTheDocument());
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
    expect(screen.queryByRole("combobox", { name: "Search conversations and commands" })).not.toBeInTheDocument();
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
    expect(screen.getByText("No conversations match this status.")).toBeVisible();
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
    expect(screen.queryByRole("combobox", { name: "Search conversations and commands" })).not.toBeInTheDocument();
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
    expect(screen.queryByRole("combobox", { name: "Search conversations and commands" })).not.toBeInTheDocument();
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
    const trigger = await screen.findByRole("button", { name: "New conversation" });
    if (inspectorOpen) click(screen.getByRole("button", { name: "Show inspector" }));
    click(trigger);
    const dialog = screen.getByRole("dialog", { name: "New conversation" });
    return { ...view, api, trigger, dialog, choose: within(dialog).getByRole("button", { name: "Choose folder" }) };
  }

  it("chooses a Git folder and creates an empty Best fit conversation from an empty install", async () => {
    const { api, dialog, choose, store } = await openChooser({ listConversations: vi.fn().mockResolvedValue({ items: [], nextCursor: null }) });
    expect(choose).toHaveFocus();
    expect(within(dialog).queryByLabelText("Title")).not.toBeInTheDocument();
    expect(dialog).toHaveTextContent(/uncommitted/i);
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

  it("gives focus back to Choose folder when the native picker is canceled", async () => {
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
    fireEvent.change(within(dialog).getByLabelText("Git execution"), { target: { value: "direct" } });
    fireEvent.change(within(dialog).getByLabelText("Routing profile"), { target: { value: routingProfile } });
    flush();
    click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith(expect.objectContaining({ routingProfile, workspace: { kind: "direct", path: "/tmp/synthetic-project" } })));
  });

  it.each([["/tmp/synthetic-project///", "synthetic-project"], ["/", "New conversation"]])("titles %s as %s", async (path, title) => {
    const { api, choose } = await openChooser({ pickProjectDirectory: vi.fn().mockResolvedValue(path) });
    click(choose);
    await waitFor(() => expect(api.createConversation).toHaveBeenCalledWith(expect.objectContaining({ title })));
  });

  it("creates without a folder and sends only the ordinary first message", async () => {
    const submitMessage = vi.fn().mockResolvedValue({});
    const { api, dialog } = await openChooser({ submitMessage });
    click(within(dialog).getByRole("button", { name: "Without a folder" }));
    await waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(api.createConversation).toHaveBeenCalledWith({ title: "New conversation", objective: "", constraints: [], workspace: { kind: "projectless" }, routingProfile: "bestFit" });
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
    const projectless = within(dialog).getByRole("button", { name: "Without a folder" });
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
    expect(within(dialog).queryByLabelText("Git execution")).not.toBeInTheDocument();
    choose.focus();
    fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true });
    expect(cancel).toHaveFocus();
    fireEvent.keyDown(dialog, { key: "Tab" });
    expect(choose).toHaveFocus();
    click(advanced);
    expect(advanced).toHaveAttribute("aria-expanded", "true");
    expect(within(dialog).getByLabelText("Git execution")).toBeVisible();
    click(cancel);
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("closes a narrow inspector overlay and focuses the new conversation's message", async () => {
    mediaQueries(narrowQuery);
    const { dialog } = await openChooser({}, true);
    expect(document.querySelector(".command-center")).toHaveAttribute("inert");
    click(within(dialog).getByRole("button", { name: "Without a folder" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveFocus());
    expect(screen.queryByRole("complementary", { name: "Inspector" })).not.toBeInTheDocument();
    expect(document.querySelector(".command-center")).not.toHaveAttribute("inert");
  });

  it("keeps an existing conversation's Balanced routing after creating a Best fit one", async () => {
    const saved = createdConversation({ id: "saved", title: "Saved Balanced", routingProfile: "balanced" });
    const { store, dialog } = await openChooser({ listConversations: vi.fn().mockResolvedValue({ items: [saved], nextCursor: null }) });
    click(within(dialog).getByRole("button", { name: "Without a folder" }));
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

  it("moves New conversation and Search into the header when All work is hidden", async () => {
    renderApp(createApi());
    await screen.findByRole("heading", { level: 1, name: "Auth refactor" });
    expect(screen.getAllByRole("button", { name: "New conversation" })).toHaveLength(1);
    click(screen.getByRole("button", { name: "Hide All work" }));
    const create = screen.getByRole("button", { name: "New conversation" });
    expect(screen.getByRole("banner")).toContainElement(create);
    click(create);
    click(within(screen.getByRole("dialog", { name: "New conversation" })).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "New conversation" })).toHaveFocus());
  });
});

describe("palette and the composer", () => {
  async function openWorkspace(items = [
    createdConversation({ id: "a", title: "Alpha", projectRoot: "/projects/compiler" }),
    createdConversation({ id: "b", title: "Beta", projectRoot: "/projects/renderer" }),
  ]) {
    const view = renderApp(createApi({ listConversations: vi.fn().mockResolvedValue({ items, nextCursor: null }) }));
    await screen.findByRole("button", { name: "New conversation" });
    return view;
  }
  async function openPalette() {
    click(screen.getByRole("button", { name: "Search (⌘K)" }));
    return screen.findByRole("combobox", { name: "Search conversations and commands" });
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
    const search = await screen.findByRole("combobox", { name: "Search conversations and commands" });
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

  it("opens the chooser from New conversation and focuses the new message after creating", async () => {
    await openWorkspace();
    choose(await openPalette(), "New conversation");
    const dialog = await screen.findByRole("dialog", { name: "New conversation" });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Choose folder" })).toHaveFocus());
    expect(screen.queryByRole("combobox", { name: "Search conversations and commands" })).not.toBeInTheDocument();
    press("k");
    await settle();
    expect(screen.queryByRole("combobox", { name: "Search conversations and commands" })).not.toBeInTheDocument();
    click(within(dialog).getByRole("button", { name: "Without a folder" }));
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
