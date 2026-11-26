import type { ConversationSummary, ProviderId, RollupStatus, RunStatus } from "../../src/bridge/types";

/** `?work=1`: invented roots across three projects and every status, so All work shows "Needs you" and project groups. */
export const workScenario = new URLSearchParams(location.search).get("work") !== null;

const roots: Array<[title: string, projectRoot: string | null, provider: ProviderId, runStatus: RunStatus, rollupStatus: RollupStatus]> = [
  ["Evaluation rows shift after rescore", "/synthetic/projects/compiler", "codex", "waiting", "needsAttention"],
  ["Tag names for the release notes", "/synthetic/projects/docs-site", "claude", "running", "needsAttention"],
  ["Parser ownership cleanup", "/synthetic/projects/compiler", "claude", "running", "active"],
  ["Retry budget for uploads", "/synthetic/projects/compiler", "codex", "queued", "active"],
  ["Shader cache warmup", "/synthetic/projects/renderer", "codex", "completed", "completed"],
  ["Flaky snapshot test", "/synthetic/projects/renderer", "codex", "failed", "failed"],
  ["Font loading audit", "/synthetic/projects/docs-site", "codex", "interrupted", "interrupted"],
  ["Draft the migration guide", null, "claude", "completed", "completed"],
];

/** The scenario's roots, built from the fixture's usual root so every other field stays the same. */
export function workConversations(template: ConversationSummary): ConversationSummary[] {
  return roots.map(([title, projectRoot, provider, runStatus, rollupStatus], index) => ({
    ...template, id: `conversation-${index}`, title, projectRoot, provider, runStatus, rollupStatus, currentRunId: `run-${index}`,
  }));
}
