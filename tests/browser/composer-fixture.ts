import type { AppApi } from "../../src/app/store";
import type { TimelineItem } from "../../src/bridge/types";

// Browser-memory acknowledgments only. No native bridge or provider execution.
export const composerScenario = new URLSearchParams(location.search).get("composer");
const submissions: Parameters<AppApi["submitMessage"]>[0][] = [];
const steers: Parameters<AppApi["steerRun"]>[0][] = [];
const messages: TimelineItem[] = [];
const pendingSubmissions: Array<() => void> = [];

// Only the explicit pending fixture waits; release without any native execution.
export function completePendingSubmissions() {
  pendingSubmissions.splice(0).forEach((resolve) => resolve());
}

export function composerFixtureStats() {
  return { submissions: [...submissions], steers: [...steers] };
}

export function composerMessages(conversationId: string) {
  return messages.filter((item) => item.conversationId === conversationId);
}

function appendMessage(conversationId: string, text: string) {
  const owner = conversationId.replace("conversation-", "");
  messages.push({
    id: `composer-message-${messages.length}`, conversationId, runId: `run-${owner}`, agentId: `root-${owner}`,
    sequence: String(2000 + messages.length), kind: "message", role: "user", provider: "codex",
    presentation: "normal", content: text, contentBytes: String(new TextEncoder().encode(text).length), truncated: false,
  });
}

export const submitComposerMessage: AppApi["submitMessage"] = async (request) => {
  if (!composerScenario) throw new Error("Enable an explicit composer fixture to submit synthetic text.");
  submissions.push({ ...request });
  if (composerScenario === "failure") throw new Error("Synthetic send failure. Your draft was not accepted.");
  if (composerScenario === "pending") await new Promise<void>((resolve) => pendingSubmissions.push(resolve));
  appendMessage(request.conversationId, request.text);
  return { runId: "synthetic-accepted-run", provider: "codex", status: "completed", duplicate: false, routingExplanation: "Synthetic acceptance only" };
};

export const steerComposerRun: AppApi["steerRun"] = async (request) => {
  if (composerScenario !== "steer") throw new Error("Only the synthetic steer scenario accepts direction.");
  steers.push({ ...request });
  appendMessage(request.runId.replace("run-", "conversation-"), request.text);
};
