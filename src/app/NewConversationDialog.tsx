import { createSignal, For, onSettled, Show } from "solid-js";
import { Button, ChatIcon, ChevronDownIcon, ChevronRightIcon, FolderIcon, FolderOpenIcon, Select } from "@prompting-time/ui";
import type { CreateConversationRequest } from "../bridge/types";

type Props = {
  /** The folders of recent chats, most recent first, each once. */
  recentFolders: readonly string[];
  pickProjectDirectory(): Promise<string | null>;
  inspectProject(path: string): Promise<{ isGit: boolean }>;
  onCancel(): void;
  onCreate(request: CreateConversationRequest): Promise<void>;
  onCreated(): void;
};

/** Where the agent works: a recent folder, one picked in the native picker, or no folder. */
type Place = { folder: string } | "another" | "none";

const folderName = (path: string) => path.replace(/\/+$/, "").split("/").pop() || "New chat";

/**
 * New chat asks where the agent should work: one of the recent chats' folders, another folder from
 * the native picker, or no folder, for questions that need no code. A chosen folder is checked,
 * and the chat is created with the folder's name as its title. Advanced, folded, picks the current
 * Git checkout instead of an isolated worktree, or the routing. One pick, check, or create runs at
 * a time, and closing the dialog drops results that arrive later.
 */
export function NewConversationDialog(props: Props) {
  const [error, setError] = createSignal<string | null>(null);
  const [executionMode, setExecutionMode] = createSignal<"isolated" | "direct">("isolated");
  const [routingProfile, setRoutingProfile] = createSignal<CreateConversationRequest["routingProfile"]>("bestFit");
  const [advanced, setAdvanced] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  let pending = false;
  let active = true;
  let places: HTMLUListElement | undefined;
  // The first place is the most recent folder, or Another folder… when there is none.
  const focusFirstPlace = () => places?.querySelector("button")?.focus();

  onSettled(() => {
    focusFirstPlace();
    return () => {
      active = false;
    };
  });

  function cancel() {
    if (!pending) props.onCancel();
  }

  async function create(place: Place) {
    if (pending) return;
    pending = true;
    const options = { executionMode: executionMode(), routingProfile: routingProfile() };
    setBusy(true);
    setError(null);
    try {
      let title = "New chat";
      let workspace: CreateConversationRequest["workspace"] = { kind: "projectless" };
      if (place !== "none") {
        const path = place === "another" ? await props.pickProjectDirectory() : place.folder;
        if (!active || path === null) return;
        const { isGit } = await props.inspectProject(path);
        if (!active) return;
        title = folderName(path);
        workspace = { kind: isGit ? options.executionMode : "direct", path };
      }
      await props.onCreate({ title, objective: "", constraints: [], workspace, routingProfile: options.routingProfile });
      if (active) props.onCreated();
    } catch (reason) {
      if (active) setError(reason instanceof Error ? reason.message : "Prompting Time could not create the chat.");
    } finally {
      pending = false;
      if (active) {
        setBusy(false);
        queueMicrotask(focusFirstPlace);
      }
    }
  }

  return (
    <div class="dialog-backdrop">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-conversation-title"
        aria-describedby="new-conversation-question"
        class="lifecycle-dialog new-chat"
        aria-busy={busy() ? "true" : "false"}
        onKeyDown={(event) => trapDialogKeys(event, cancel, pending)}
      >
        <h2 id="new-conversation-title">New chat</h2>
        <p id="new-conversation-question">Where should the agent work?</p>
        <Show when={error()}><p role="alert" class="inline-error">{error()}</p></Show>
        <ul ref={places} class="new-chat-places">
          <For each={props.recentFolders}>
            {(folder) => (
              <li>
                <button type="button" class="new-chat-place" disabled={busy()} onClick={() => void create({ folder })}>
                  <FolderIcon />
                  <span class="new-chat-place-name">{folderName(folder)}</span>
                  <span class="new-chat-place-detail new-chat-place-path">{folder}</span>
                </button>
              </li>
            )}
          </For>
          <li>
            <button type="button" class="new-chat-place" disabled={busy()} onClick={() => void create("another")}>
              <FolderOpenIcon />
              <span class="new-chat-place-name">Another folder…</span>
            </button>
          </li>
          <li>
            <button type="button" class="new-chat-place" disabled={busy()} onClick={() => void create("none")}>
              <ChatIcon />
              <span class="new-chat-place-name">No folder</span>
              <span class="new-chat-place-detail">· for questions that don't need code</span>
            </button>
          </li>
        </ul>
        <button
          type="button"
          class="new-chat-advanced"
          aria-expanded={advanced() ? "true" : "false"}
          aria-controls="new-conversation-advanced"
          disabled={busy()}
          onClick={() => setAdvanced((open) => !open)}
        >
          {advanced() ? <ChevronDownIcon /> : <ChevronRightIcon />}
          Advanced
        </button>
        <Show when={advanced()}>
          <div id="new-conversation-advanced" class="conversation-options">
            <Select
              label="In a Git folder, work in"
              hint="An isolated worktree starts from the last commit, without your uncommitted edits. A folder without Git is always used as it is."
              value={executionMode()}
              disabled={busy()}
              options={[{ value: "isolated", label: "An isolated worktree" }, { value: "direct", label: "The current checkout" }]}
              onChange={(event) => setExecutionMode(event.currentTarget.value as "isolated" | "direct")}
            />
            <Select
              label="Routing profile"
              value={routingProfile()}
              disabled={busy()}
              options={[{ value: "bestFit", label: "Best fit" }, { value: "balanced", label: "Balanced" }, { value: "usageBalance", label: "Usage balance" }]}
              onChange={(event) => setRoutingProfile(event.currentTarget.value as CreateConversationRequest["routingProfile"])}
            />
          </div>
        </Show>
        <div class="dialog-actions">
          <Button disabled={busy()} onClick={cancel}>Cancel</Button>
        </div>
      </div>
    </div>
  );
}

/** Keeps Tab inside the dialog and lets Escape cancel, except while work runs. */
function trapDialogKeys(event: KeyboardEvent, onCancel: () => void, busy: boolean) {
  if (event.key === "Escape" && !busy) {
    event.preventDefault();
    onCancel();
    return;
  }
  if (event.key !== "Tab") return;
  const controls = [...(event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>("button:not(:disabled), select:not(:disabled), input:not(:disabled), textarea:not(:disabled)")];
  const first = controls[0];
  const last = controls.at(-1);
  if (!first || !last) {
    event.preventDefault();
    return;
  }
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}
