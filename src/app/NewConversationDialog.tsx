import { createSignal, onSettled, Show } from "solid-js";
import { Button, Select } from "@prompting-time/ui";
import type { CreateConversationRequest } from "../bridge/types";

type Props = {
  pickProjectDirectory(): Promise<string | null>;
  inspectProject(path: string): Promise<{ isGit: boolean }>;
  onCancel(): void;
  onCreate(request: CreateConversationRequest): Promise<void>;
  onCreated(): void;
};

/**
 * The new-conversation chooser: Choose folder opens the native picker, checks the folder, and
 * creates the conversation; Without a folder creates a projectless one. Advanced changes routing or
 * picks the current Git checkout. One pick, check, or create runs at a time, and closing the
 * chooser drops results that arrive later.
 */
export function NewConversationDialog(props: Props) {
  const [error, setError] = createSignal<string | null>(null);
  const [executionMode, setExecutionMode] = createSignal<"isolated" | "direct">("isolated");
  const [routingProfile, setRoutingProfile] = createSignal<CreateConversationRequest["routingProfile"]>("bestFit");
  const [advanced, setAdvanced] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  let pending = false;
  let active = true;
  let chooseButton: HTMLButtonElement | undefined;

  onSettled(() => {
    chooseButton?.focus();
    return () => {
      active = false;
    };
  });

  function cancel() {
    if (!pending) props.onCancel();
  }

  async function create(intent: "folder" | "projectless") {
    if (pending) return;
    pending = true;
    const options = { executionMode: executionMode(), routingProfile: routingProfile() };
    setBusy(true);
    setError(null);
    try {
      let title = "New conversation";
      let workspace: CreateConversationRequest["workspace"] = { kind: "projectless" };
      if (intent === "folder") {
        const path = await props.pickProjectDirectory();
        if (!active || path === null) return;
        const { isGit } = await props.inspectProject(path);
        if (!active) return;
        title = path.replace(/\/+$/, "").split("/").pop() || title;
        workspace = { kind: isGit ? options.executionMode : "direct", path };
      }
      await props.onCreate({ title, objective: "", constraints: [], workspace, routingProfile: options.routingProfile });
      if (active) props.onCreated();
    } catch (reason) {
      if (active) setError(reason instanceof Error ? reason.message : "Prompting Time could not create the conversation.");
    } finally {
      pending = false;
      if (active) {
        setBusy(false);
        queueMicrotask(() => chooseButton?.focus());
      }
    }
  }

  return (
    <div class="dialog-backdrop">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-conversation-title"
        class="lifecycle-dialog"
        aria-busy={busy() ? "true" : "false"}
        onKeyDown={(event) => trapDialogKeys(event, cancel, pending)}
      >
        <h2 id="new-conversation-title">New conversation</h2>
        <p>Git folders use an isolated worktree by default, without uncommitted edits. Non-Git folders are used directly.</p>
        <Show when={error()}><p role="alert" class="inline-error">{error()}</p></Show>
        <Button ref={chooseButton} variant="primary" disabled={busy()} onClick={() => void create("folder")}>Choose folder</Button>
        <Button disabled={busy()} onClick={() => void create("projectless")}>Without a folder</Button>
        <Button aria-expanded={advanced() ? "true" : "false"} aria-controls="new-conversation-advanced" disabled={busy()} onClick={() => setAdvanced((open) => !open)}>
          Advanced
        </Button>
        <Show when={advanced()}>
          <div id="new-conversation-advanced" class="conversation-options">
            <Select
              label="Git execution"
              hint="This execution option applies only to Git folders."
              value={executionMode()}
              disabled={busy()}
              options={[{ value: "isolated", label: "Isolated worktree" }, { value: "direct", label: "Current checkout" }]}
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
