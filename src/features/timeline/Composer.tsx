import { useEffect, useId, useLayoutEffect, useRef, useState, useSyncExternalStore, type RefObject } from "react";
import { createPortal } from "react-dom";
import { CircleHelp } from "lucide-react";
import type {
  ConversationSummary,
  ProviderId,
  ProviderInstallation,
  RoutingProfile,
  ThinkingPreference,
} from "../../bridge/types";
import type { AppStore, ConversationActions } from "../../app/store";
import { MessageContent } from "./MessageContent";

type ProviderChoice = "auto" | ProviderId;
type PendingInterruption = {
  choice: ProviderId | "interrupt";
  provider: ProviderId;
  runId: string;
};

type ComposerProps = {
  conversation: ConversationSummary;
  providers: readonly ProviderInstallation[];
  routingProfile: RoutingProfile;
  actions: ConversationActions;
  store: AppStore;
  onMutation(): void | Promise<void>;
  onModalChange?(open: boolean): void;
  messageRef?: RefObject<HTMLTextAreaElement | null>;
  focusRequested?: boolean;
  onFocusHandled?(): void;
};

const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
const profileNames: Record<RoutingProfile, string> = {
  balanced: "Balanced",
  bestFit: "Best fit",
  usageBalance: "Usage balance",
};

export function Composer({ conversation, providers, routingProfile, actions, store, onMutation, onModalChange, messageRef, focusRequested, onFocusHandled }: ComposerProps) {
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const text = snapshot.draftsById[conversation.id]?.text ?? "";
  const submission = snapshot.submissionsById[conversation.id];
  const thinkingSave = snapshot.thinkingSavesById[conversation.id];
  const thinkingPreference = thinkingSave?.preference ?? conversation.thinkingPreference;
  const contextBudgetSave = snapshot.contextBudgetSavesById[conversation.id];
  const contextBudget = contextBudgetSave?.preference ?? conversation.contextBudget;
  const composerView = snapshot.composerViewsById[conversation.id];
  const preview = composerView?.preview ?? false;
  const setPreview = (preview: boolean) => store.setComposerView(conversation.id, { preview });
  const [helpOpen, setHelpOpen] = useState(false);
  const helpId = useId();
  const choice = composerView?.choice ?? submission?.providerOverride ?? "auto";
  const setChoice = (choice: ProviderChoice) => store.setComposerView(conversation.id, { choice });
  const [pendingInterruption, setPendingInterruption] = useState<PendingInterruption | null>(null);
  const [interruptRequestedFor, setInterruptRequestedFor] = useState<string | null>(null);
  const [localSubmitting, setSubmitting] = useState(false);
  const submitting = localSubmitting || submission?.pending === true;
  const [localError, setError] = useState<string | null>(null);
  const error = localError ?? contextBudgetSave?.error ?? thinkingSave?.error ?? submission?.error ?? null;
  const retrying = submission?.command != null && submission.error !== null;
  const providerSelect = useRef<HTMLSelectElement>(null);
  const localMessageField = useRef<HTMLTextAreaElement>(null);
  const messageField = messageRef ?? localMessageField;
  const restoreFocusRequested = useRef(false);
  const restoreDraftFocusRequested = useRef(false);

  const rootTurnActive = conversation.currentRunId !== null
    && (conversation.runStatus === "queued" || conversation.runStatus === "running" || conversation.runStatus === "waiting");
  const descendantOnlyActive = conversation.currentRunId !== null
    && !rootTurnActive
    && (conversation.rollupStatus === "active" || conversation.rollupStatus === "needsAttention");
  const active = rootTurnActive || descendantOnlyActive;
  const interruptionPending = conversation.currentRunId !== null
    && conversation.currentRunId === interruptRequestedFor;
  const currentProvider = providers.find(({ id }) => id === conversation.provider);
  const canSteer = conversation.capabilities.canSend && conversation.runStatus === "running"
    && !retrying
    && !interruptionPending
    && currentProvider?.capabilities.includes("steering") === true;
  const canInterrupt = conversation.capabilities.canInterrupt && currentProvider?.capabilities.includes("interruption") === true;
  const interruptionDialogOpen = pendingInterruption !== null;
  const canSubmit = conversation.capabilities.canSend && !submitting && !thinkingSave?.pending && !contextBudgetSave?.pending && !!text.trim() && (!active || canSteer || retrying) && !interruptionDialogOpen;
  const applicableConfiguration = choice === "auto" || choice === conversation.provider ? conversation.thinkingConfiguration : null;
  const effortChoices = [...new Set(["low", "medium", "high", ...(applicableConfiguration?.supportedEfforts ?? [])])];
  const savedLevel = thinkingPreference?.kind === "manual" ? thinkingPreference.level : null;
  const unreportedChoice = savedLevel !== null && (applicableConfiguration
    ? !applicableConfiguration.supportedEfforts.includes(savedLevel)
    : !effortChoices.includes(savedLevel));
  const thinkingValue = savedLevel === null ? thinkingPreference?.kind ?? "auto" : `manual:${savedLevel}`;

  useLayoutEffect(() => {
    if (!conversation.capabilities.canSend || !focusRequested || submitting || interruptionDialogOpen) return;
    if (preview) {
      setPreview(false);
      return;
    }
    messageField.current?.focus();
    onFocusHandled?.();
  }, [focusRequested, interruptionDialogOpen, messageField, onFocusHandled, preview, submitting]);

  useLayoutEffect(() => {
    const field = messageField.current;
    if (!field || preview) return;
    function resize() {
      if (!field) return;
      field.style.height = "auto";
      const style = window.getComputedStyle(field);
      const border = parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth);
      field.style.height = `${field.scrollHeight + border}px`;
    }
    resize();
    let width = field.clientWidth;
    let resizeFrame: number | null = null;
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => {
      if (field.clientWidth === width) return;
      width = field.clientWidth;
      if (resizeFrame !== null) return;
      resizeFrame = window.requestAnimationFrame(() => {
        resizeFrame = null;
        resize();
      });
    });
    observer?.observe(field);
    return () => {
      observer?.disconnect();
      if (resizeFrame !== null) window.cancelAnimationFrame(resizeFrame);
    };
  }, [messageField, preview, text]);

  useLayoutEffect(() => {
    if (!restoreDraftFocusRequested.current || preview || submitting || pendingInterruption) return;
    restoreDraftFocusRequested.current = false;
    messageField.current?.focus();
  }, [messageField, pendingInterruption, preview, submitting]);

  useLayoutEffect(() => {
    onModalChange?.(interruptionDialogOpen);
    return () => {
      if (interruptionDialogOpen) onModalChange?.(false);
    };
  }, [interruptionDialogOpen, onModalChange]);

  useEffect(() => {
    if (
      conversation.currentRunId !== interruptRequestedFor
      || !active
    ) {
      setInterruptRequestedFor(null);
    }
  }, [active, conversation.currentRunId, interruptRequestedFor]);

  useEffect(() => {
    if (
      !pendingInterruption
      || (rootTurnActive
        && conversation.currentRunId === pendingInterruption.runId
        && conversation.provider === pendingInterruption.provider)
    ) return;
    restoreFocusRequested.current = true;
    setPendingInterruption(null);
  }, [conversation.currentRunId, conversation.provider, pendingInterruption, rootTurnActive]);

  useEffect(() => {
    if (!restoreFocusRequested.current || submitting || pendingInterruption) return;
    const frame = window.requestAnimationFrame(() => {
      restoreFocusRequested.current = false;
      restoreComposerFocus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [pendingInterruption, submitting]);

  function restoreComposerFocus() {
    if (providerSelect.current && !providerSelect.current.disabled) providerSelect.current.focus();
    else if (preview) {
      restoreDraftFocusRequested.current = true;
      setPreview(false);
    }
    else messageField.current?.focus();
  }

  function selectProvider(next: ProviderChoice) {
    if (!conversation.capabilities.canSend || !conversation.capabilities.canRoute) return;
    if (
      active
      && conversation.currentRunId
      && conversation.provider
      && next !== "auto"
      && next !== conversation.provider
    ) {
      setPendingInterruption({
        choice: next,
        provider: conversation.provider,
        runId: conversation.currentRunId,
      });
      return;
    }
    setChoice(next);
    store.resetDraftCommand(conversation.id);
  }

  function closeInterruptionDialog() {
    restoreFocusRequested.current = true;
    setPendingInterruption(null);
  }

  function handleDialogKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape" && !submitting) {
      event.preventDefault();
      closeInterruptionDialog();
      return;
    }
    if (event.key !== "Tab") return;
    const controls = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
    const first = controls[0];
    const last = controls.at(-1);
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function confirmSwitch() {
    if (
      !conversation.capabilities.canSend
      || !pendingInterruption
      || !rootTurnActive
      || conversation.currentRunId !== pendingInterruption.runId
      || conversation.provider !== pendingInterruption.provider
      || submitting
      || !canInterrupt
    ) return;
    const nextChoice = pendingInterruption.choice === "interrupt"
      ? null
      : pendingInterruption.choice;
    if (nextChoice) {
      const target = providers.find(({ id }) => id === nextChoice);
      if (!target?.available) {
        setPendingInterruption(null);
        setError(`${providerNames[nextChoice]} is unavailable: ${target?.diagnostic ?? "Check the provider installation and sign in."}`);
        restoreFocusRequested.current = true;
        return;
      }
    }
    setSubmitting(true);
    setError(null);
    try {
      await actions.interruptRun({ conversationId: conversation.id, runId: pendingInterruption.runId });
      setInterruptRequestedFor(pendingInterruption.runId);
      if (nextChoice) setChoice(nextChoice);
      restoreFocusRequested.current = true;
      setPendingInterruption(null);
      await onMutation();
    } catch (reason) {
      setError(messageFor(reason));
    } finally {
      setSubmitting(false);
    }
  }

  async function submitDraft() {
    if (!canSubmit) return;
    const provider = choice === "auto" ? null : choice;
    setSubmitting(true);
    setError(null);
    try {
      if (!await store.submitDraft(conversation.id, provider, canSteer ? conversation.currentRunId : null)) return;
      setPreview(false);
      restoreDraftFocusRequested.current = true;
      await onMutation();
    } catch (reason) {
      setError(messageFor(reason));
    } finally {
      setSubmitting(false);
    }
  }

  const activeName = conversation.provider ? providerNames[conversation.provider] : "provider";

  if (!conversation.capabilities.canSend) return (
    <section className="composer composer-read-only" aria-label="Message composer">
      <p className="composer-explanation">{conversation.capabilities.unavailableReason ?? "This conversation is read-only."}<span> Captured activity may be incomplete.</span></p>
    </section>
  );

  return (
    <>
      <section className="composer" aria-label="Message composer" aria-busy={submitting} inert={interruptionDialogOpen}>
      <label className="message-field" hidden={preview}>
        <span className="sr-only">Message</span>
        <textarea
          ref={messageField}
          value={text}
          rows={2}
          disabled={submitting}
          placeholder={active ? `Add direction for ${activeName}` : "Ask Prompting Time…"}
          onChange={(event) => {
            store.setDraft(conversation.id, event.target.value);
          }}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || event.shiftKey || event.altKey || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
            event.preventDefault();
            if (!event.repeat) void submitDraft();
          }}
        />
      </label>
      {preview ? <section className="composer-preview" aria-label="Message preview">
        {text ? <MessageContent content={text} /> : <p className="empty-note">Nothing to preview.</p>}
      </section> : null}
      {error && !pendingInterruption ? <p role="alert" className="inline-error">{error}</p> : null}
      {active && !canSteer && !retrying ? (
        <p className="composer-explanation">{interruptionPending
          ? `Waiting for ${activeName} to stop before another turn.`
          : descendantOnlyActive
          ? "Active child conversations must finish before another turn or provider switch."
          : `${activeName} cannot be steered in this state. Interrupt it or wait for the turn to finish.`}</p>
      ) : null}
      <div className="composer-actions">
        {conversation.capabilities.canRoute ? <label className="composer-provider">
          <span className="sr-only">Provider</span>
          <select
            ref={providerSelect}
            value={choice}
            disabled={submitting || descendantOnlyActive || interruptionPending}
            onChange={(event) => selectProvider(event.target.value as ProviderChoice)}
          >
            <option value="auto">Auto · {profileNames[routingProfile]}</option>
            {providers.map((provider) => (
              <option key={provider.id} value={provider.id} disabled={!provider.available}>
                {providerNames[provider.id]}{provider.available ? "" : ` — ${provider.diagnostic ?? "Unavailable"}`}
              </option>
            ))}
          </select>
        </label> : null}
        <label className="composer-thinking">
          <span>{active ? "Thinking · Next turn" : "Thinking"}</span>
          <select aria-label="Thinking" aria-describedby={helpId} value={thinkingValue}
            disabled={submitting || thinkingSave?.pending}
            onChange={(event) => {
              setError(null);
              const value = event.target.value;
              const preference: ThinkingPreference = value.startsWith("manual:")
                ? { kind: "manual", level: value.slice(7) }
                : { kind: value as "auto" | "providerDefault" };
              void store.setThinkingPreference(conversation.id, preference);
            }}>
            <option value="auto">Auto</option>
            <option value="providerDefault">Provider default</option>
            {effortChoices.map(level => <option key={level} value={`manual:${level}`}>{effortName(level)}</option>)}
            {savedLevel && !effortChoices.includes(savedLevel) ? <option value={`manual:${savedLevel}`}>{effortName(savedLevel)} (saved)</option> : null}
          </select>
        </label>
        <label className="composer-context-budget">
          <span>{active ? "Context budget · Next turn" : "Context budget"}</span>
          <select aria-label="Context budget" aria-describedby={helpId}
            value={contextBudget.kind === "tokens" ? String(contextBudget.tokens) : "providerDefault"}
            disabled={submitting || contextBudgetSave?.pending}
            onChange={(event) => {
              setError(null);
              const value = event.target.value;
              void store.setContextBudget(conversation.id, value === "providerDefault"
                ? { kind: "providerDefault" }
                : { kind: "tokens", tokens: Number(value) });
            }}>
            {[200000, 300000, 400000, 500000].map(tokens => <option key={tokens} value={tokens}>{tokens / 1000}k</option>)}
            <option value="providerDefault">Provider default</option>
          </select>
        </label>
        <button
          type="button"
          className="composer-help"
          title="Composer help"
          aria-expanded={helpOpen}
          aria-controls={helpId}
          onClick={() => setHelpOpen(!helpOpen)}
        ><CircleHelp size={16} aria-hidden="true" /><span className="sr-only">Composer help</span></button>
        <button
          type="button"
          className="disclosure-link"
          aria-label={preview ? "Edit Markdown" : "Preview Markdown"}
          title={preview ? "Edit Markdown" : "Preview Markdown"}
          disabled={submitting}
          onClick={() => {
            if (preview) restoreDraftFocusRequested.current = true;
            setPreview(!preview);
          }}
        >{preview ? "Edit" : "Preview"}</button>
        {conversation.capabilities.canInterrupt && rootTurnActive && !interruptionPending ? (
          <button
            type="button"
            className="secondary-button"
            disabled={!canInterrupt}
            onClick={() => {
              if (canInterrupt && conversation.currentRunId && conversation.provider) {
                setPendingInterruption({
                  choice: "interrupt",
                  provider: conversation.provider,
                  runId: conversation.currentRunId,
                });
              }
            }}
          >
            Interrupt {activeName}
          </button>
        ) : null}
        <button
          type="button"
          className="primary-button"
          disabled={!canSubmit}
          onClick={() => void submitDraft()}
        >
          {submitting ? "Working…" : canSteer ? `Steer ${activeName}` : retrying ? "Retry send" : "Send"}
        </button>
        {conversation.thinkingDecision ? <span className="thinking-decision" title={conversation.thinkingDecision.reason}>
          {conversation.thinkingDecision.preference.kind === "auto" ? "Auto → " : "Requested "}
          {conversation.thinkingDecision.requestedEffort ? effortName(conversation.thinkingDecision.requestedEffort) : "Provider default"}
          {conversation.thinkingConfiguration
            ? conversation.thinkingConfiguration.configuredEffort !== null
              ? ` · configured ${effortName(conversation.thinkingConfiguration.configuredEffort)}`
              : " · configured effort not set"
            : " · configuration unreported"}
        </span> : null}
      </div>
      {unreportedChoice ? <p className="composer-explanation">{effortName(savedLevel!)} is not reported as supported for this provider/model. The saved choice is retained and dispatch will validate it.</p> : null}
      <div id={helpId} className="composer-help-content" hidden={!helpOpen}>
        <p className="route-note">{choice === "auto" ? "Prompting Time will explain the selected route." : `Pinned to ${providerNames[choice]}.`}</p>
        <p className="route-note">Auto chooses effort locally for each new turn. Provider default uses the native CLI configuration without an app override. Manual levels depend on the provider and model; dispatch validates support.</p>
        <p className="route-note">Context budget requests native automatic compaction for the next new turn; 1k is 1,000 tokens. Providers may compact earlier for model capacity or response headroom. Provider default removes the app override.</p>
        {choice === "auto" && applicableConfiguration ? <p className="route-note">Choices use the latest {conversation.provider ? providerNames[conversation.provider] : "provider"} model report ({applicableConfiguration.model}); automatic routing revalidates them at dispatch.</p> : null}
        {active ? <p className="route-note">Thinking changes apply to the next turn. Steering keeps the active turn's frozen decision.</p> : null}
        {conversation.thinkingDecision ? <p className="route-note">Latest run: {conversation.thinkingDecision.reason} Configured effort reports a provider setting, not a thinking-token count.</p> : null}
        {retrying ? <p className="route-note">Retry sends the original command with its frozen Thinking and context budget choices. Edit the message or provider to start a new request.</p> : null}
        {!active || canSteer || retrying ? <p className="composer-shortcut">Enter to {canSteer ? "steer" : "send"} · Shift + Enter for newline · ⌘ / Ctrl + Enter also {canSteer ? "steers" : "sends"}</p> : null}
      </div>
      </section>
      {pendingInterruption ? createPortal((
        <div className="dialog-backdrop">
          <div role="dialog" aria-modal="true" aria-labelledby="switch-dialog-title" className="confirm-dialog" onKeyDown={handleDialogKeyDown}>
            <h2 id="switch-dialog-title">{pendingInterruption.choice === "interrupt" ? `Interrupt ${providerNames[pendingInterruption.provider]}` : `Interrupt ${providerNames[pendingInterruption.provider]} to switch provider`}</h2>
            <p>{!canInterrupt
              ? `${providerNames[pendingInterruption.provider]} does not report interruption support. Wait for the active turn to finish before switching providers.`
              : pendingInterruption.choice === "interrupt"
              ? `Stop the active ${providerNames[pendingInterruption.provider]} run? Its completed activity will remain in the timeline.`
              : `Provider changes happen only between turns. Interrupt the active run before switching to ${providerNames[pendingInterruption.choice]}.`}</p>
            {error ? <p role="alert" className="inline-error">{error}</p> : null}
            <div className="dialog-actions">
              <button type="button" className="secondary-button" autoFocus={!canInterrupt} disabled={submitting} onClick={closeInterruptionDialog}>
                Keep {providerNames[pendingInterruption.provider]} running
              </button>
              <button type="button" className="danger-button" autoFocus={canInterrupt} disabled={submitting || !canInterrupt} onClick={() => void confirmSwitch()}>
                {!canInterrupt ? "Interruption unavailable" : pendingInterruption.choice === "interrupt" ? `Interrupt ${providerNames[pendingInterruption.provider]}` : `Interrupt and switch to ${providerNames[pendingInterruption.choice]}`}
              </button>
            </div>
          </div>
        </div>
      ), document.body) : null}
    </>
  );
}

function messageFor(reason: unknown) {
  return reason instanceof Error ? reason.message : "Prompting Time could not submit this request.";
}

function effortName(level: string) {
  return level.charAt(0).toUpperCase() + level.slice(1);
}
