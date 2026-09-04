import { useEffect, useMemo, useRef, useState } from "react";
import type {
  ApprovalDetailSnapshot,
  ApprovalQuestionPreview,
  ApprovalResponse,
  ApprovalSnapshot,
  UserInputQuestion,
} from "../../bridge/types";
import type { ConversationActions } from "../../app/store";

const QUESTION_PAGE_SIZE = 20;

type ApprovalCardProps = {
  approval: ApprovalSnapshot;
  agentPath?: string;
  actions: ConversationActions;
  onReconcile(detail?: ApprovalDetailSnapshot): Pick<ApprovalDetailSnapshot, "status" | "responsePending"> | null | void | Promise<Pick<ApprovalDetailSnapshot, "status" | "responsePending"> | null | void>;
};

const providerNames = { codex: "Codex", claude: "Claude" } as const;

export function ApprovalCard({ approval, agentPath = "Agent", actions, onReconcile }: ApprovalCardProps) {
  const [detail, setDetail] = useState<ApprovalDetailSnapshot | null>(null);
  const [questions, setQuestions] = useState<ApprovalQuestionPreview[]>([]);
  const [questionCursor, setQuestionCursor] = useState<string | null>(null);
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [owned, setOwned] = useState(approval.responsePending);
  const [status, setStatus] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [disclosed, setDisclosed] = useState(false);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [loadingQuestions, setLoadingQuestions] = useState(false);
  const statusRef = useRef<HTMLParagraphElement>(null);
  const retryRef = useRef<HTMLButtonElement>(null);
  const loadGeneration = useRef(0);
  const questionGeneration = useRef(0);
  const questionCursorRef = useRef<string | null>(null);
  const questionLoadInFlight = useRef(false);

  useEffect(() => {
    loadGeneration.current += 1;
    questionGeneration.current += 1;
    questionCursorRef.current = null;
    questionLoadInFlight.current = false;
    setOwned(approval.responsePending || approval.status !== "pending");
    setStatus(approval.status === "pending" ? null : `Approval ${approval.status}.`);
    setLoadError(null);
    setDetail(null);
    setQuestions([]);
    setQuestionCursor(null);
    setAnswers({});
    setDisclosed(false);
    setLoadingDetail(false);
    setLoadingQuestions(false);
  }, [approval.id, approval.responsePending, approval.status]);

  useEffect(() => {
    if (loadError) retryRef.current?.focus();
  }, [loadError]);

  async function loadRequestDetails() {
    const generation = ++loadGeneration.current;
    setDisclosed(true);
    setLoadingDetail(true);
    setLoadError(null);
    try {
      await actions.loadApprovalDetail({ approvalId: approval.id })
      .then(async (next) => {
        if (generation !== loadGeneration.current) return;
        setDetail(next);
        if (next.status !== "pending") {
          setOwned(true);
          setStatus(`Approval ${next.status}.`);
          await onReconcile(next);
          return;
        } else if (next.responsePending) {
          setOwned(true);
          setStatus("The response is recorded and still being delivered.");
        }
        if (next.questionCount > 0) {
          const page = await actions.loadApprovalQuestions({
            approvalId: approval.id,
            cursor: null,
            limit: QUESTION_PAGE_SIZE,
          });
          if (generation === loadGeneration.current) {
            setQuestions(page.items);
            setQuestionCursor(page.nextCursor);
            questionCursorRef.current = page.nextCursor;
          }
        }
      });
    } catch (reason) {
      if (generation === loadGeneration.current) {
        setLoadError(messageFor(reason));
      }
    } finally {
      if (generation === loadGeneration.current) setLoadingDetail(false);
    }
  }

  async function respond(response: ApprovalResponse) {
    if (owned) return;
    setOwned(true);
    setStatus("Sending response…");
    let responseError: string | null = null;
    try {
      await actions.respondToApproval({ approvalId: approval.id, response });
      setStatus("Response recorded.");
    } catch (reason) {
      responseError = messageFor(reason);
      setStatus(responseError);
    } finally {
      try {
        const durable = await onReconcile();
        if (durable && durable.status === "pending") {
          setOwned(durable.responsePending);
          if (durable.responsePending) {
            setStatus("The response is recorded and still being delivered.");
          } else if (responseError) {
            setStatus(responseError);
          }
        } else {
          setOwned(true);
        }
      } catch {
        setOwned(true);
        setStatus("The response result could not be reconciled. Reload the conversation before trying again.");
      } finally {
        queueMicrotask(() => statusRef.current?.focus());
      }
    }
  }

  async function loadMoreQuestions() {
    if (!questionCursor || questionLoadInFlight.current) return;
    const requestedCursor = questionCursor;
    const generation = questionGeneration.current;
    questionLoadInFlight.current = true;
    setLoadingQuestions(true);
    try {
      const page = await actions.loadApprovalQuestions({
        approvalId: approval.id,
        cursor: requestedCursor,
        limit: QUESTION_PAGE_SIZE,
      });
      if (generation !== questionGeneration.current || questionCursorRef.current !== requestedCursor) return;
      setQuestions((current) => {
        const byId = new Map(current.map((question) => [question.id, question]));
        page.items.forEach((question) => byId.set(question.id, question));
        return [...byId.values()];
      });
      setQuestionCursor(page.nextCursor);
      questionCursorRef.current = page.nextCursor;
    } catch (reason) {
      if (generation === questionGeneration.current) setLoadError(messageFor(reason));
    } finally {
      if (generation === questionGeneration.current) {
        questionLoadInFlight.current = false;
        setLoadingQuestions(false);
      }
    }
  }

  const exactQuestions = useMemo(() => {
    if (!detail?.input || detail.truncated || detail.input.questions.length !== detail.questionCount) {
      return new Map<string, UserInputQuestion>();
    }
    return new Map(detail.input.questions.map((question) => [question.id, question]));
  }, [detail]);
  const displayQuestions = questions.map((preview) => {
    const exact = exactQuestions.get(preview.id);
    return exact ? { ...exact, truncated: false } : preview;
  });
  const isQuestion = (detail?.questionCount ?? 0) > 0;
  const allAnswered = displayQuestions.length === detail?.questionCount
    && displayQuestions.every((question) => validQuestionAnswer(
      question,
      answers[question.id] ?? "",
      exactQuestions.has(question.id) || !question.truncated,
    ));
  const summaryAgentPath = approval.agentPath?.length
    ? `${approval.agentPathTruncated ? "…/" : ""}${approval.agentPath.join("/")}`
    : agentPath;
  const requestingAgentPath = detail?.agentPath?.length
    ? `${detail.agentPathTruncated ? "…/" : ""}${detail.agentPath.join("/")}`
    : summaryAgentPath;

  return (
    <article className="approval-card" data-approval-id={approval.id} aria-labelledby={`approval-${approval.id}`}>
      <header>
        <div>
          <span className="provider-badge">{providerNames[approval.provider]}</span>
          <h4 id={`approval-${approval.id}`}>{approval.operation}</h4>
        </div>
        <span className="approval-scope">{approval.scope}</span>
      </header>
      <dl className="approval-meta">
        <div><dt>Requesting agent</dt><dd>{requestingAgentPath}</dd></div>
        <div><dt>Scope</dt><dd>{approval.scope}</dd></div>
      </dl>
      {!disclosed ? (
        <button type="button" className="secondary-button" onClick={() => void loadRequestDetails()}>
          Review {approval.operation}
        </button>
      ) : null}
      {loadingDetail ? <p role="status">Loading request details…</p> : null}
      {detail ? <ApprovalOperation detail={detail} /> : null}
      {detail?.truncated ? <p className="truncation-note">Request detail is truncated to its safe display bound.</p> : null}
      {displayQuestions.map((question) => (
        <QuestionField
          key={question.id}
          question={question}
          value={answers[question.id] ?? ""}
          disabled={owned}
          choicesComplete={exactQuestions.has(question.id) || !question.truncated}
          onChange={(value) => setAnswers((current) => ({ ...current, [question.id]: value }))}
        />
      ))}
      {questionCursor ? (
        <button type="button" className="secondary-button" disabled={owned || loadingQuestions} onClick={() => void loadMoreQuestions()}>
          {loadingQuestions ? "Loading questions…" : "Load more questions"}
        </button>
      ) : null}
      {loadError ? (
        <div className="inline-error">
          <p role="alert">{loadError}</p>
          <button ref={retryRef} type="button" className="secondary-button" onClick={() => void loadRequestDetails()}>
            Retry request details
          </button>
        </div>
      ) : null}
      {detail ? <div className="approval-actions">
        {isQuestion ? (
          <button
            type="button"
            className="primary-button"
            disabled={owned || !detail || !allAnswered}
            onClick={() => void respond({
              kind: "answers",
              value: Object.fromEntries(Object.entries(answers).map(([id, value]) => [id, [value]])),
            })}
          >
            Answer {approval.operation}
          </button>
        ) : (
          <>
            <button type="button" className="danger-button" disabled={owned || !detail} onClick={() => void respond({ kind: "denied" })}>
              Deny {approval.operation}
            </button>
            <button type="button" className="primary-button" disabled={owned || !detail} onClick={() => void respond({ kind: "approved" })}>
              Allow {approval.operation}
            </button>
          </>
        )}
      </div> : null}
      {status ? <p ref={statusRef} role="status" tabIndex={-1}>{status}</p> : null}
    </article>
  );
}

function ApprovalOperation({ detail }: { detail: ApprovalDetailSnapshot }) {
  const operation = detail.details;
  if (!operation) return null;
  switch (operation.kind) {
    case "commandExecution":
      return (
        <div className="operation-detail">
          {operation.command ? <code>{operation.command}</code> : <span>Command details unavailable</span>}
          {operation.cwd ? <small>Working directory: {operation.cwd}</small> : null}
        </div>
      );
    case "fileChange":
      return (
        <div className="operation-detail">
          {operation.reason ? <p>{operation.reason}</p> : null}
          {operation.grantRoot ? <small>Grant root: {operation.grantRoot}</small> : null}
          <ul>{operation.changes.map((change) => <li key={change.path}>{changeKind(change.change)}: {change.path}</li>)}</ul>
        </div>
      );
    case "permissionProfile":
      return (
        <div className="operation-detail">
          <small>Working directory: {operation.cwd}</small>
          <p>Network: {operation.profile.networkEnabled === true ? "allowed" : operation.profile.networkEnabled === false ? "denied" : "not specified"}</p>
          {operation.profile.read ? <p>Read: {operation.profile.read.join(", ")}</p> : null}
          {operation.profile.write ? <p>Write: {operation.profile.write.join(", ")}</p> : null}
          {operation.profile.globScanMaxDepth ? <p>Glob scan depth: {operation.profile.globScanMaxDepth}</p> : null}
          {operation.profile.entries ? (
            <ul>{operation.profile.entries.map((entry, index) => (
              <li key={`${entry.access}-${index}`}>{entry.access}: {permissionPath(entry.path)}</li>
            ))}</ul>
          ) : null}
        </div>
      );
  }
}

function QuestionField({
  question,
  value,
  disabled,
  choicesComplete,
  onChange,
}: {
  question: ApprovalQuestionPreview;
  value: string;
  disabled: boolean;
  choicesComplete: boolean;
  onChange(value: string): void;
}) {
  const answerable = choicesComplete
    && (question.options === null || question.options.length > 0 || question.isOther);
  const acceptsFreeText = answerable && (question.options === null || question.isOther);
  return (
    <fieldset className="approval-question" disabled={disabled}>
      <legend>{question.header}</legend>
      <p>{question.question}</p>
      {answerable ? question.options?.map((option) => (
        <label key={option.label}>
          <input type="radio" name={question.id} value={option.label} checked={value === option.label} onChange={() => onChange(option.label)} />
          <span><strong>{option.label}</strong> — {option.description}</span>
        </label>
      )) : null}
      {acceptsFreeText ? (
        <label>
          <span>{question.options === null ? "Answer" : "Other answer"}</span>
          <input type={question.isSecret ? "password" : "text"} value={question.options?.some(({ label }) => label === value) ? "" : value} onChange={(event) => onChange(event.target.value)} />
        </label>
      ) : null}
      {!answerable ? <small className="truncation-note">Exact choices are unavailable. Reload the conversation before answering.</small> : null}
      {question.truncated ? <small className="truncation-note">This question preview is truncated.</small> : null}
    </fieldset>
  );
}

function validQuestionAnswer(
  question: ApprovalQuestionPreview | UserInputQuestion,
  answer: string,
  choicesComplete: boolean,
) {
  const value = answer.trim();
  if (!choicesComplete || !value) return false;
  if (question.options?.some(({ label }) => label === answer)) return true;
  return question.options === null || question.isOther;
}

function changeKind(change: import("../../bridge/types").FileChangeKind) {
  if (change.kind !== "update") return change.kind;
  return change.movePath ? `move to ${change.movePath}` : "update";
}

function permissionPath(path: import("../../bridge/types").FileSystemPath) {
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
