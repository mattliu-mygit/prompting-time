# Optional Pxpipe context compression

Status: proposed staged integration; scope approved, written specification awaiting review.

## Intent and tradeoff

Offer an experimental, explicit opt-in to reduce bulky model context by rendering selected
text as images before provider requests leave the machine. Pxpipe owns that transformation;
Prompting Time owns lifecycle, privacy boundaries, supported-provider gating, and clear status.
Reuse a pinned upstream library rather than porting its renderer or compression heuristics.

This is lossy visual context, not the native context-budget/compaction feature and not ordinary
image upload. Exact identifiers, code bytes, negations, and detailed requirements can be misread.
The upstream factsheet does not make the transformation lossless. Keep Pxpipe off by default,
retain original canonical chat text, and never describe savings or accuracy as guaranteed.
The feature must not silently change the user's selected model, including Astra.

Manual text-to-image export is simpler but does not optimize the CLI's internally assembled
history. The selected approach is an app-managed local proxy. TLS interception via `warp`,
local certificate authorities, global proxy settings, and a public/network listener are outside
this initial integration.

## User-visible behavior

Expose Pxpipe as an experimental conversation setting separate from Thinking and Context
budget. Before first enablement, explain lossiness and that requests and provider credentials
pass through the local helper in memory. Explain that upstream token-count measurement can
send the original uncompressed request to the same provider as well.

Show Off, Active, or Unavailable with a concrete reason. A model allowlist entry alone is not
evidence of support: compression is enabled only for an app-verified provider, native transport,
authentication mode, and model/render profile. Unknown or unverified combinations retain normal
text transport and visibly report that compression is unavailable. Do not claim an active mode
when all traffic is bypassing the helper.

Changes apply at an idle turn boundary. They must not interrupt a turn, silently create a new
conversation, drop native history, or affect other conversations. Freeze the selected mode with
the accepted request so retries, recovery, and provider fallback cannot accidentally change its
meaning. If a destination provider cannot honor the mode, show the limitation rather than
silently sending a differently configured fallback.

If transport startup fails, reject before dispatch with a visible error. If the helper fails
after possible dispatch, preserve uncertain-send semantics; never automatically replay the
request directly. Turning Pxpipe off restores direct native transport at the next safe boundary.

## Architecture and privacy

Keep the existing Rust core and native provider adapters. Package an exact-version Node helper
around Pxpipe's exported proxy API, with its dependency lock and required license/asset notices.
Do not use runtime `npx`, package updates, or the stock proxy executable. Resolve and verify the
packaged runtime; the app must not depend on an arbitrary shell's Node installation.

The helper omits the stock dashboard, file tracker, source/PNG captures, and native-transcript
enrichment. It must not scan unrelated Claude/Codex histories, settings, skills, or memories.
Only bounded allowlisted status and aggregate metrics may return to Rust; never persist request
bodies, provider credentials, raw response errors, private paths, or rendered context pages.

Use a loopback-only app-owned listener. Reject browser-origin requests and protect ingress with
a per-launch random capability that is distinct from provider credentials and is not forwarded
upstream. Verify a native child-only mechanism for presenting that capability before enabling
a provider. If that mechanism cannot be established, leave that provider unsupported rather
than shipping an unauthenticated local proxy.

Keep each CLI responsible for authentication and token renewal; do not copy credentials from
its private files, override accounts, or substitute an API key for subscription authentication.
Restrict upstream destinations and routes to the verified provider endpoints, with explicit
streaming, cancellation, retry, and response-size behavior. Never redirect every app-server
session by changing the environment of a shared Codex process. Prove conversation isolation
and native-session continuity before enabling per-conversation Codex compression.

The helper is a child process owned by the app. Stop and reap it at shutdown and on failed
startup, and reconnect recovered work only through normal owned provider-session boundaries.
Original user image attachments continue to use the shared attachment path; internal proxy
renderings are not saved as user attachments or inserted into the visible chat transcript.

## Delivery gates

1. Complete ordinary image attachments and the focused Claude compatibility work first.
2. Validate the pinned library in a mock-upstream harness: authentication forwarding/isolation,
   local ingress protection, endpoint restrictions, streaming/cancellation, bounded input and
   output, disabled captures/enrichment, helper death, and no automatic request replay.
3. Verify each CLI's endpoint override with its existing authentication mode and resume behavior.
   Native first-party features may depend on endpoint configuration; document restrictions.
4. Use paired synthetic workloads to compare off/on behavior for exact identifiers, negations,
   Unicode, source edits, long history, token usage, and latency on the intended model/profile.
   Keep the experimental setting unavailable when evidence is insufficient. Large paid quality
   runs require a separately agreed budget; they are not implied by a tiny connectivity check.
5. Verify UI state, next-turn application, independent conversations, crash/restart cleanup,
   uncertain retries, and provider switching. Update canonical docs and remove temporary plans.

No percentage savings are promised by this design. Report measured context/token results and
native-provider limitations separately from unit-test or build success.

## Upstream references

- [Pxpipe modes, model quality, and lossiness](https://github.com/teamchong/pxpipe)
- [Security model](https://github.com/teamchong/pxpipe/blob/main/docs/SECURITY_MODEL.md)
- [Library exports](https://github.com/teamchong/pxpipe/blob/main/src/core/index.ts)

The initial source audit covered release v0.14.0 and main commit
75668a4a80751fce3ba4e2c0af79ee6af3c410f1. Verify the selected release again before packaging;
an upstream feature claim is not proof of compatibility with this app's installed CLIs.
