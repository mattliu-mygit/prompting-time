import type { PluginAction, PluginActionRequest, PluginActionResult } from "../../src/bridge/types";

// `?agents=reviews`: the invented Whooper lists reviews in its status, as whooper-status --json does:
// in status format 2 (items in Whooper's groups, blockers from its causes) beside the reviews its page
// reads. The actions its page and its rows run (show-review, draft, ask, submit, retry, skip, remove,
// and round) answer in browser memory the way Whooper's commands would. Every repo, person, and line
// of code is made up.

export const reviewActions: PluginAction[] = [
  { name: "round", title: "Poll now", description: "Runs a round now: polls and reviews what it finds. Posts nothing.", confirm: false },
  { name: "show-review", title: "Show a review", description: "Prints one review as JSON. Reads files only.", confirm: false },
  { name: "draft", title: "Change a draft", description: "Includes or leaves out a comment, edits its text, or sets the review's event or message.", confirm: false },
  { name: "ask", title: "Ask about this", description: "Answers a question about a review with a run of the review's agent that reads the review's record and writes nothing else.", confirm: false },
  { name: "retry", title: "Retry a review", description: "Queues the pull request of a review for the next round, which reviews its head again.", confirm: false },
  { name: "skip", title: "Skip a review", description: "Marks a review done without submitting it; --undo takes that back.", confirm: false },
  { name: "remove", title: "Remove a review", description: "Takes a review out of the lists; its files stay, and --undo puts it back.", confirm: false },
];

const at = (daysAgo: number, hours: number, minutes = 0) => {
  const date = new Date();
  date.setDate(date.getDate() - daysAgo);
  date.setHours(hours, minutes, 0, 0);
  return date.toISOString();
};

type Cause = { id: string; summary: string; scope: "all" | "pr" };
type Entry = {
  run_id: string; repo: string; pr: number; title: string | null; url: string | null; started_at: string; ended_at: string | null;
  outcome: "running" | "reviewed" | "failed" | "waiting"; verdict: string | null; skipped: boolean; findings: number;
  submitted: { event: "APPROVE" | "COMMENT"; at: string } | null; needs_you: boolean;
  head_sha: string | null; pr_head_sha: string | null; cause: Cause | null; skipped_by_user: boolean;
  author: string | null; removed_by_user: boolean; safe_to_approve: boolean;
};

/** The cause that blocks every review in this fixture: Docker stopped, so each review's setup failed. */
export const dockerCause: Cause = { id: "docker", summary: "Docker isn't running", scope: "all" };
const agentExit: Cause = { id: "agent-exit", summary: "The review agent exited without findings", scope: "pr" };
const sha = (seed: number) => (0x1a2b3c4d5e + seed * 0x9e3779b1).toString(16).padStart(12, "0").slice(-12);
type Drafts = { event: "APPROVE" | "COMMENT"; message: string; findings: Record<string, { included: boolean; text?: string }> };
type Finding = { id: string; path: string; line: number; side: "RIGHT"; title: string; draft: string; kind: string };
type Ask = { question: string; finding: string | null; answer: string; agent: string; started_at: string; ended_at: string; transcript: string | null };

/** Invented GitHub logins, one per pull request number's last digit. */
const authors = ["ade-o", "lin-w", "rhea-k", "jon-b", "mika-s", "priya-r", "tomasz-p", "sam-t", "noor-a", "eli-v"];
/**
 * A run as the status lists it. Whooper calls a run safe to approve when it reviewed the whole
 * change, says approve once CI passes, and found only nits, as this fixture's approvals do unless
 * an entry says otherwise.
 */
const run = (entry: Omit<Entry, "head_sha" | "pr_head_sha" | "cause" | "skipped_by_user" | "url" | "skipped" | "author" | "removed_by_user" | "safe_to_approve"> & Partial<Entry>): Entry => ({
  url: null, skipped: false, head_sha: sha(entry.pr), pr_head_sha: sha(entry.pr), cause: null, skipped_by_user: false,
  author: authors[entry.pr % 10]!, removed_by_user: false,
  safe_to_approve: entry.outcome === "reviewed" && entry.verdict === "approve_once_ci_passes" && !entry.skipped, ...entry,
});
/** A finished review that waits on the user, `minutes` after 9:00 today. */
const reviewed = (pr: number, repo: string, title: string, verdict: string, findings: number, minutes: number) => run({
  run_id: `fixture-run-${pr}`, repo, pr, title, started_at: at(0, 9, minutes), ended_at: at(0, 9, minutes + 20), outcome: "reviewed", verdict, findings, submitted: null, needs_you: true,
});
const waiting = (pr: number, repo: string, title: string, minutes: number) => run({ run_id: `fixture-run-${pr}`, repo, pr, title, started_at: at(0, 14, minutes), ended_at: null, outcome: "waiting", verdict: null, findings: 0, submitted: null, needs_you: false, cause: dockerCause });

const entries: Entry[] = [
  run({ run_id: "fixture-run-104", repo: "acme/widgets", pr: 104, title: "stream uploads in chunks", started_at: at(0, 15, 30), ended_at: null, outcome: "running", verdict: null, findings: 0, submitted: null, needs_you: false }),
  // Review 2 of #101: the pull request moved on after it, so its header offers Rerun review.
  run({ run_id: "fixture-run-101", repo: "acme/widgets", pr: 101, title: "add a retry policy for uploads", started_at: at(0, 14, 0), ended_at: at(0, 14, 25), outcome: "reviewed", verdict: "approve_once_ci_passes", findings: 2, submitted: null, needs_you: true, pr_head_sha: sha(9101) }),
  run({ run_id: "fixture-run-103", repo: "acme/widgets", pr: 103, title: "cap the upload retry delay", started_at: at(0, 11, 0), ended_at: at(0, 11, 20), outcome: "reviewed", verdict: "look_first", findings: 1, submitted: null, needs_you: true }),
  // Enough reviews that Needs you scrolls on its own above the stack of folded sections.
  reviewed(105, "acme/widgets", "drop the unused metrics exporter from the upload client", "approve_once_ci_passes", 0, 30),
  // Approve once CI passes, but its one finding is a question, so Whooper calls it not safe to approve and it needs a look.
  run({ ...reviewed(106, "acme/widgets", "fix the timezone in exported traces", "approve_once_ci_passes", 1, 20), safe_to_approve: false }),
  reviewed(66, "acme/gadgets", "lazy-load the chart bundle", "approve_once_ci_passes", 0, 10),
  reviewed(67, "acme/gadgets", "rename the queue worker settings", "approve_once_ci_passes", 1, 5),
  reviewed(68, "acme/gadgets", "validate project names before creating buckets", "approve_once_ci_passes", 2, 0),
  reviewed(107, "acme/widgets", "move the span tree into its own package", "approve_once_ci_passes", 2, 0),
  reviewed(69, "acme/gadgets", "show token counts in the trace sidebar", "look_first", 2, 15),
  // Removed by the user: it leaves the lists, and search finds it, tagged removed.
  run({ ...reviewed(108, "acme/widgets", "keep focus in the date picker after Escape", "approve_once_ci_passes", 2, 40), needs_you: false, removed_by_user: true }),
  waiting(110, "acme/widgets", "split the upload client", 30),
  waiting(111, "acme/widgets", "log slow uploads", 31),
  waiting(62, "acme/gadgets", "index gadgets by owner", 30),
  waiting(63, "acme/gadgets", "retry webhook deliveries", 31),
  waiting(64, "acme/gadgets", "drop the unused metrics", 32),
  run({ run_id: "fixture-run-61", repo: "acme/gadgets", pr: 61, title: "drop the legacy upload path", started_at: at(1, 16, 0), ended_at: at(1, 16, 1), outcome: "failed", verdict: null, findings: 0, submitted: null, needs_you: true }),
  run({ run_id: "fixture-run-88", repo: "acme/gadgets", pr: 88, title: "move the projection into the domain package", started_at: at(1, 10, 0), ended_at: at(1, 10, 30), outcome: "reviewed", verdict: "approve_once_ci_passes", findings: 1, submitted: { event: "APPROVE", at: at(1, 11, 5) }, needs_you: false }),
  run({ run_id: "fixture-run-77", repo: "acme/gadgets", pr: 77, title: "add the tags package", started_at: at(2, 15, 0), ended_at: at(2, 15, 40), outcome: "reviewed", verdict: "look_first", findings: 1, submitted: { event: "COMMENT", at: at(2, 16, 10) }, needs_you: false }),
  run({ run_id: "fixture-run-55", repo: "acme/widgets", pr: 55, title: "keep only shared types in the core package", started_at: at(3, 9, 0), ended_at: at(3, 9, 1), outcome: "failed", verdict: null, findings: 0, submitted: null, needs_you: true, cause: agentExit }),
  // Review 1 of #101, before the retry policy changed: it opens read-only.
  run({ run_id: "fixture-run-101-1", repo: "acme/widgets", pr: 101, title: "add a retry policy for uploads", started_at: at(3, 13, 0), ended_at: at(3, 13, 20), outcome: "reviewed", verdict: "look_first", findings: 1, submitted: null, needs_you: false, head_sha: sha(8101) }),
];

/** The rounds whooper-status lists, newest first: today's polls, Docker's failures, and earlier days. */
export function fixtureRounds() {
  const rounds: Array<{ started_at: string; ended_at: string | null; result: string; selected: number | null; reason: string | null }> = [];
  for (const [daysAgo, count] of [[0, 6], [1, 9], [2, 8], [3, 7], [6, 8], [7, 5]] as const) {
    for (let index = 0; index < count; index += 1) {
      const minutes = 15 * index;
      const failed = daysAgo === 0 && index === 1;
      rounds.push({ started_at: at(daysAgo, 15 - Math.floor(minutes / 60), 30 - (minutes % 60) < 0 ? 0 : 30 - (minutes % 60)), ended_at: at(daysAgo, 15, 32), result: failed ? "poll_failed" : daysAgo === 0 && index === 2 ? "skipped" : "done", selected: failed ? null : 2, reason: failed ? "gh: could not reach GitHub" : daysAgo === 0 && index === 2 ? "the previous poll was still running" : null });
    }
  }
  return rounds.slice(0, 50);
}

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

const result = (overrides: Partial<PluginActionResult>): PluginActionResult => ({ exitCode: 0, stdout: "", stdoutTruncated: false, stderr: "", fingerprint: null, preview: null, ...overrides });
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

/** Answers show-review, draft, ask, retry, skip, remove, and round; null for the other actions. A row passes the run as `--item <run>`, the page as the first argument. */
export function answerReviewAction(sent: PluginActionRequest): PluginActionResult | null {
  const args = sent.args[0] === "--item" ? sent.args.slice(1) : sent.args;
  const [runId = "", ...rest] = args;
  const entry = entries.find((item) => item.run_id === runId);
  switch (sent.action) {
    case "show-review": {
      if (!entry) return refused(`whooper-show: no run ${runId}`);
      if (entry.outcome === "reviewed" && !findings[runId]) {
        // A review without findings of its own gets the nits when Whooper calls it safe to approve,
        // else the question; like Whooper, only a safe run's drafts start as an approval.
        findings[runId] = entry.safe_to_approve
          ? { verdict: "approve_once_ci_passes", reason: "Small and tested.", findings: nits.slice(0, Math.min(entry.findings, 2)) }
          : { verdict: entry.verdict ?? "look_first", reason: entry.verdict === "look_first" ? "One question about the delay." : "Small and tested, with one open question.", findings: [question] };
        drafts[runId] = { event: entry.safe_to_approve ? "APPROVE" : "COMMENT", message: "", findings: Object.fromEntries(findings[runId]!.findings.map((finding) => [finding.id, { included: entry.safe_to_approve }])) };
      }
      const found = findings[runId];
      return result({ stdout: JSON.stringify({
        version: 1, run_id: runId, run: { repo: entry.repo, pr: entry.pr }, title: entry.title, author: entry.author, outcome: entry.outcome,
        findings: found ? { status: "reviewed", skip_reason: "", approval: { verdict: found.verdict, reason: found.reason }, findings: found.findings } : null,
        drafts: drafts[runId] ?? null, diff: found ? patch : null,
        failure: failures[runId] ?? null, asks: asks[runId] ?? [], submitted: entry.submitted,
        head_sha: entry.head_sha, cause: entry.cause, skipped_by_user: entry.skipped_by_user,
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
      if (entry.outcome === "running") return refused(`whooper-retry: ${entry.repo}#${entry.pr} is being reviewed now`);
      return result({ stdout: `queued ${entry.repo}#${entry.pr} for the next round\n` });
    case "remove":
      if (!entry) return refused(`whooper-remove: no run ${runId} in /synthetic/state/whooper/runs`);
      entry.removed_by_user = !rest.includes("--undo");
      entry.needs_you = !entry.removed_by_user && !entry.skipped_by_user && entry.outcome !== "running" && entry.outcome !== "waiting" && entry.submitted === null;
      return result({ stdout: `${entry.removed_by_user ? "removed" : "restored"} ${entry.repo}#${entry.pr}\n` });
    case "skip":
      if (!entry) return refused(`whooper-skip: no run ${runId} in /synthetic/state/whooper/runs`);
      entry.skipped_by_user = !rest.includes("--undo");
      entry.needs_you = !entry.skipped_by_user && entry.outcome === "reviewed" && entry.submitted === null;
      return result({ stdout: `${entry.skipped_by_user ? "skipped" : "unskipped"} ${entry.repo}#${entry.pr}\n` });
    case "round":
      return result({ stdout: "polled: 2 review requests, 1 retry\nreviewing acme/widgets#55\nacme/widgets#55: approve once CI passes, 1 finding\nround done\n" });
    default:
      return null;
  }
}

/** A time as Whooper's rows say it: "3:12 PM" today, "Nov 24" before. */
const shortDate = (iso: string) => {
  const date = new Date(iso);
  return date.toDateString() === new Date().toDateString()
    ? date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : date.toLocaleDateString([], { month: "short", day: "numeric" });
};

/** The group of a pull request's newest review, by whooper-status's review_group; null once removed. */
function groupOf(entry: Entry): string | null {
  if (entry.removed_by_user) return null;
  const finished = entry.submitted !== null || entry.skipped_by_user || (entry.outcome === "reviewed" && !entry.needs_you);
  if (entry.outcome === "reviewed" && entry.needs_you && !finished) return entry.safe_to_approve ? "ready" : "look";
  if (finished) return "done";
  if (entry.outcome === "running") return "reviewing";
  const blocking = entry.cause?.scope === "all";
  if (entry.outcome === "waiting") return blocking ? "waiting" : "reviewing";
  return blocking ? "waiting" : "failed";
}

/** A pull request's newest review as an item of status format 2, by whooper-status's review_item. */
function itemOf(entry: Entry, group: string) {
  const name = `${entry.repo.split("/").at(-1)}#${entry.pr}`;
  const standing = entry.submitted ? `${entry.submitted.event === "APPROVE" ? "approved" : "commented"} ${shortDate(entry.submitted.at)}`
    : entry.skipped_by_user ? "skipped"
      : entry.outcome === "running" ? `reviewing since ${shortDate(entry.started_at)}`
        : entry.outcome === "waiting" ? (entry.cause ? `waiting · ${entry.cause.summary}` : "waiting for the next round")
          : entry.outcome === "failed" ? entry.cause?.summary ?? "review failed"
            : !entry.needs_you ? "not submitted"
              : entry.skipped ? "skipped, look first"
                : entry.findings > 0 ? `${entry.findings} finding${entry.findings === 1 ? "" : "s"}` : "no findings";
  const finished = entry.submitted !== null || entry.skipped_by_user;
  const dot = finished ? "idle" : ({ running: "running", waiting: "idle", failed: "error" } as Record<string, string>)[entry.outcome]
    ?? ({ ready: "success", look: "attention" } as Record<string, string>)[group] ?? "idle";
  const running = entry.outcome === "running";
  return {
    id: entry.run_id, group, title: entry.title ?? `${entry.repo}#${entry.pr}`,
    detail: [name, ...(entry.author ? [`@${entry.author}`] : []), standing].join(" · "), dot, project: null,
    updated_at: entry.submitted?.at ?? entry.ended_at ?? entry.started_at, route: `/reviews/${entry.run_id}`,
    actions: [...(running ? [] : ["retry"]), ...(!running && entry.submitted === null && !entry.skipped_by_user ? ["skip"] : []), ...(running ? [] : ["remove"])],
  };
}

/**
 * What whooper-status --json adds in status format 2 for these reviews, as its status_v2 does: the
 * items (the newest review of each pull request the user did not remove, current ones before the
 * finished ones, each part newest first) and a blocker, which a round rechecks, for each cause that
 * stops every review.
 */
export function fixtureStatusV2Parts() {
  const seen = new Set<string>();
  const items = [...entries].sort((left, right) => right.started_at.localeCompare(left.started_at)).flatMap((entry) => {
    const key = `${entry.repo}#${entry.pr}`;
    if (seen.has(key)) return [];
    seen.add(key);
    const group = groupOf(entry);
    return group ? [itemOf(entry, group)] : [];
  });
  const ordered = items.sort((left, right) => right.updated_at.localeCompare(left.updated_at)).sort((left, right) => Number(left.group === "done") - Number(right.group === "done"));
  const causes = new Map(items.flatMap((item) => {
    const cause = entries.find((entry) => entry.run_id === item.id)?.cause;
    return item.group === "waiting" && cause ? [[cause.id, cause.summary] as const] : [];
  }));
  return {
    blockers: [...causes].map(([id, title]) => ({ id, title, dot: "error", route: `/causes/${id}`, action: "round" })),
    items: ordered.slice(0, 50),
    truncated: ordered.length > 50,
  };
}
