import { createEffect, createMemo, createSignal, createUniqueId, For, Show, untrack } from "solid-js";
import { Button, ConfirmDialog, HelpIcon, IconButton, Markdown, Textarea } from "@prompting-time/ui";
import type { ConversationSummary, ProviderId, ProviderInstallation, RoutingProfile, ThinkingPreference } from "../../bridge/types";
import { createSnapshot, type AppStore, type ConversationActions } from "../../app/store";

type ProviderChoice = "auto" | ProviderId;
type PendingInterruption = { choice: ProviderId | "interrupt"; provider: ProviderId; runId: string };

type ComposerProps = {
  /** One Composer writes to one conversation; the caller makes a new one for another conversation. */
  conversation: ConversationSummary;
  providers: readonly ProviderInstallation[];
  routingProfile: RoutingProfile;
  actions: Pick<ConversationActions, "interruptRun">;
  store: AppStore;
  onMutation(): void | Promise<void>;
  /** Called with true while the interruption dialog is open, so the app can make itself inert. */
  onModalChange?(open: boolean): void;
  messageRef?: (element: HTMLTextAreaElement) => void;
  /** Set to ask for focus in the message; the composer calls onFocusHandled once it has it. */
  focusRequested?: boolean;
  onFocusHandled?(): void;
};

const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
const profileNames: Record<RoutingProfile, string> = { balanced: "Balanced", bestFit: "Best fit", usageBalance: "Usage balance" };
const CONTEXT_BUDGETS = [200000, 300000, 400000, 500000];

/**
 * The message editor under a conversation's timeline. The draft, provider choice, and preview live
 * in the store under the conversation, so they survive switching away. Send starts a turn, Steer
 * adds direction to a running one where the provider supports it, and Interrupt stops a run after a
 * confirmation; switching provider during a run asks to interrupt it first.
 */
export function Composer(props: ComposerProps) {
  const conversationId = untrack(() => props.conversation.id);
  const snapshot = createSnapshot(untrack(() => props.store));
  const store = untrack(() => props.store);
  const helpId = createUniqueId();
  const [helpOpen, setHelpOpen] = createSignal(false);
  const [pendingInterruption, setPendingInterruption] = createSignal<PendingInterruption | null>(null);
  const [interruptRequestedFor, setInterruptRequestedFor] = createSignal<string | null>(null);
  const [localSubmitting, setSubmitting] = createSignal(false);
  const [localError, setError] = createSignal<string | null>(null);
  let messageField: HTMLTextAreaElement | undefined;
  let providerSelect: HTMLSelectElement | undefined;
  // Focus to restore once nothing blocks it: the message after a send or after preview, and a
  // composer control after the interruption dialog closes.
  let restoreDraftFocus = false;
  let restoreFocus = false;

  const text = () => snapshot().draftsById[conversationId]?.text ?? "";
  const submission = () => snapshot().submissionsById[conversationId];
  const thinkingSave = () => snapshot().thinkingSavesById[conversationId];
  const contextBudgetSave = () => snapshot().contextBudgetSavesById[conversationId];
  const thinkingPreference = () => thinkingSave()?.preference ?? props.conversation.thinkingPreference;
  const contextBudget = () => contextBudgetSave()?.preference ?? props.conversation.contextBudget;
  const preview = () => snapshot().composerViewsById[conversationId]?.preview ?? false;
  const setPreview = (next: boolean) => store.setComposerView(conversationId, { preview: next });
  const choice = (): ProviderChoice => snapshot().composerViewsById[conversationId]?.choice ?? submission()?.providerOverride ?? "auto";
  const setChoice = (next: ProviderChoice) => store.setComposerView(conversationId, { choice: next });
  const submitting = () => localSubmitting() || submission()?.pending === true;
  const error = () => localError() ?? contextBudgetSave()?.error ?? thinkingSave()?.error ?? submission()?.error ?? null;
  const retrying = () => submission()?.command != null && submission()?.error !== null;

  const runStatus = () => props.conversation.runStatus;
  const rootTurnActive = () => props.conversation.currentRunId !== null && (runStatus() === "queued" || runStatus() === "running" || runStatus() === "waiting");
  const descendantOnlyActive = () => props.conversation.currentRunId !== null && !rootTurnActive()
    && (props.conversation.rollupStatus === "active" || props.conversation.rollupStatus === "needsAttention");
  const active = () => rootTurnActive() || descendantOnlyActive();
  const interruptionPending = () => props.conversation.currentRunId !== null && props.conversation.currentRunId === interruptRequestedFor();
  const currentProvider = () => props.providers.find(({ id }) => id === props.conversation.provider);
  const canSteer = () => props.conversation.capabilities.canSend && runStatus() === "running" && !retrying() && !interruptionPending()
    && currentProvider()?.capabilities.includes("steering") === true;
  const canInterrupt = () => props.conversation.capabilities.canInterrupt && currentProvider()?.capabilities.includes("interruption") === true;
  const dialogOpen = () => pendingInterruption() !== null;
  const canSubmit = () => props.conversation.capabilities.canSend && !submitting() && !thinkingSave()?.pending && !contextBudgetSave()?.pending
    && !!text().trim() && (!active() || canSteer() || retrying()) && !dialogOpen();
  const configuration = () => (choice() === "auto" || choice() === props.conversation.provider ? props.conversation.thinkingConfiguration : null);
  const effortChoices = () => [...new Set(["low", "medium", "high", ...(configuration()?.supportedEfforts ?? [])])];
  const savedLevel = () => {
    const preference = thinkingPreference();
    return preference?.kind === "manual" ? preference.level : null;
  };
  const unreportedChoice = () => {
    const level = savedLevel();
    if (level === null) return false;
    const reported = configuration();
    return reported ? !reported.supportedEfforts.includes(level) : !effortChoices().includes(level);
  };
  const thinkingValue = () => (savedLevel() === null ? thinkingPreference()?.kind ?? "auto" : `manual:${savedLevel()}`);
  const activeName = () => (props.conversation.provider ? providerNames[props.conversation.provider] : "provider");

  // Focus asked for by the app (the palette's Focus message, a new conversation). Preview ends first.
  createEffect(
    () => ({ requested: !!props.focusRequested && props.conversation.capabilities.canSend, preview: preview(), blocked: submitting() || dialogOpen() }),
    ({ requested, preview: previewing, blocked }) => {
      if (!requested || blocked) return;
      if (previewing) {
        setPreview(false);
        return;
      }
      messageField?.focus();
      untrack(() => props.onFocusHandled)?.();
    },
  );

  createEffect(
    () => ({ preview: preview(), blocked: submitting() || dialogOpen() }),
    ({ preview: previewing, blocked }) => {
      if (!restoreDraftFocus || previewing || blocked) return;
      restoreDraftFocus = false;
      messageField?.focus();
    },
  );

  createEffect(dialogOpen, (open) => {
    const onModalChange = untrack(() => props.onModalChange);
    onModalChange?.(open);
    return open ? () => onModalChange?.(false) : undefined;
  });

  // An interruption is pending only for the run it was asked for, while that run is active.
  createEffect(
    () => ({ runId: props.conversation.currentRunId, active: active(), requested: interruptRequestedFor() }),
    ({ runId, active: running, requested }) => {
      if (requested !== null && (runId !== requested || !running)) setInterruptRequestedFor(null);
    },
  );

  // A confirmation is for one run of one provider; it closes when that run ends or changes.
  createEffect(
    () => ({ pending: pendingInterruption(), rootActive: rootTurnActive(), runId: props.conversation.currentRunId, provider: props.conversation.provider }),
    ({ pending, rootActive, runId, provider }) => {
      if (!pending || (rootActive && runId === pending.runId && provider === pending.provider)) return;
      restoreFocus = true;
      setPendingInterruption(null);
    },
  );

  // After the dialog closes, put focus back on the composer in the next frame, after the dialog has
  // returned focus to where it was.
  createEffect(
    () => submitting() || dialogOpen(),
    (blocked) => {
      if (!restoreFocus || blocked) return;
      const frame = window.requestAnimationFrame(() => {
        restoreFocus = false;
        untrack(restoreComposerFocus);
      });
      return () => window.cancelAnimationFrame(frame);
    },
  );

  function restoreComposerFocus() {
    if (providerSelect && !providerSelect.disabled) providerSelect.focus();
    else if (preview()) {
      restoreDraftFocus = true;
      setPreview(false);
    } else messageField?.focus();
  }

  function selectProvider(next: ProviderChoice) {
    if (!props.conversation.capabilities.canSend || !props.conversation.capabilities.canRoute) return;
    const { currentRunId, provider } = props.conversation;
    if (active() && currentRunId && provider && next !== "auto" && next !== provider) {
      setPendingInterruption({ choice: next, provider, runId: currentRunId });
      return;
    }
    setChoice(next);
    store.resetDraftCommand(conversationId);
  }

  function closeInterruptionDialog() {
    restoreFocus = true;
    setPendingInterruption(null);
  }

  async function confirmSwitch() {
    const pending = pendingInterruption();
    if (!props.conversation.capabilities.canSend || !pending || !rootTurnActive() || props.conversation.currentRunId !== pending.runId
      || props.conversation.provider !== pending.provider || submitting() || !canInterrupt()) return;
    const nextChoice = pending.choice === "interrupt" ? null : pending.choice;
    if (nextChoice) {
      const target = props.providers.find(({ id }) => id === nextChoice);
      if (!target?.available) {
        setPendingInterruption(null);
        setError(`${providerNames[nextChoice]} is unavailable: ${target?.diagnostic ?? "Check the provider installation and sign in."}`);
        restoreFocus = true;
        return;
      }
    }
    setSubmitting(true);
    setError(null);
    try {
      await props.actions.interruptRun({ conversationId, runId: pending.runId });
      setInterruptRequestedFor(pending.runId);
      if (nextChoice) setChoice(nextChoice);
      restoreFocus = true;
      setPendingInterruption(null);
      await props.onMutation();
    } catch (reason) {
      setError(messageFor(reason));
    } finally {
      setSubmitting(false);
    }
  }

  async function submitDraft() {
    if (!canSubmit()) return;
    const provider = choice() === "auto" ? null : (choice() as ProviderId);
    setSubmitting(true);
    setError(null);
    try {
      if (!(await store.submitDraft(conversationId, provider, canSteer() ? props.conversation.currentRunId : null))) return;
      setPreview(false);
      restoreDraftFocus = true;
      await props.onMutation();
    } catch (reason) {
      setError(messageFor(reason));
    } finally {
      setSubmitting(false);
    }
  }

  function onMessageKeyDown(event: KeyboardEvent) {
    if (event.key !== "Enter" || event.shiftKey || event.altKey || event.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    if (!event.repeat) void submitDraft();
  }

  const interruptionTitle = (pending: PendingInterruption) =>
    pending.choice === "interrupt" ? `Interrupt ${providerNames[pending.provider]}` : `Interrupt ${providerNames[pending.provider]} to switch provider`;
  const interruptionText = (pending: PendingInterruption) =>
    !canInterrupt()
      ? `${providerNames[pending.provider]} does not report interruption support. Wait for the active turn to finish before switching providers.`
      : pending.choice === "interrupt"
        ? `Stop the active ${providerNames[pending.provider]} run? Its completed activity will remain in the timeline.`
        : `Provider changes happen only between turns. Interrupt the active run before switching to ${providerNames[pending.choice]}.`;
  const interruptionConfirm = (pending: PendingInterruption) =>
    !canInterrupt() ? "Interruption unavailable" : pending.choice === "interrupt" ? `Interrupt ${providerNames[pending.provider]}` : `Interrupt and switch to ${providerNames[pending.choice]}`;
  // Keeps the dialog's words while it closes, after the pending interruption is gone.
  const shownInterruption = createMemo((previous: PendingInterruption | null | undefined) => pendingInterruption() ?? previous ?? null);

  return (
    <Show
      when={props.conversation.capabilities.canSend}
      fallback={
        <section class="composer composer-read-only" aria-label="Message composer">
          <p class="composer-explanation">
            {props.conversation.capabilities.unavailableReason ?? "This conversation is read-only."}
            <span> Captured activity may be incomplete.</span>
          </p>
        </section>
      }
    >
      <section class="composer" aria-label="Message composer" aria-busy={submitting() ? "true" : "false"} inert={dialogOpen()}>
        <div class="message-field" hidden={preview()}>
          <Textarea
            ref={(element: HTMLTextAreaElement) => {
              messageField = element;
              props.messageRef?.(element);
            }}
            label="Message"
            grow
            rows={2}
            value={text()}
            disabled={submitting()}
            placeholder={active() ? `Add direction for ${activeName()}` : "Ask Prompting Time…"}
            onInput={(event) => store.setDraft(conversationId, event.currentTarget.value)}
            onKeyDown={onMessageKeyDown}
          />
        </div>
        <Show when={preview()}>
          <section class="composer-preview" aria-label="Message preview">
            <Show when={text()} fallback={<p class="empty-note">Nothing to preview.</p>}>
              <Markdown content={text()} />
            </Show>
          </section>
        </Show>
        <Show when={error() && !pendingInterruption()}>
          <p role="alert" class="inline-error">{error()}</p>
        </Show>
        <Show when={active() && !canSteer() && !retrying()}>
          <p class="composer-explanation">
            {interruptionPending()
              ? `Waiting for ${activeName()} to stop before another turn.`
              : descendantOnlyActive()
                ? "Active child conversations must finish before another turn or provider switch."
                : `${activeName()} cannot be steered in this state. Interrupt it or wait for the turn to finish.`}
          </p>
        </Show>
        <div class="composer-actions">
          <Show when={props.conversation.capabilities.canRoute}>
            <label class="composer-provider">
              <span class="pt-visually-hidden">Provider</span>
              <select
                ref={providerSelect}
                value={choice()}
                disabled={submitting() || descendantOnlyActive() || interruptionPending()}
                onChange={(event) => selectProvider(event.currentTarget.value as ProviderChoice)}
              >
                <option value="auto">Auto · {profileNames[props.routingProfile]}</option>
                <For each={props.providers}>
                  {(provider) => (
                    <option value={provider.id} disabled={!provider.available}>
                      {providerNames[provider.id]}{provider.available ? "" : ` — ${provider.diagnostic ?? "Unavailable"}`}
                    </option>
                  )}
                </For>
              </select>
            </label>
          </Show>
          <label class="composer-thinking">
            <span>{active() ? "Thinking · Next turn" : "Thinking"}</span>
            <select
              aria-label="Thinking"
              aria-describedby={helpId}
              value={thinkingValue()}
              disabled={submitting() || thinkingSave()?.pending}
              onChange={(event) => {
                setError(null);
                const value = event.currentTarget.value;
                const preference: ThinkingPreference = value.startsWith("manual:")
                  ? { kind: "manual", level: value.slice(7) }
                  : { kind: value as "auto" | "providerDefault" };
                void store.setThinkingPreference(conversationId, preference);
              }}
            >
              <option value="auto">Auto</option>
              <option value="providerDefault">Provider default</option>
              <For each={effortChoices()}>{(level) => <option value={`manual:${level}`}>{effortName(level)}</option>}</For>
              <Show when={savedLevel() && !effortChoices().includes(savedLevel()!)}>
                <option value={`manual:${savedLevel()}`}>{effortName(savedLevel()!)} (saved)</option>
              </Show>
            </select>
          </label>
          <label class="composer-context-budget">
            <span>{active() ? "Context budget · Next turn" : "Context budget"}</span>
            <select
              aria-label="Context budget"
              aria-describedby={helpId}
              value={contextBudget().kind === "tokens" ? String((contextBudget() as { tokens: number }).tokens) : "providerDefault"}
              disabled={submitting() || contextBudgetSave()?.pending}
              onChange={(event) => {
                setError(null);
                const value = event.currentTarget.value;
                void store.setContextBudget(conversationId, value === "providerDefault" ? { kind: "providerDefault" } : { kind: "tokens", tokens: Number(value) });
              }}
            >
              <For each={CONTEXT_BUDGETS}>{(tokens) => <option value={String(tokens)}>{tokens / 1000}k</option>}</For>
              <option value="providerDefault">Provider default</option>
            </select>
          </label>
          <IconButton label="Composer help" aria-expanded={helpOpen() ? "true" : "false"} aria-controls={helpId} onClick={() => setHelpOpen((open) => !open)}>
            <HelpIcon />
          </IconButton>
          <Button
            aria-label={preview() ? "Edit Markdown" : "Preview Markdown"}
            title={preview() ? "Edit Markdown" : "Preview Markdown"}
            disabled={submitting()}
            onClick={() => {
              if (preview()) restoreDraftFocus = true;
              setPreview(!preview());
            }}
          >
            {preview() ? "Edit" : "Preview"}
          </Button>
          <Show when={props.conversation.capabilities.canInterrupt && rootTurnActive() && !interruptionPending()}>
            <Button
              disabled={!canInterrupt()}
              onClick={() => {
                const { currentRunId, provider } = props.conversation;
                if (canInterrupt() && currentRunId && provider) setPendingInterruption({ choice: "interrupt", provider, runId: currentRunId });
              }}
            >
              Interrupt {activeName()}
            </Button>
          </Show>
          <Button variant="primary" disabled={!canSubmit()} onClick={() => void submitDraft()}>
            {submitting() ? "Working…" : canSteer() ? `Steer ${activeName()}` : retrying() ? "Retry send" : "Send"}
          </Button>
          <Show when={props.conversation.thinkingDecision}>
            {(decision) => (
              <span class="thinking-decision" title={decision().reason}>
                {decision().preference.kind === "auto" ? "Auto → " : "Requested "}
                {decision().requestedEffort ? effortName(decision().requestedEffort!) : "Provider default"}
                {props.conversation.thinkingConfiguration
                  ? props.conversation.thinkingConfiguration.configuredEffort !== null
                    ? ` · configured ${effortName(props.conversation.thinkingConfiguration.configuredEffort)}`
                    : " · configured effort not set"
                  : " · configuration unreported"}
              </span>
            )}
          </Show>
        </div>
        <Show when={unreportedChoice()}>
          <p class="composer-explanation">{effortName(savedLevel()!)} is not reported as supported for this provider/model. The saved choice is retained and dispatch will validate it.</p>
        </Show>
        <div id={helpId} class="composer-help-content" hidden={!helpOpen()}>
          <p class="route-note">{choice() === "auto" ? "Prompting Time will explain the selected route." : `Pinned to ${providerNames[choice() as ProviderId]}.`}</p>
          <p class="route-note">Auto chooses effort locally for each new turn. Provider default uses the native CLI configuration without an app override. Manual levels depend on the provider and model; dispatch validates support.</p>
          <p class="route-note">Context budget requests native automatic compaction for the next new turn; 1k is 1,000 tokens. Providers may compact earlier for model capacity or response headroom. Provider default removes the app override.</p>
          <Show when={choice() === "auto" && configuration()}>
            <p class="route-note">Choices use the latest {props.conversation.provider ? providerNames[props.conversation.provider] : "provider"} model report ({configuration()!.model}); automatic routing revalidates them at dispatch.</p>
          </Show>
          <Show when={active()}><p class="route-note">Thinking changes apply to the next turn. Steering keeps the active turn's frozen decision.</p></Show>
          <Show when={props.conversation.thinkingDecision}>
            <p class="route-note">Latest run: {props.conversation.thinkingDecision!.reason} Configured effort reports a provider setting, not a thinking-token count.</p>
          </Show>
          <Show when={retrying()}><p class="route-note">Retry sends the original command with its frozen Thinking and context budget choices. Edit the message or provider to start a new request.</p></Show>
          <Show when={!active() || canSteer() || retrying()}>
            <p class="composer-shortcut">Enter to {canSteer() ? "steer" : "send"} · Shift + Enter for newline · ⌘ / Ctrl + Enter also {canSteer() ? "steers" : "sends"}</p>
          </Show>
        </div>
      </section>
      <ConfirmDialog
        open={dialogOpen()}
        busy={submitting()}
        confirmDisabled={!canInterrupt()}
        title={shownInterruption() ? interruptionTitle(shownInterruption()!) : ""}
        cancelLabel={`Keep ${shownInterruption() ? providerNames[shownInterruption()!.provider] : ""} running`}
        confirmLabel={shownInterruption() ? interruptionConfirm(shownInterruption()!) : ""}
        onConfirm={() => void confirmSwitch()}
        onCancel={closeInterruptionDialog}
      >
        <p>{shownInterruption() ? interruptionText(shownInterruption()!) : ""}</p>
        <Show when={error() && pendingInterruption()}>
          <p role="alert" class="inline-error">{error()}</p>
        </Show>
      </ConfirmDialog>
    </Show>
  );
}

function messageFor(reason: unknown) {
  return reason instanceof Error ? reason.message : "Prompting Time could not submit this request.";
}

function effortName(level: string) {
  return level.charAt(0).toUpperCase() + level.slice(1);
}
