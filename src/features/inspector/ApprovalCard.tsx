import { createEffect, createMemo, createSignal, For, Match, Show, Switch, untrack } from "solid-js";
import { Button } from "@prompting-time/ui";
import type {
  ApprovalDetailSnapshot,
  ApprovalQuestionPreview,
  ApprovalResponse,
  ApprovalSnapshot,
  FileChangeKind,
  FileSystemPath,
  UserInputQuestion,
} from "../../bridge/types";
import type { ConversationActions } from "../../app/store";

const QUESTION_PAGE_SIZE = 20;
const providerNames = { codex: "Codex", claude: "Claude" } as const;

type Durable = Pick<ApprovalDetailSnapshot, "status" | "responsePending">;

type ApprovalCardProps = {
  approval: ApprovalSnapshot;
  agentPath?: string;
  actions: Pick<ConversationActions, "loadApprovalDetail" | "loadApprovalQuestions" | "respondToApproval">;
  /** Re-reads the request's durable state after a response or a terminal detail, and returns it. */
  onReconcile(detail?: ApprovalDetailSnapshot): Durable | null | void | Promise<Durable | null | void>;
};

/**
 * One pending permission or question request. It shows the operation and scope from the summary
 * and reads the exact request only when you ask to review it; questions page in 20 at a time. A
 * response is sent at most once, and the card then trusts only the request's durable state.
 */
export function ApprovalCard(props: ApprovalCardProps) {
  const [detail, setDetail] = createSignal<ApprovalDetailSnapshot | null>(null);
  const [questions, setQuestions] = createSignal<ApprovalQuestionPreview[]>([]);
  const [questionCursor, setQuestionCursor] = createSignal<string | null>(null);
  const [answers, setAnswers] = createSignal<Record<string, string>>({});
  const [owned, setOwned] = createSignal(untrack(() => props.approval.responsePending));
  const [status, setStatus] = createSignal<string | null>(null);
  const [loadError, setLoadError] = createSignal<string | null>(null);
  const [disclosed, setDisclosed] = createSignal(false);
  const [loadingDetail, setLoadingDetail] = createSignal(false);
  const [loadingQuestions, setLoadingQuestions] = createSignal(false);
  let statusElement: HTMLParagraphElement | undefined;
  let retryButton: HTMLButtonElement | undefined;
  let loadGeneration = 0;
  let questionGeneration = 0;
  let questionLoadInFlight = false;

  // A new durable state for this request starts the card over.
  createEffect(
    () => ({ id: props.approval.id, responsePending: props.approval.responsePending, status: props.approval.status }),
    ({ responsePending, status: durableStatus }) => {
      loadGeneration += 1;
      questionGeneration += 1;
      questionLoadInFlight = false;
      setOwned(responsePending || durableStatus !== "pending");
      setStatus(durableStatus === "pending" ? null : `Approval ${durableStatus}.`);
      setLoadError(null);
      setDetail(null);
      setQuestions([]);
      setQuestionCursor(null);
      setAnswers({});
      setDisclosed(false);
      setLoadingDetail(false);
      setLoadingQuestions(false);
    },
  );

  createEffect(loadError, (error) => {
    if (error) retryButton?.focus();
  });

  async function loadRequestDetails() {
    const generation = ++loadGeneration;
    const approvalId = props.approval.id;
    setDisclosed(true);
    setLoadingDetail(true);
    setLoadError(null);
    try {
      const next = await props.actions.loadApprovalDetail({ approvalId });
      if (generation !== loadGeneration) return;
      setDetail(next);
      if (next.status !== "pending") {
        setOwned(true);
        setStatus(`Approval ${next.status}.`);
        await props.onReconcile(next);
        return;
      } else if (next.responsePending) {
        setOwned(true);
        setStatus("The response is recorded and still being delivered.");
      }
      if (next.questionCount > 0) {
        const page = await props.actions.loadApprovalQuestions({ approvalId, cursor: null, limit: QUESTION_PAGE_SIZE });
        if (generation === loadGeneration) {
          setQuestions(page.items);
          setQuestionCursor(page.nextCursor);
        }
      }
    } catch (reason) {
      if (generation === loadGeneration) setLoadError(messageFor(reason));
    } finally {
      if (generation === loadGeneration) setLoadingDetail(false);
    }
  }

  async function respond(response: ApprovalResponse) {
    if (owned()) return;
    setOwned(true);
    setStatus("Sending response…");
    let responseError: string | null = null;
    try {
      await props.actions.respondToApproval({ approvalId: props.approval.id, response });
      setStatus("Response recorded.");
    } catch (reason) {
      responseError = messageFor(reason);
      setStatus(responseError);
    } finally {
      try {
        const durable = await props.onReconcile();
        if (durable && durable.status === "pending") {
          setOwned(durable.responsePending);
          if (durable.responsePending) setStatus("The response is recorded and still being delivered.");
          else if (responseError) setStatus(responseError);
        } else {
          setOwned(true);
        }
      } catch {
        setOwned(true);
        setStatus("The response result could not be reconciled. Reload the conversation before trying again.");
      } finally {
        queueMicrotask(() => statusElement?.focus());
      }
    }
  }

  async function loadMoreQuestions() {
    const requestedCursor = questionCursor();
    if (!requestedCursor || questionLoadInFlight) return;
    const generation = questionGeneration;
    questionLoadInFlight = true;
    setLoadingQuestions(true);
    try {
      const page = await props.actions.loadApprovalQuestions({ approvalId: props.approval.id, cursor: requestedCursor, limit: QUESTION_PAGE_SIZE });
      if (generation !== questionGeneration || questionCursor() !== requestedCursor) return;
      setQuestions((current) => {
        const byId = new Map(current.map((question) => [question.id, question]));
        page.items.forEach((question) => byId.set(question.id, question));
        return [...byId.values()];
      });
      setQuestionCursor(page.nextCursor);
    } catch (reason) {
      if (generation === questionGeneration) setLoadError(messageFor(reason));
    } finally {
      if (generation === questionGeneration) {
        questionLoadInFlight = false;
        setLoadingQuestions(false);
      }
    }
  }

  // Complete canonical questions from the detail, used when every question fits in it untruncated.
  const exactQuestions = createMemo(() => {
    const current = detail();
    if (!current?.input || current.truncated || current.input.questions.length !== current.questionCount) return new Map<string, UserInputQuestion>();
    return new Map(current.input.questions.map((question) => [question.id, question]));
  });
  const displayQuestions = () => questions().map((preview) => {
    const exact = exactQuestions().get(preview.id);
    return exact ? { ...exact, truncated: false } : preview;
  });
  const choicesComplete = (question: ApprovalQuestionPreview) => exactQuestions().has(question.id) || !question.truncated;
  const isQuestion = () => (detail()?.questionCount ?? 0) > 0;
  const allAnswered = () => displayQuestions().length === detail()?.questionCount
    && displayQuestions().every((question) => validQuestionAnswer(question, answers()[question.id] ?? "", choicesComplete(question)));
  const summaryAgentPath = () => (props.approval.agentPath?.length
    ? `${props.approval.agentPathTruncated ? "…/" : ""}${props.approval.agentPath.join("/")}`
    : props.agentPath ?? "Conversation");
  const requestingAgentPath = () => {
    const current = detail();
    return current?.agentPath?.length ? `${current.agentPathTruncated ? "…/" : ""}${current.agentPath.join("/")}` : summaryAgentPath();
  };
  const headingId = () => `approval-${props.approval.id}`;

  return (
    <article class="approval-card" data-approval-id={props.approval.id} aria-labelledby={headingId()}>
      <header>
        <span class="approval-provider">{providerNames[props.approval.provider]}</span>
        <h4 id={headingId()}>{props.approval.operation}</h4>
        <span class="approval-scope">{props.approval.scope}</span>
      </header>
      <dl class="approval-meta">
        <div><dt>Requesting conversation</dt><dd>{requestingAgentPath()}</dd></div>
        <div><dt>Scope</dt><dd>{props.approval.scope}</dd></div>
      </dl>
      <Show when={!disclosed()}>
        <Button onClick={() => void loadRequestDetails()}>Review {props.approval.operation}</Button>
      </Show>
      <Show when={loadingDetail()}><p role="status">Loading request details…</p></Show>
      <Show when={detail()}>{(current) => <ApprovalOperation detail={current()} />}</Show>
      <Show when={detail()?.truncated}><p class="truncation-note">Request detail is truncated to its safe display bound.</p></Show>
      <For each={displayQuestions()} keyed={(question) => question.id}>
        {(question) => (
          <QuestionField
            question={question()}
            value={answers()[question().id] ?? ""}
            disabled={owned()}
            choicesComplete={choicesComplete(question())}
            onChange={(value) => setAnswers((current) => ({ ...current, [question().id]: value }))}
          />
        )}
      </For>
      <Show when={questionCursor()}>
        <Button disabled={owned() || loadingQuestions()} onClick={() => void loadMoreQuestions()}>
          {loadingQuestions() ? "Loading questions…" : "Load more questions"}
        </Button>
      </Show>
      <Show when={loadError()}>
        <div class="inline-error">
          <p role="alert">{loadError()}</p>
          <Button ref={retryButton} onClick={() => void loadRequestDetails()}>Retry request details</Button>
        </div>
      </Show>
      <Show when={detail()}>
        <div class="approval-actions">
          <Show
            when={isQuestion()}
            fallback={
              <>
                <Button disabled={owned()} onClick={() => void respond({ kind: "denied" })}>Deny {props.approval.operation}</Button>
                <Button variant="primary" disabled={owned()} onClick={() => void respond({ kind: "approved" })}>Allow {props.approval.operation}</Button>
              </>
            }
          >
            <Button
              variant="primary"
              disabled={owned() || !allAnswered()}
              onClick={() => void respond({ kind: "answers", value: Object.fromEntries(Object.entries(answers()).map(([id, value]) => [id, [value]])) })}
            >
              Answer {props.approval.operation}
            </Button>
          </Show>
        </div>
      </Show>
      <Show when={status()}>
        <p ref={statusElement} role="status" tabindex={-1}>{status()}</p>
      </Show>
    </article>
  );
}

function ApprovalOperation(props: { detail: ApprovalDetailSnapshot }) {
  return (
    <Switch>
      <Match when={props.detail.details?.kind === "commandExecution" && props.detail.details}>
        {(operation) => {
          const command = () => operation() as Extract<NonNullable<ApprovalDetailSnapshot["details"]>, { kind: "commandExecution" }>;
          return (
            <div class="operation-detail">
              <Show when={command().command} fallback={<span>Command details unavailable</span>}><code>{command().command}</code></Show>
              <Show when={command().cwd}><small>Working directory: {command().cwd}</small></Show>
            </div>
          );
        }}
      </Match>
      <Match when={props.detail.details?.kind === "fileChange" && props.detail.details}>
        {(operation) => {
          const change = () => operation() as Extract<NonNullable<ApprovalDetailSnapshot["details"]>, { kind: "fileChange" }>;
          return (
            <div class="operation-detail">
              <Show when={change().reason}><p>{change().reason}</p></Show>
              <Show when={change().grantRoot}><small>Grant root: {change().grantRoot}</small></Show>
              <ul><For each={change().changes}>{(item) => <li>{changeKind(item.change)}: {item.path}</li>}</For></ul>
            </div>
          );
        }}
      </Match>
      <Match when={props.detail.details?.kind === "permissionProfile" && props.detail.details}>
        {(operation) => {
          const permission = () => operation() as Extract<NonNullable<ApprovalDetailSnapshot["details"]>, { kind: "permissionProfile" }>;
          const profile = () => permission().profile;
          return (
            <div class="operation-detail">
              <small>Working directory: {permission().cwd}</small>
              <p>Network: {profile().networkEnabled === true ? "allowed" : profile().networkEnabled === false ? "denied" : "not specified"}</p>
              <Show when={profile().read}><p>Read: {profile().read!.join(", ")}</p></Show>
              <Show when={profile().write}><p>Write: {profile().write!.join(", ")}</p></Show>
              <Show when={profile().globScanMaxDepth}><p>Glob scan depth: {profile().globScanMaxDepth}</p></Show>
              <Show when={profile().entries}>
                <ul><For each={profile().entries!}>{(entry) => <li>{entry.access}: {permissionPath(entry.path)}</li>}</For></ul>
              </Show>
            </div>
          );
        }}
      </Match>
    </Switch>
  );
}

function QuestionField(props: {
  question: ApprovalQuestionPreview;
  value: string;
  disabled: boolean;
  choicesComplete: boolean;
  onChange(value: string): void;
}) {
  const answerable = () => props.choicesComplete && (props.question.options === null || props.question.options.length > 0 || props.question.isOther);
  const acceptsFreeText = () => answerable() && (props.question.options === null || props.question.isOther);
  return (
    <fieldset class="approval-question" disabled={props.disabled}>
      <legend>{props.question.header}</legend>
      <p>{props.question.question}</p>
      <Show when={answerable()}>
        <For each={props.question.options ?? []}>
          {(option) => (
            <label>
              <input type="radio" name={props.question.id} value={option.label} checked={props.value === option.label} onChange={() => props.onChange(option.label)} />
              <span><strong>{option.label}</strong> — {option.description}</span>
            </label>
          )}
        </For>
      </Show>
      <Show when={acceptsFreeText()}>
        <label>
          <span>{props.question.options === null ? "Answer" : "Other answer"}</span>
          <input
            type={props.question.isSecret ? "password" : "text"}
            class="pt-input"
            value={props.question.options?.some(({ label }) => label === props.value) ? "" : props.value}
            onInput={(event) => props.onChange(event.currentTarget.value)}
          />
        </label>
      </Show>
      <Show when={!answerable()}><small class="truncation-note">Exact choices are unavailable. Reload the conversation before answering.</small></Show>
      <Show when={props.question.truncated}><small class="truncation-note">This question preview is truncated.</small></Show>
    </fieldset>
  );
}

function validQuestionAnswer(question: ApprovalQuestionPreview | UserInputQuestion, answer: string, choicesComplete: boolean) {
  const value = answer.trim();
  if (!choicesComplete || !value) return false;
  if (question.options?.some(({ label }) => label === answer)) return true;
  return question.options === null || question.isOther;
}

function changeKind(change: FileChangeKind) {
  if (change.kind !== "update") return change.kind;
  return change.movePath ? `move to ${change.movePath}` : "update";
}

function permissionPath(path: FileSystemPath) {
  switch (path.type) {
    case "path": return path.path;
    case "glob_pattern": return path.pattern;
    case "special": {
      const value = path.value;
      if (value.kind === "project_roots") return `project roots${value.subpath ? `/${value.subpath}` : ""}`;
      if (value.kind === "unknown") return `${value.path}${value.subpath ? `/${value.subpath}` : ""}`;
      return value.kind.replace("slash_tmp", "/tmp");
    }
  }
}

function messageFor(reason: unknown) {
  return reason instanceof Error ? reason.message : "Prompting Time could not respond to this request.";
}
