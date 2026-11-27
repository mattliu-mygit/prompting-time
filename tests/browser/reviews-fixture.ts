import type { PluginAction, PluginActionRequest, PluginActionResult } from "../../src/bridge/types";

// `?agents=reviews`: the invented Whooper lists reviews in its status, and the actions its page and
// the failed-review screen run (show-review, draft, ask, submit, retry, and round) answer in browser
// memory the way Whooper's commands would. Every repo, person, and line of code is made up.

export const reviewActions: PluginAction[] = [
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
  "fixture-run-68": {
    verdict: "approve_once_ci_passes",
    reason: "A small rule with a test; two nits.",
    findings: [
      { id: "1", path: "internal/projects/name.go", line: 9, side: "RIGHT", title: "Two dashes pass", kind: "nit", draft: "**nit:** `namePattern` lets `a--b` through; some bucket stores refuse two dashes in a row." },
      { id: "2", path: "internal/projects/create.go", line: 22, side: "RIGHT", title: "The bucket is the name", kind: "nit", draft: "**nit:** `bucket := name` reads as a leftover; `store.NewBucket(ctx, name)` says the same." },
    ],
  },
};
const drafts: Record<string, Drafts> = {
  "fixture-run-101": { event: "APPROVE", message: "", findings: { "1": { included: true }, "2": { included: true } } },
  "fixture-run-103": { event: "COMMENT", message: "", findings: {} },
  "fixture-run-88": { event: "APPROVE", message: "", findings: { "1": { included: true } } },
  "fixture-run-77": { event: "COMMENT", message: "", findings: { "1": { included: true, text: "**question:** Should the delay stop doubling at a cap, such as 30 seconds?" } } },
  "fixture-run-68": { event: "APPROVE", message: "", findings: { "1": { included: true }, "2": { included: true } } },
};

/**
 * The overview Whooper writes of a run, its walkthrough's content, for the runs that have one:
 * review 2 of #101, which says approve once CI passes, and #103, which says look first.
 */
const retryOverview = {
  summary: "Uploads get a `RetryPolicy`: five attempts, a delay that doubles from 250 ms, and a yes for cancelled uploads. The upload module exports it.",
  verdict_reason: "The policy is small and its tests pass; two nits about naming and cancelled uploads.",
  before_after: [
    { label: "A failed upload", before: "fails at once", after: "tries 5 times, waiting 250 ms, then 500 ms, 1 s, 2 s" },
    { label: "A cancelled upload", before: "stops", after: "tries again, since `retries_cancelled` says yes" },
  ],
  places: [
    { id: "p1", title: "The delay doubles without a cap", reason: "`delay_before` doubles each attempt, so a policy with more attempts waits hours, and `2u32.pow` overflows after 32.", evidence: { kind: "read", detail: "read `delay_before`; no test sets more than 5 attempts" }, path: "src/upload/retry.rs", start_line: 15, end_line: 18 },
    { id: "p2", title: "Cancelled uploads retry", reason: "`retries_cancelled` always says yes, so an upload the user stopped starts again.", evidence: { kind: "searched", detail: "searched for callers; the client doesn't check it yet" }, path: "src/upload/retry.rs", start_line: 20, end_line: 23 },
    { id: "p3", title: "The policy is public", reason: "`pub use retry::RetryPolicy` makes the type part of the crate's interface.", evidence: { kind: "ran", detail: "ran `cargo doc`; RetryPolicy is listed" }, path: "src/upload/mod.rs", start_line: 2, end_line: 5 },
  ],
  layers: [
    { id: "policy", title: "The retry policy", intent: "A new type that says how many times to try and how long to wait.", read_hint: "read", ranges: [{ path: "src/upload/retry.rs", start_line: 1, end_line: 13 }] },
    { id: "delays", title: "Delays and cancelled uploads", intent: "How the wait grows, and which uploads may try again.", read_hint: "read", ranges: [{ path: "src/upload/retry.rs", start_line: 14, end_line: 24 }] },
    { id: "exports", title: "The module exports it", intent: "Two lines that make the policy public.", read_hint: "skim", ranges: [{ path: "src/upload/mod.rs", start_line: 1, end_line: 8 }] },
  ],
  flow: {
    title: "A failed upload",
    input: "upload(\"report.csv\") while the server answers 503",
    calls: [
      { from: "UploadClient", to: "server", label: "send once", status: "removed", old_label: null, path: null, line: null },
      { from: "UploadClient", to: "RetryPolicy", label: "standard()", status: "added", old_label: null, path: "src/upload/retry.rs", line: 11 },
      { from: "UploadClient", to: "server", label: "send", status: "same", old_label: null, path: null, line: null },
      { from: "UploadClient", to: "RetryPolicy", label: "delay_before(n)", status: "changed", old_label: "fail", path: "src/upload/retry.rs", line: 16 },
      { from: "RetryPolicy", to: "RetryPolicy", label: "retries_cancelled()", status: "added", old_label: null, path: "src/upload/retry.rs", line: 21 },
    ],
  },
  checked: ["Ran the upload tests at head and base", "Searched for other retry loops in the client", "Built the docs"],
  not_checked: ["Didn't run the integration suite, which needs a server", "Didn't time a long retry"],
};
/** gadgets#68, in Go: project names must fit a bucket name, checked by a new ValidateName in place of slugify. */
const namePatch = `diff --git a/internal/projects/name.go b/internal/projects/name.go
new file mode 100644
index 0000000..5d41c2a
--- /dev/null
+++ b/internal/projects/name.go
@@ -0,0 +1,17 @@
+package projects
+
+import (
+\t"fmt"
+\t"regexp"
+)
+
+// namePattern is what a bucket name allows: 3 to 63 lowercase letters, digits, and dashes.
+var namePattern = regexp.MustCompile(\`^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$\`)
+
+// ValidateName says why a project name can't name a bucket, or nil when it can.
+func ValidateName(name string) error {
+\tif !namePattern.MatchString(name) {
+\t\treturn fmt.Errorf("project name %q: use 3 to 63 lowercase letters, digits, and dashes", name)
+\t}
+\treturn nil
+}
diff --git a/internal/projects/create.go b/internal/projects/create.go
index 7c1e0a4..b92f3d8 100644
--- a/internal/projects/create.go
+++ b/internal/projects/create.go
@@ -3,7 +3,6 @@ package projects
 import (
 \t"context"
 \t"fmt"
-\t"strings"
 
 \t"example.invalid/acme/gadgets/internal/storage"
 )
@@ -18,13 +17,11 @@ type Project struct {
 // Create makes a project and the bucket that holds its files.
 func Create(ctx context.Context, store *storage.Client, name string) (*Project, error) {
-\tbucket := slugify(name)
+\tif err := ValidateName(name); err != nil {
+\t\treturn nil, err
+\t}
+\tbucket := name
 \tif err := store.NewBucket(ctx, bucket); err != nil {
 \t\treturn nil, fmt.Errorf("create bucket %s: %w", bucket, err)
 \t}
 \treturn &Project{Name: name, Bucket: bucket}, nil
 }
-
-// slugify turns any name into a bucket name, so two names can share a bucket.
-func slugify(name string) string {
-\treturn strings.ToLower(strings.ReplaceAll(name, " ", "-"))
-}
diff --git a/internal/projects/name_test.go b/internal/projects/name_test.go
new file mode 100644
index 0000000..e3a0b61
--- /dev/null
+++ b/internal/projects/name_test.go
@@ -0,0 +1,11 @@
+package projects
+
+import "testing"
+
+func TestValidateName(t *testing.T) {
+\tfor _, name := range []string{"ab", "Has Space", "-dash-first"} {
+\t\tif ValidateName(name) == nil {
+\t\t\tt.Errorf("ValidateName(%q) = nil, want an error", name)
+\t\t}
+\t}
+}
`;

/** The patch each run read; runs not listed read the retry policy's. */
const patches: Record<string, string> = { "fixture-run-68": namePatch };

type Overview = Omit<typeof retryOverview, "flow"> & { flow: typeof retryOverview.flow | null };
const overviews: Record<string, Overview> = {
  "fixture-run-101": retryOverview,
  "fixture-run-103": { ...retryOverview, verdict_reason: "The delay can overflow for long retries; look at it before approving.", places: retryOverview.places.slice(0, 2) },
  "fixture-run-68": {
    summary: "Projects check their names before they get a bucket: `ValidateName` refuses what a bucket can't be named, in place of `slugify`, which turned any name into one.",
    verdict_reason: "A small rule with a test; two nits.",
    before_after: [
      { label: "A project named \"Q3 Report\"", before: "gets the bucket `q3-report`", after: "fails: use 3 to 63 lowercase letters, digits, and dashes" },
      { label: "Projects \"a b\" and \"a-b\"", before: "share the bucket `a-b`", after: "\"a b\" fails, so each bucket has one project" },
    ],
    places: [
      { id: "p1", title: "Names with spaces now fail", reason: "`slugify` used to turn \"Q3 Report\" into `q3-report`; now `Create` refuses it, and the API passes the error on.", evidence: { kind: "read", detail: "read `Create` and `api.handleCreate`" }, path: "internal/projects/create.go", start_line: 19, end_line: 21 },
      { id: "p2", title: "Two dashes in a row pass", reason: "`namePattern` allows `a--b`, which some bucket stores refuse.", evidence: { kind: "ran", detail: "ran `TestValidateName`; no case has two dashes" }, path: "internal/projects/name.go", start_line: 9, end_line: 9 },
    ],
    layers: [
      { id: "rule", title: "The name rule", intent: "What a bucket name allows, and the error that says so.", read_hint: "read", ranges: [{ path: "internal/projects/name.go", start_line: 1, end_line: 17 }] },
      { id: "create", title: "Create checks names", intent: "The check takes the place of `slugify`, which nothing calls now.", read_hint: "read", ranges: [{ path: "internal/projects/create.go", start_line: 3, end_line: 27 }] },
      { id: "test", title: "A test of the rule", intent: "Three names the rule refuses.", read_hint: "skim", ranges: [{ path: "internal/projects/name_test.go", start_line: 1, end_line: 11 }] },
    ],
    flow: null,
    checked: ["Ran `go test ./internal/projects/...` at head and base", "Searched for other callers of `slugify`"],
    not_checked: ["Didn't create a real bucket"],
  },
};

/**
 * How each run's changed code connects, as show-review prints it: gadgets#68's new rule and its
 * callers. gopls couldn't load internal/legacy, so its caller came from a search for the name, as
 * did api.signup's, which calls users.ValidateName, a different function with the same name.
 */
const connections: Record<string, object> = {
  "fixture-run-68": {
    nodes: [
      { id: "validate", name: "projects.ValidateName", kind: "func", path: "internal/projects/name.go", line: 12, change: "added", lines_changed: 6 },
      { id: "pattern", name: "projects.namePattern", kind: "var", path: "internal/projects/name.go", line: 9, change: "added", lines_changed: 2 },
      { id: "create", name: "projects.Create", kind: "func", path: "internal/projects/create.go", line: 18, change: "changed", lines_changed: 5 },
      { id: "slugify", name: "projects.slugify", kind: "func", path: "internal/projects/create.go", line: 28, change: "removed", lines_changed: 5 },
      { id: "test", name: "projects.TestValidateName", kind: "func", path: "internal/projects/name_test.go", line: 5, change: "added", lines_changed: 7 },
      { id: "handle", name: "api.handleCreate", kind: "func", path: "internal/api/projects.go", line: 41, change: "unchanged", lines_changed: 0 },
      { id: "import", name: "migrate.importProjects", kind: "func", path: "internal/migrate/import.go", line: 58, change: "unchanged", lines_changed: 0 },
      { id: "copy", name: "legacy.copyProject", kind: "func", path: "internal/legacy/copy.go", line: 22, change: "unchanged", lines_changed: 0 },
      { id: "signup", name: "api.signup", kind: "func", path: "internal/api/users.go", line: 30, change: "unchanged", lines_changed: 0 },
    ],
    edges: [
      { from: "handle", to: "create", kind: "calls", found_by: "gopls" },
      { from: "import", to: "create", kind: "calls", found_by: "gopls" },
      { from: "copy", to: "create", kind: "calls", found_by: "name_search" },
      { from: "create", to: "validate", kind: "calls", found_by: "gopls" },
      { from: "test", to: "validate", kind: "calls", found_by: "gopls" },
      { from: "signup", to: "validate", kind: "calls", found_by: "name_search" },
      { from: "validate", to: "pattern", kind: "uses", found_by: "gopls" },
    ],
    limits: [
      "gopls couldn't load internal/legacy, which doesn't build, so its callers come from name search.",
      "Callers in other repositories, such as acme/tools, aren't searched.",
    ],
  },
};
/**
 * Which changed lines each run's tests ran, and which changes made on purpose they noticed, as
 * show-review prints it. gadgets#68's tests run only ValidateName, and only with names it refuses,
 * so no test ran Create's new lines or ValidateName's `return nil`. They caught both changes Whooper
 * made to ValidateName's check, but none of their names is long enough to notice the one to
 * namePattern's limit, in the second place to look. widgets#103 changes Rust, which Whooper doesn't
 * measure.
 */
const testGaps: Record<string, object> = {
  "fixture-run-68": {
    measured: true,
    reason: null,
    tool: "go test -coverprofile ./internal/projects/...",
    lines: [
      { path: "internal/projects/create.go", line: 19, status: "not_run" },
      { path: "internal/projects/create.go", line: 20, status: "not_run" },
      { path: "internal/projects/create.go", line: 21, status: "not_run" },
      { path: "internal/projects/create.go", line: 22, status: "not_run" },
      { path: "internal/projects/name.go", line: 9, status: "survived", change: "`{1,61}` to `{1,62}`" },
      { path: "internal/projects/name.go", line: 13, status: "caught", change: "`!namePattern.MatchString(name)` to `namePattern.MatchString(name)`", test: "TestValidateName" },
      { path: "internal/projects/name.go", line: 14, status: "caught", change: "`fmt.Errorf(…)` to `nil`", test: "TestValidateName" },
      { path: "internal/projects/name.go", line: 16, status: "not_run" },
    ],
    summary: { changed_lines: 21, run: 4, not_run: 5, survived: 1, caught: 2 },
    limits: [
      "Whooper changed only lines a test ran, one change on each, so a line no test ran has none.",
      "A change no test noticed can still behave the same as the code it replaced, so that no test could notice it.",
      "Only the tests in internal/projects ran; tests of code that calls Create, such as internal/api, didn't.",
      "name_test.go is a test, so its lines aren't counted.",
    ],
  },
  "fixture-run-103": {
    measured: false,
    reason: "Whooper measures Go files only so far, and this pull request changes Rust.",
    tool: "go test -coverprofile",
    lines: [],
    summary: { changed_lines: 0, run: 0, not_run: 0 },
    limits: [],
  },
};

const nameError = (name: string) => `error: project name "${name}": use 3 to 63 lowercase letters, digits, and dashes`;
const project = (name: string, bucket: string) => `&Project{Name: "${name}", Bucket: "${bucket}"}`;
/**
 * The example inputs each run ran through the old and new code, as show-review prints them:
 * gadgets#68's Create at base and head, with an in-memory store, and ValidateName, new in the pull
 * request, at its head only.
 */
const examples: Record<string, object> = {
  "fixture-run-68": {
    functions: [
      { name: "projects.Create", path: "internal/projects/create.go", line: 18 },
      { name: "projects.ValidateName", path: "internal/projects/name.go", line: 12 },
    ],
    rows: [
      { function: "projects.Create", input: "\"reports-2026\"", before: project("reports-2026", "reports-2026"), after: project("reports-2026", "reports-2026"), status: "same", label: "ran_base_and_head", note: null },
      { function: "projects.Create", input: "\"Q3 Report\"", before: project("Q3 Report", "q3-report"), after: nameError("Q3 Report"), status: "different", label: "ran_base_and_head", note: null },
      { function: "projects.Create", input: "\"a b\"", before: project("a b", "a-b"), after: nameError("a b"), status: "different", label: "ran_base_and_head", note: "\"a-b\" gets the same bucket at base" },
      { function: "projects.Create", input: "\"a-b\"", before: project("a-b", "a-b"), after: project("a-b", "a-b"), status: "same", label: "ran_base_and_head", note: null },
      { function: "projects.Create", input: "\"ab\"", before: project("ab", "ab"), after: nameError("ab"), status: "different", label: "ran_base_and_head", note: null },
      { function: "projects.ValidateName", input: "\"a--b\"", before: null, after: "nil", status: "different", label: "ran_head_only", note: "new in this PR" },
      { function: "projects.ValidateName", input: "\"Q3 Report\"", before: null, after: nameError("Q3 Report"), status: "different", label: "ran_head_only", note: "new in this PR" },
    ],
    limits: [
      "Create ran with an in-memory storage.Client, not a real bucket store.",
      "Each input ran twice at the base; an input whose two runs differed would say so.",
    ],
  },
};

/** The commits after each run's head, as show-review lists them: #101 moved on after its review 2. */
const sinceReview: Record<string, Array<{ sha: string; title: string; author: string; at: string; files: string[] }>> = {
  "fixture-run-101": [
    { sha: sha(9100), title: "upload: cap the retry delay at 30 s", author: "lin-w", at: at(0, 15, 0), files: ["src/upload/retry.rs"] },
    { sha: sha(9101), title: "docs: describe upload retries", author: "lin-w", at: at(0, 15, 10), files: ["docs/uploads.md"] },
  ],
};
const coChange: Record<string, Array<{ path: string; partner: string; ratio: number; commits: number }>> = {
  "fixture-run-101": [{ path: "src/upload/client.rs", partner: "src/upload/mod.rs", ratio: 0.75, commits: 12 }],
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
        drafts: drafts[runId] ?? null, diff: found ? patches[runId] ?? patch : null,
        failure: failures[runId] ?? null, asks: asks[runId] ?? [], submitted: entry.submitted,
        head_sha: entry.head_sha, cause: entry.cause, skipped_by_user: entry.skipped_by_user,
        overview: found ? overviews[runId] ?? null : null, commits_since_review: sinceReview[runId] ?? (found ? [] : null), co_change: coChange[runId] ?? [],
        connections: found ? connections[runId] ?? null : null,
        test_gaps: found ? testGaps[runId] ?? null : null,
        examples: found ? examples[runId] ?? null : null,
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
