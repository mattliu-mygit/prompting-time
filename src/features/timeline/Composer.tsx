import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type {
  ConversationSummary,
  ProviderId,
  ProviderInstallation,
  RoutingProfile,
} from "../../bridge/types";
import type { ConversationActions } from "../../app/store";

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
  onMutation(): void | Promise<void>;
  onModalChange?(open: boolean): void;
};

const providerNames: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };
const profileNames: Record<RoutingProfile, string> = {
  balanced: "Balanced",
  bestFit: "Best fit",
  usageBalance: "Usage balance",
};

export function Composer({ conversation, providers, routingProfile, actions, onMutation, onModalChange }: ComposerProps) {
  const [text, setText] = useState("");
  const [choice, setChoice] = useState<ProviderChoice>("auto");
  const [pendingInterruption, setPendingInterruption] = useState<PendingInterruption | null>(null);
  const [interruptRequestedFor, setInterruptRequestedFor] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingCommand, setPendingCommand] = useState<{ id: string; text: string; provider: ProviderId | null } | null>(null);
  const providerSelect = useRef<HTMLSelectElement>(null);
  const messageField = useRef<HTMLTextAreaElement>(null);
  const restoreFocusRequested = useRef(false);

  const rootTurnActive = conversation.currentRunId !== null
    && (conversation.runStatus === "queued" || conversation.runStatus === "running" || conversation.runStatus === "waiting");
  const descendantOnlyActive = conversation.currentRunId !== null
    && !rootTurnActive
    && (conversation.rollupStatus === "active" || conversation.rollupStatus === "needsAttention");
  const active = rootTurnActive || descendantOnlyActive;
  const interruptionPending = conversation.currentRunId !== null
    && conversation.currentRunId === interruptRequestedFor;
  const currentProvider = providers.find(({ id }) => id === conversation.provider);
  const canSteer = conversation.runStatus === "running"
    && !interruptionPending
    && currentProvider?.capabilities.includes("steering") === true;
  const canInterrupt = currentProvider?.capabilities.includes("interruption") === true;
  const interruptionDialogOpen = pendingInterruption !== null;

  useEffect(() => {
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
    else messageField.current?.focus();
  }

  function selectProvider(next: ProviderChoice) {
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
    setPendingCommand(null);
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
      !pendingInterruption
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
      await actions.interruptRun({ runId: pendingInterruption.runId });
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

  async function send() {
    const trimmed = text.trim();
    if (!trimmed || submitting) return;
    const provider = choice === "auto" ? null : choice;
    const command = pendingCommand
      && pendingCommand.text === trimmed
      && pendingCommand.provider === provider
      ? pendingCommand
      : { id: newCommandId(), text: trimmed, provider };
    setPendingCommand(command);
    setSubmitting(true);
    setError(null);
    try {
      await actions.submitMessage({
        conversationId: conversation.id,
        text: command.text,
        providerOverride: command.provider,
        commandId: command.id,
      });
      setText("");
      setPendingCommand(null);
      await onMutation();
    } catch (reason) {
      if (!isAmbiguousTransportFailure(reason)) setPendingCommand(null);
      setError(messageFor(reason));
    } finally {
      setSubmitting(false);
    }
  }

  async function steer() {
    const trimmed = text.trim();
    if (!trimmed || !conversation.currentRunId || !canSteer || submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      await actions.steerRun({ runId: conversation.currentRunId, text: trimmed });
      setText("");
      await onMutation();
    } catch (reason) {
      setError(messageFor(reason));
    } finally {
      setSubmitting(false);
    }
  }

  const retrying = pendingCommand !== null && error !== null;
  const activeName = conversation.provider ? providerNames[conversation.provider] : "provider";

  return (
    <>
      <section className="composer" aria-label="Message composer" aria-busy={submitting} inert={interruptionDialogOpen}>
      <div className="composer-controls">
        <label>
          <span>Provider</span>
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
        </label>
        <span className="route-note">
          {choice === "auto" ? "Prompting Time will explain the selected route." : `Pinned to ${providerNames[choice]}.`}
        </span>
      </div>
      <label className="message-field">
        <span>Message</span>
        <textarea
          ref={messageField}
          value={text}
          rows={3}
          disabled={submitting}
          placeholder={active ? `Add direction for ${activeName}` : "Ask Prompting Time…"}
          onChange={(event) => {
            setText(event.target.value);
            if (pendingCommand && event.target.value.trim() !== pendingCommand.text) setPendingCommand(null);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void (canSteer ? steer() : active ? Promise.resolve() : send());
            }
          }}
        />
      </label>
      {error && !pendingInterruption ? <p role="alert" className="inline-error">{error}</p> : null}
      {active && !canSteer ? (
        <p className="composer-explanation">{interruptionPending
          ? `Waiting for ${activeName} to stop before another turn.`
          : descendantOnlyActive
          ? "Active child agents must finish before another turn or provider switch."
          : `${activeName} cannot be steered in this state. Interrupt it or wait for the turn to finish.`}</p>
      ) : null}
      <div className="composer-actions">
        {rootTurnActive && !canSteer && !interruptionPending ? (
          <button
            type="button"
            className="secondary-button"
            disabled={!canInterrupt}
            onClick={() => {
              if (conversation.currentRunId && conversation.provider) {
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
          disabled={submitting || !text.trim() || (active && !canSteer)}
          onClick={() => void (canSteer ? steer() : send())}
        >
          {submitting ? "Working…" : canSteer ? `Steer ${activeName}` : retrying ? "Retry send" : "Send"}
        </button>
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

function newCommandId() {
  return globalThis.crypto.randomUUID();
}

function isAmbiguousTransportFailure(reason: unknown) {
  return reason instanceof Error
    && "code" in reason
    && reason.code === "outcome-unknown";
}

function messageFor(reason: unknown) {
  return reason instanceof Error ? reason.message : "Prompting Time could not submit this request.";
}
