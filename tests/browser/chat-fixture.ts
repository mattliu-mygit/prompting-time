import type { AppEvent, ApprovalSnapshot, TimelineItem, TimelinePage } from "../../src/bridge/types";

// Synthetic browser data only: no native bridge, files, provider calls, or timers.
const encoder = new TextEncoder();
const scenario = new URLSearchParams(location.search).get("chat");
let revision = 0;
let listener: ((event: AppEvent) => void) | null = null;
let diagnosticsReads = 0;

export const chatScenario = scenario !== null;

export function listenToChatEvents(handler: (event: AppEvent) => void) {
  listener = handler;
  return () => { if (listener === handler) listener = null; };
}

// Invoke explicitly from Playwright while reading older text to test scroll intent.
export function advanceChatStream() {
  revision += 1;
  listener?.({ kind: "conversationChanged", sequence: String(revision), conversationId: "conversation-0" });
  return revision;
}

export function chatFixtureStats() { return { revision, diagnosticsReads }; }

function item(conversationId: string, sequence: number, content: string, extra: Partial<TimelineItem> = {}): TimelineItem {
  const owner = conversationId.replace("conversation-", "");
  return {
    id: `${conversationId}-chat-${sequence}`, conversationId, runId: `run-${owner}`, agentId: `root-${owner}`,
    sequence: String(sequence), kind: "message", role: "assistant", provider: "codex",
    presentation: "normal", content, contentBytes: String(encoder.encode(content).length), truncated: false,
    ...extra,
  };
}

const response = [
  "## The parser is ready for review",
  "The update keeps **validation at the boundary** and leaves the storage contract unchanged.",
  "- Reject empty identifiers before opening a transaction.\n- Preserve Unicode in names.\n- Return a useful error when a row is missing.",
  "### Implementation",
  "```rust\nfn validate_name(name: &str) -> Result<&str, &'static str> {\n    let name = name.trim();\n    if name.is_empty() {\n        return Err(\"Name must not be empty\");\n    }\n    Ok(name)\n}\n```",
  "| Check | Result |\n| --- | --- |\n| Empty name | Rejected |\n| Unicode name | Preserved |\n| Existing record | Unchanged |",
  "> This is invented verification content, not a report about actual code changes.",
  "The next step is to review the error wording. The implementation does not create a new provider session or retry a write.",
].join("\n\n");

export function chatApproval(conversationId: string): ApprovalSnapshot {
  const owner = conversationId.replace("conversation-", "");
  return {
    id: `${conversationId}-approval`, runId: `run-${owner}`, agentId: `child-${owner}`, provider: "codex",
    agentPath: ["Root agent", `Synthetic child ${owner}`], agentPathTruncated: false,
    operation: "Run synthetic test command", scope: "One synthetic operation", status: "pending", responsePending: false,
  };
}

export function chatTimeline(conversationId: string, cursor: string | null = null, limit = 80): TimelinePage {
  if (scenario === "orientation") {
    const count = 240 + (conversationId === "conversation-0" ? revision : 0);
    const end = cursor === null ? count : Number(cursor);
    const start = Math.max(0, end - limit);
    return {
      items: Array.from({ length: end - start }, (_, offset) => {
        const sequence = start + offset + 1;
        return item(conversationId, sequence, `### Observation ${sequence}\n\n${"Invented history for reading-position checks. ".repeat(8)}`);
      }),
      nextCursor: start === 0 ? null : String(start),
      approvals: [], approvalsTruncated: false, approvalsNextCursor: null,
    };
  }
  const items = [
    item(conversationId, 1, "Please simplify the parser, check Unicode handling, and show the important changes.", { role: "user" }),
    item(conversationId, 2, "I’ll inspect the parser and its tests, then make the smallest change that covers the missing validation."),
    item(conversationId, 3, "Inspecting the parser and its callers", { kind: "progress", role: null }),
    item(conversationId, 4, "synthetic-parser.rs\nsynthetic-parser-test.rs", { kind: "tool", role: null }),
    item(conversationId, 5, "Synthetic test output: 12 passed, 0 failed", { kind: "tool", role: null }),
    item(conversationId, 6, response),
  ];
  if (scenario === "stream") {
    for (let index = 0; index < 12; index += 1) {
      items.push(item(conversationId, index + 7, `### Earlier observation ${index + 1}\n\nThis is synthetic history for checking that incoming output does not interrupt reading.\n\n${"A bounded reply remains available while you scroll. ".repeat(8)}`));
    }
    items.push(item(conversationId, 19, `## Live response\n\n${"A new synthetic paragraph arrived.\n\n".repeat(revision + 1)}`));
  }
  if (scenario === "failure") {
    items.push(item(conversationId, 20, "The synthetic test process exited before producing a result. Review the output before continuing.", { kind: "diagnostic", role: null, presentation: "failure" }));
  }
  return {
    items, nextCursor: null,
    approvals: scenario === "approval" ? [chatApproval(conversationId)] : [],
    approvalsTruncated: false, approvalsNextCursor: null,
  };
}

export function chatDiagnostics(conversationId: string, cursor: string | null, limit: number) {
  diagnosticsReads += 1;
  const end = cursor === null ? 150 : Number(cursor);
  const start = Math.max(0, end - limit);
  return {
    items: Array.from({ length: end - start }, (_, offset) => item(conversationId, 1000 + start + offset,
      offset % 2 === 0 ? "thread/tokenUsage/updated" : "hook/started",
      { kind: "diagnostic", role: null, presentation: "telemetry" })),
    nextCursor: start === 0 ? null : String(start),
  };
}
