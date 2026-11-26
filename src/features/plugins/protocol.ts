/**
 * The messages between a plugin's page and Prompting Time, page protocol version 1, which a
 * manifest names as `page.protocol`. Whooper's page/src/protocol.ts is the format's reference: it
 * says which plugin action each request runs. Every message is a JSON object with `version` and
 * `type`; Prompting Time drops a message from the page that does not fit, and runs nothing for it.
 *
 * Page to host, each with an `id` that the answer carries back as `reply_to`: ready, open-review,
 * change-draft, submit, and ask. Host to page: status, review, drafts, submit-result, answer, and
 * error, as answers, or unasked as updates.
 */
export const PROTOCOL_VERSION = 1;

/** One change to a review's drafts, as the plugin's draft action takes it. */
export type DraftChange =
  | { op: "include" | "leave-out"; finding: string }
  | { op: "text"; finding: string; text: string }
  | { op: "approve" | "comment" }
  | { op: "message"; text: string };

export type PageRequest =
  | { type: "ready" }
  | { type: "open-review"; run_id: string }
  | { type: "change-draft"; run_id: string; change: DraftChange }
  | { type: "submit"; run_id: string }
  | { type: "ask"; run_id: string; question: string; finding: string | null };

export type PageMessage = PageRequest & { version: typeof PROTOCOL_VERSION; id: number };

/** A review as the page draws it. Prompting Time passes the plugin's findings and drafts through. */
export type PageReview = {
  run_id: string;
  repo: string;
  pr: number;
  title: string | null;
  author: string | null;
  findings: object;
  drafts: object;
  patch: string;
};

/** One review in the page's queue. */
export type QueueItem = {
  run_id: string | null;
  /** <owner>/<repo>#<number> */
  pr: string;
  title: string | null;
  state: "decide" | "reviewing" | "failed" | "submitted";
  verdict: string | null;
  findings: number;
};

export type HostUpdate =
  | { type: "status"; status: object; queue: QueueItem[] }
  | { type: "review"; review: PageReview }
  | { type: "drafts"; run_id: string; drafts: object }
  | { type: "submit-result"; run_id: string; posted: boolean; message: string }
  | { type: "answer"; run_id: string; finding: string | null; question: string; answer: string }
  | { type: "error"; message: string };

export type HostMessage = HostUpdate & { version: typeof PROTOCOL_VERSION; reply_to?: number };

type Field = "string" | "string|null" | "object";
type Shape = Record<string, Field>;

const REQUEST_FIELDS: Record<PageRequest["type"], Shape> = {
  ready: {},
  "open-review": { run_id: "string" },
  "change-draft": { run_id: "string", change: "object" },
  submit: { run_id: "string" },
  ask: { run_id: "string", question: "string", finding: "string|null" },
};

const CHANGE_FIELDS: Record<DraftChange["op"], Shape> = {
  include: { finding: "string" },
  "leave-out": { finding: "string" },
  text: { finding: "string", text: "string" },
  approve: {},
  comment: {},
  message: { text: "string" },
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function fits(value: Record<string, unknown>, shape: Shape): boolean {
  return Object.entries(shape).every(([key, field]) => {
    const item = value[key];
    if (field === "string|null") return item === null || typeof item === "string";
    if (field === "object") return isRecord(item);
    return typeof item === "string";
  });
}

/** The request in `data` when it fits protocol version 1, or null: the check before anything runs. */
export function readPageMessage(data: unknown): PageMessage | null {
  if (!isRecord(data) || data.version !== PROTOCOL_VERSION || typeof data.id !== "number" || typeof data.type !== "string") return null;
  const shape = Object.hasOwn(REQUEST_FIELDS, data.type) ? REQUEST_FIELDS[data.type as PageRequest["type"]] : undefined;
  if (!shape || !fits(data, shape)) return null;
  if (data.type === "change-draft") {
    const change = data.change as Record<string, unknown>;
    const fields = typeof change.op === "string" && Object.hasOwn(CHANGE_FIELDS, change.op) ? CHANGE_FIELDS[change.op as DraftChange["op"]] : undefined;
    if (!fields || !fits(change, fields)) return null;
  }
  return data as unknown as PageMessage;
}
