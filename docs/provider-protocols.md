# Provider protocol evidence

This document records version-specific observations that gate provider adapters. It contains no
credentials, transcript content from existing conversations, private paths, or machine-specific
configuration.

## Claude Code

### 2.1.205 protocol gate (2026-09-03)

The installed CLI reports `2.1.205 (Claude Code)`. Its help exposes the intended non-interactive
surface: stream-JSON input and output, partial messages, explicit session IDs, resume, permission
modes, and agent tools. Source inspection of Anthropic's public Agent SDK also confirms that the CLI
has a bidirectional JSON-lines control protocol for initialization, interruption, and permission
responses. Those static observations do not demonstrate the runtime behaviors Prompting Time needs,
so the live gate remains mandatory.

The ignored `claude_protocol` tests launched only Prompting Time-owned processes in fresh temporary
directories with mode `0700`, disabled filesystem setting sources, an empty strict MCP
configuration, restrictive `dontAsk` or `manual` permissions, and invented prompts. Prompts were
written as structured JSON to stdin. No-tool probes pass an empty tool allowlist, the mutation probe
allows only `Write`, and the child probe allows only the installed version's `Agent` tool. Every
process disables Chrome and has a USD 0.50 maximum budget. The approval probe uses a temporary
startup-loaded `PreToolUse` hook whose `deny` state survives process restart; it does not bypass
permissions. Normal and panic cleanup request shutdown and wait for the owned process for up to ten
seconds. The observed post-gate process inventory contained no live-probe child.

Each full control request or turn uses one absolute 120-second deadline rather than restarting a
timeout after every event. Result validation requires a non-error `success` subtype. The approval
probe additionally requires `stop_reason: tool_deferred`, a nonempty deferred `Write` ID, and the
exact temporary path and `PROBE` content before resuming. The denial resume must finish successfully
without another deferred call or filesystem mutation. The child probe requires a top-level
`Agent` tool call, child-origin output correlated through `parent_tool_use_id`, and explicit
successful child lifecycle termination. A matching tool result can acknowledge a background spawn
and is not completion evidence. The child-specific collector continues after a root success until
the child evidence passes or the original operation deadline expires.

The synthetic gate correlates `system/task_started` by session and `tool_use_id`, preserves its
native `task_id`, and requires a `system/task_notification` with the same session/task, status
`completed`, and matching `tool_use_id` when present. Missing evidence, wrong identities, and
failed/stopped termination cannot pass. These are candidate wire shapes from the
[public SDK types](https://raw.githubusercontent.com/anthropics/claude-agent-sdk-python/main/src/claude_agent_sdk/types.py),
not authenticated observations of installed 2.1.205. The current public types also describe
`task_updated` alternatives; this narrow probe requires `task_notification` and may remain
inconclusive on a runtime emitting only that alternative. Synthetic regressions cover spawn-only
acknowledgements and termination before/after the root result; they do not establish live support.

Command:

```text
PROMPTING_TIME_LIVE_CLAUDE=1 cargo test -p prompting-time-core --test claude_protocol -- --ignored --nocapture --test-threads=1
```

Observed result:

| Required behavior | Result | Sanitized evidence |
| --- | --- | --- |
| Two turns on one long-lived session | Inconclusive | The first turn returned `Not logged in - Please run /login` instead of model output. |
| Deferred denial and same-session resume | Inconclusive | No `deferred_tool_use` was emitted because the unauthenticated run never proposed a tool call. |
| Interrupt and resume | Inconclusive | No assistant delta arrived before the bounded probe timeout because the run was unauthenticated. |
| One-level child lifecycle and identity | Inconclusive | No child-agent start or stop event was emitted because the unauthenticated run never started an agent. |

A separate read-only `claude auth status --json` check returned `loggedIn: false`, `authMethod:
none`, and `apiProvider: firstParty`. This is an environment blocker, not evidence that any protocol
behavior is unsupported. The direct CLI adapter decision therefore remains pending and the Agent SDK
sidecar fallback is not selected.

To unblock the gate, authenticate the installed CLI interactively with `claude` followed by
`/login`, then rerun the command above. Adapter implementation, protocol fixtures, and any Claude
feature commit must remain blocked until all four behaviors are demonstrated. These four probes
are necessary but do not establish full adapter readiness: recursive parent/child correlation and
affirmative approval still need live acceptance coverage. The existing defer/deny/resume mechanism
is retained; its planned scope is denial and continuation, not an affirmative authorization path.
