# Provider protocol evidence

This document records version-specific observations that gate provider adapters. It contains no
credentials, existing conversation content, private paths, or account identity.

## Claude Code 2.1.205 (2026-09-04)

Authenticated probes establish the direct CLI boundaries below. The previous unauthenticated run
was inconclusive; it did not demonstrate unsupported behavior. Production adapter implementation
and application acceptance are separate from this protocol evidence.

| Behavior | Result | Sanitized evidence |
| --- | --- | --- |
| Two turns on one long-lived session | Pass | One session returned the requested ONE and TWO responses. |
| Completed-turn process restart and context recall | Pass | A resumed process returned an invented marker supplied only before restart, with the same session ID. |
| Interrupt and resume | Pass | Interruption produced terminal evidence, then the same session resumed successfully. |
| Stdio mutation denial and approval | Pass | Write was held before mutation; denial left no file. A separate approved request wrote exactly PROBE to the validated owned target. |
| Stdio single-select user question | Pass | AskUserQuestion accepted a selected option through updatedInput and returned BLUE. |
| Two Write calls in one assistant batch | Pass, serialized callbacks | Distinct tool/request IDs shared an assistant message ID. Denying the first released the second; approving the second created only its file. |
| Interrupt with permission pending | Pass | The held mutation did not run; shutdown invalidated the probe's response path. |
| One-level child output and lifecycle | Pass | Agent invocation, child-origin output, matching task start and successful termination were observed. |
| Depth-two identity and lifecycle | Pass | A nested Agent invocation attributed to the child linked to a distinct grandchild task; both tasks emitted successful termination. |
| Grandchild text forwarding | Not observed | The nested invocation and lifecycle arrived, but no output carried the grandchild invocation's parent_tool_use_id. |
| Legacy deferred denial and resume | Failed probe | A valid deferred Write was captured, but the restarted stream did not produce a result before its 120-second deadline. |
| Restart after initialize, before any prompt | Unsupported | A focused probe exited with native session not found in 0.85 seconds. Initialization alone did not create a resumable transcript; the temporary positive assertion was removed. |

### Adapter and desktop integration evidence

The Rust `ClaudeAdapter` now implements the selected stdio transport and is registered by the
desktop app when its health check succeeds. Health accepts major 2 at or above 2.1.205, checks
`claude auth status --json` under bounded process and output limits, and parses only `loggedIn`.
Account identifiers and the complete auth response are neither logged nor persisted. The version
policy permits compatibility checks on newer major-2 releases; live verification is specifically
2.1.205, and unknown or malformed runtime protocol data still fails closed.

The actual adapter passed two focused account-backed tests: streamed output with same-session
process restart/context recall, and denied/allowed exact temporary Write. These tests exposed
advisory `rate_limit_event` and `thinking_tokens` estimates; neither payload is persisted. The
adapter uses one process per active turn, default stdio permissions, empty settings sources, an
empty strict MCP config, and no model override. The legacy hook-defer/restart path is not used.

Hermetic application composition tests exercise projectless automatic selection and explicit
Claude override, persisted native-session resume after app reconstruction, canonical approval IDs
and question answers, stale response rejection, and completed depth-two sidebar ancestry. Existing
provider-switch tests cover handoff and fallback policy. Desktop tests verify that availability and
registered capabilities agree across authenticated, missing, unsupported, and malformed states.

Separate opt-in `live_app_` tests exercise canonical application APIs for repeated projectless
turns and Claude → Claude → Codex → Claude continuity, exact-file denial/approval, and recursive
child/grandchild completion. Those application tests are not claimed as passed here until run and
recorded by the release coordinator. They do not constitute native UI or notification evidence.

Claude advertises streaming, permission/question responses, interruption, resume, and child
hierarchy, but not steering. Multi-select questions are safely declined; duplicate question text
fails closed. Native task IDs establish ancestry and status, not independently resumable child
sessions. Grandchild text remains absent from the observed depth-two protocol stream. Cancellation
before the first prompt is kept fresh in-process, but restarting the app in that narrow state can
leave a missing native transcript and require a new conversation. Completed and interrupted prompt
sessions have resumed in the focused live probes. No replay/fallback policy is added for missing
native sessions, and private resource import remains outside this milestone.

### Probe boundaries

The ignored `claude_protocol` tests use only owned processes and fresh mode-0700 directories,
invented prompts, disabled filesystem setting sources, and an empty strict MCP configuration.
Workspace paths are canonical at creation: the CLI canonicalized a temporary-directory symlink,
which originally caused a false path mismatch despite matching canonical parents. Exact target and
content validation remains mandatory; an unrelated file with the same basename is rejected.

No-tool probes allow no tools; child probes expose only Agent. Stdio probes expose Write and
AskUserQuestion, use `--permission-mode default --permission-prompt-tool stdio`, and add no allow
rules or permission updates. Each process disables Chrome and has a USD 0.50 budget. Each operation
retains one absolute 120-second deadline. Normal and panic cleanup stop and await only the owned
process, bounded to ten seconds.

### Validated control and lifecycle semantics

The [public SDK transport](https://raw.githubusercontent.com/anthropics/claude-agent-sdk-python/main/src/claude_agent_sdk/_internal/transport/subprocess_cli.py)
and [control implementation](https://raw.githubusercontent.com/anthropics/claude-agent-sdk-python/main/src/claude_agent_sdk/_internal/query.py)
describe the stdio mechanism verified here. Initialize completes before the user message. An
incoming `control_request` carries `request_id` and `request` with `subtype: can_use_tool`,
`tool_name`, `input`, and `tool_use_id`. A matching `control_response` returns a success envelope
whose inner response is either `behavior: deny` with a message, or `behavior: allow` with the
validated `updatedInput`. No globally relaxed permission mode is needed.

The question response preserves the received questions and adds an answers map keyed by question
text. Only single-select was exercised live; the adapter safely declines multi-select and rejects
duplicate question text in hermetic tests.
See the [user-input documentation](https://code.claude.com/docs/en/agent-sdk/user-input).

Holding the first callback while waiting for a second timed out. The follow-up verified that both
Writes belonged to one assistant message and that answering the first released the second. This
establishes serialized permission checks for the observed batch, not concurrent outstanding
callbacks. Separate hermetic runtime tests cover application request arbitration and cancellation;
application composition tests cover stale-response protection through canonical approval IDs.

Child correlation uses `system/task_started` task/session/tool IDs and
`system/task_notification` with matching identity and status `completed`. A spawn acknowledgement
is not termination. Root success is held until required child evidence arrives under the original
deadline. The nested Agent block's `parent_tool_use_id` identifies its owning child invocation;
its own tool ID links to the grandchild lifecycle. Task IDs are native task identity, not proof of
independently resumable sessions. Top-level output remains required, while recursive hierarchy and
status do not depend on grandchild text forwarding. The current
[SDK types](https://raw.githubusercontent.com/anthropics/claude-agent-sdk-python/main/src/claude_agent_sdk/types.py)
also describe task_updated alternatives; those were not needed or validated in this gate.

The legacy defer/deny/resume probe remains as an explicitly unverified path. The
[hooks documentation](https://code.claude.com/docs/en/hooks#defer-a-tool-call-for-later) also limits
defer to a single tool call; it cannot replace general permission callbacks for a batch. The
demonstrated stdio flow is the selected approval boundary.

Run supported live acceptance separately from the known legacy failure:

```text
PROMPTING_TIME_LIVE_CLAUDE=1 cargo test -p prompting-time-core --test claude_protocol -- --ignored --nocapture --test-threads=1 --skip live_deferred_approval_can_resume
```

Synthetic regressions validate the probe's rejection and correlation logic; they are not live
evidence. Adapter, application, and native UI checks remain distinct evidence boundaries.
