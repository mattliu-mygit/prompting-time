import type { PluginAction, PluginActionRequest, PluginActionResult } from "../../src/bridge/types";

// `?agents=reviews`: the invented Whooper lists reviews in its status, and the actions its page and
// the failed-review screen run (show-review, draft, ask, submit, retry, and round) answer in browser
// memory the way Whooper's commands would. Every repo, person, and line of code is made up.

export const reviewActions: PluginAction[] = [
  { name: "show-review", title: "Show a review", description: "Prints one review as JSON. Reads files only.", confirm: false },
  { name: "draft", title: "Change a draft", description: "Includes or leaves out a comment, edits its text, or sets the review's event or message.", confirm: false },
  { name: "ask", title: "Ask about this", description: "Answers a question about a review with a run of the review's agent that reads the review's record and writes nothing else.", confirm: false },
  { name: "retry", title: "Retry a review", description: "Queues the pull request of a failed review for the next round.", confirm: false },
];

const at = (daysAgo: number, hours: number, minutes = 0) => {
  const date = new Date();
  date.setDate(date.getDate() - daysAgo);
  date.setHours(hours, minutes, 0, 0);
  return date.toISOString();
};

type Entry = {
  run_id: string; repo: string; pr: number; title: string | null; url: string | null; started_at: string; ended_at: string | null;
  outcome: "running" | "reviewed" | "failed"; verdict: string | null; skipped: boolean; findings: number;
  submitted: { event: "APPROVE" | "COMMENT"; at: string } | null; needs_you: boolean;
};
type Drafts = { event: "APPROVE" | "COMMENT"; message: string; findings: Record<string, { included: boolean; text?: string }> };
type Finding = { id: string; path: string; line: number; side: "RIGHT"; title: string; draft: string; kind: string };
type Ask = { question: string; finding: string | null; answer: string; agent: string; started_at: string; ended_at: string; transcript: string | null };

const entries: Entry[] = [
  { run_id: "fixture-run-104", repo: "acme/widgets", pr: 104, title: "stream uploads in chunks", url: null, started_at: at(0, 15, 30), ended_at: null, outcome: "running", verdict: null, skipped: false, findings: 0, submitted: null, needs_you: false },
  { run_id: "fixture-run-101", repo: "acme/widgets", pr: 101, title: "add a retry policy for uploads", url: null, started_at: at(0, 14, 0), ended_at: at(0, 14, 25), outcome: "reviewed", verdict: "approve_once_ci_passes", skipped: false, findings: 2, submitted: null, needs_you: true },
  { run_id: "fixture-run-103", repo: "acme/widgets", pr: 103, title: "cap the upload retry delay", url: null, started_at: at(0, 11, 0), ended_at: at(0, 11, 20), outcome: "reviewed", verdict: "look_first", skipped: false, findings: 1, submitted: null, needs_you: true },
  { run_id: "fixture-run-61", repo: "acme/gadgets", pr: 61, title: "drop the legacy upload path", url: null, started_at: at(1, 16, 0), ended_at: at(1, 16, 1), outcome: "failed", verdict: null, skipped: false, findings: 0, submitted: null, needs_you: true },
  { run_id: "fixture-run-88", repo: "acme/gadgets", pr: 88, title: "move the projection into the domain package", url: null, started_at: at(1, 10, 0), ended_at: at(1, 10, 30), outcome: "reviewed", verdict: "approve_once_ci_passes", skipped: false, findings: 1, submitted: { event: "APPROVE", at: at(1, 11, 5) }, needs_you: false },
  { run_id: "fixture-run-77", repo: "acme/gadgets", pr: 77, title: "add the tags package", url: null, started_at: at(2, 15, 0), ended_at: at(2, 15, 40), outcome: "reviewed", verdict: "look_first", skipped: false, findings: 1, submitted: { event: "COMMENT", at: at(2, 16, 10) }, needs_you: false },
  { run_id: "fixture-run-55", repo: "acme/widgets", pr: 55, title: "keep only shared types in the core package", url: null, started_at: at(3, 9, 0), ended_at: at(3, 9, 1), outcome: "failed", verdict: null, skipped: false, findings: 0, submitted: null, needs_you: true },
];

const patch = `diff --git a/src/upload/retry.rs b/src/upload/retry.rs
new file mode 100644
index 0000000..3f2a9c1
--- /dev/null
+++ b/src/upload/retry.rs
@@ -0,0 +1,24 @@
+use std::time::Duration;
+
+/// How an upload retries after a failed request.
+pub struct RetryPolicy {
+    pub attempts: u32,
+    pub delay: Duration,
+}
+
+impl RetryPolicy {
+    /// The policy uploads use unless the config sets one.
+    pub fn standard() -> Self {
+        Self { attempts: 5, delay: Duration::from_millis(250) }
+    }
+
+    /// The delay before attempt \`n\`, doubling each time.
+    pub fn delay_before(&self, n: u32) -> Duration {
+        self.delay * 2u32.pow(n.saturating_sub(1))
+    }
+
+    /// Whether a cancelled upload may try again.
+    pub fn retries_cancelled(&self) -> bool {
+        true
+    }
+}
diff --git a/src/upload/mod.rs b/src/upload/mod.rs
index 9897331..922830f 100644
--- a/src/upload/mod.rs
+++ b/src/upload/mod.rs
@@ -1,6 +1,8 @@
 mod client;
+mod retry;
 
 pub use client::UploadClient;
+pub use retry::RetryPolicy;
 
 /// The largest file one upload sends.
 pub const MAX_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;
`;

const nits: Finding[] = [
  { id: "1", path: "src/upload/retry.rs", line: 12, side: "RIGHT", title: "Name the attempt count", kind: "nit", draft: "**nit:** The attempt count reads clearer as a named constant, such as `STANDARD_ATTEMPTS`." },
  { id: "2", path: "src/upload/retry.rs", line: 21, side: "RIGHT", title: "Cancelled uploads retry", kind: "nit", draft: "**nit:** `retries_cancelled` always says yes; a doc line on why a cancelled upload retries would help." },
];
const question: Finding = { id: "1", path: "src/upload/retry.rs", line: 17, side: "RIGHT", title: "The delay overflows", kind: "question", draft: "**question:** `2u32.pow` overflows after 32 attempts. Should the delay stop doubling at a cap?" };

const findings: Record<string, { verdict: string; reason: string; findings: Finding[] }> = {
  "fixture-run-101": { verdict: "approve_once_ci_passes", reason: "The policy is small and tested; two nits.", findings: nits },
  "fixture-run-103": { verdict: "look_first", reason: "The delay can overflow for long retries.", findings: [question] },
  "fixture-run-88": { verdict: "approve_once_ci_passes", reason: "A move with no change in behavior.", findings: [nits[0]!] },
  "fixture-run-77": { verdict: "look_first", reason: "One question about the delay.", findings: [question] },
};
const drafts: Record<string, Drafts> = {
  "fixture-run-101": { event: "APPROVE", message: "", findings: { "1": { included: true }, "2": { included: true } } },
  "fixture-run-103": { event: "COMMENT", message: "", findings: {} },
  "fixture-run-88": { event: "APPROVE", message: "", findings: { "1": { included: true } } },
  "fixture-run-77": { event: "COMMENT", message: "", findings: { "1": { included: true, text: "**question:** Should the delay stop doubling at a cap, such as 30 seconds?" } } },
};

/** Why each failed run failed, as its failure.txt says; fixture-run-61 recorded no reason. */
const failures: Record<string, string> = {
  "fixture-run-55": "the review run exited 1 without findings; see /synthetic/state/whooper/runs/fixture-run-55/stderr.log and the transcript in /synthetic/state/whooper/runs/fixture-run-55",
};

/** The questions asked about each run, with their answers, as asks/<n>.json holds them. */
const asks: Record<string, Ask[]> = {
  "fixture-run-55": [{
    question: "Why did this review fail?",
    finding: null,
    answer: "The run's `stderr.log` ends with `Error: Agent connection lost: agent child exited: exit status: 0`, two seconds after the run started. Devin's agent stopped before it read the pull request, so nothing in the PR caused the failure, and a retry should work.",
    agent: "devin",
    started_at: at(3, 9, 30),
    ended_at: at(3, 9, 32),
    transcript: null,
  }],
};

/** The reviews the status lists, newest first. */
export function fixtureReviewEntries(): Entry[] {
  return entries.map((entry) => ({ ...entry }));
}

const result = (overrides: Partial<PluginActionResult>): PluginActionResult => ({ exitCode: 0, stdout: "", stderr: "", fingerprint: null, preview: null, ...overrides });
const refused = (stderr: string) => result({ exitCode: 1, stderr });

/** The request whooper-submit --print would save for the run's drafts. */
export function fixtureSubmitRequest(runId: string): string {
  const choice = drafts[runId]!;
  const comments = (findings[runId]?.findings ?? []).filter((finding) => choice.findings[finding.id]?.included).map((finding) => ({
    path: finding.path, line: finding.line, side: finding.side, body: `AI comment (vetted)\n\n${choice.findings[finding.id]?.text ?? finding.draft}`,
  }));
  const body = choice.event === "APPROVE" && !choice.message ? (comments.length > 0 ? "Pre-approving to unblock" : "") : choice.message;
  return JSON.stringify({ commit_id: "0123456789abcdef0123456789abcdef01234567", event: choice.event, body, comments }, null, 2);
}

/** Records a submit for real, as whooper-submit's submit.json would. */
export function markFixtureSubmitted(runId: string) {
  const entry = entries.find((item) => item.run_id === runId);
  if (entry) Object.assign(entry, { submitted: { event: drafts[runId]!.event, at: new Date().toISOString() }, needs_you: false });
}

/** Answers show-review, draft, ask, retry, and round; null for the other actions. */
export function answerReviewAction(sent: PluginActionRequest): PluginActionResult | null {
  const [runId = "", ...rest] = sent.args;
  const entry = entries.find((item) => item.run_id === runId);
  switch (sent.action) {
    case "show-review": {
      if (!entry) return refused(`whooper-show: no run ${runId}`);
      const found = findings[runId];
      return result({ stdout: JSON.stringify({
        version: 1, run_id: runId, run: { repo: entry.repo, pr: entry.pr }, title: entry.title, author: "fixture-author", outcome: entry.outcome,
        findings: found ? { status: "reviewed", skip_reason: "", approval: { verdict: found.verdict, reason: found.reason }, findings: found.findings } : null,
        drafts: drafts[runId] ?? null, diff: found ? patch : null,
        failure: failures[runId] ?? null, asks: asks[runId] ?? [], submitted: entry.submitted,
      }) });
    }
    case "draft": {
      const choice = drafts[runId];
      if (!entry || !choice) return refused(`whooper-draft: no drafts for run ${runId}`);
      if (entry.submitted) return refused(`whooper-draft: ${runId} was already submitted; see its submit.json`);
      const [op, finding, text] = rest;
      if (op === "include" || op === "leave-out") choice.findings[finding!] = { ...choice.findings[finding!], included: op === "include" };
      else if (op === "text") choice.findings[finding!] = { included: choice.findings[finding!]?.included ?? false, text: text! };
      else if (op === "approve" || op === "comment") choice.event = op === "approve" ? "APPROVE" : "COMMENT";
      else if (op === "message") choice.message = finding!;
      return result({ stdout: JSON.stringify(choice) });
    }
    case "ask": {
      if (!entry) return refused(`whooper-ask: no run ${runId} in /synthetic/state/whooper/runs`);
      const [question = "", , finding = null] = rest;
      const answer = entry.outcome === "failed"
        ? "Whooper's log for this run stops right after the agent started: it never read the pull request or wrote findings. That points at the agent, not the PR; retry it, and if the next run fails the same way, check that the agent's CLI still signs in."
        : "A cancelled upload is one the user stopped on purpose, so trying it again restarts work they meant to end. Returning false, or retrying only after network failures, keeps the policy to what it is for.";
      (asks[runId] ??= []).push({ question, finding, answer, agent: "devin", started_at: new Date().toISOString(), ended_at: new Date().toISOString(), transcript: null });
      return result({ stdout: `${answer}\n` });
    }
    case "retry":
      if (!entry) return refused(`whooper-retry: no run ${runId} in /synthetic/state/whooper/runs`);
      if (entry.outcome !== "failed") return refused(`whooper-retry: ${runId} did not fail`);
      return result({ stdout: `queued ${entry.repo}#${entry.pr} for the next round\n` });
    case "round":
      return result({ stdout: "polled: 2 review requests, 1 retry\nreviewing acme/widgets#55\nacme/widgets#55: approve once CI passes, 1 finding\nround done\n" });
    default:
      return null;
  }
}
