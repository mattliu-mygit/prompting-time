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
`Agent` tool call, at least one child-origin message correlated through `parent_tool_use_id`, and a
matching tool result. It captures and checks a stable `agentId` when that version exposes one.

Command:

```text
PROMPTING_TIME_LIVE_CLAUDE=1 cargo test -p prompting-time-core --test claude_protocol -- --ignored --nocapture --test-threads=1
```

Observed result:

| Required behavior | Result | Sanitized evidence |
| --- | --- | --- |
| Two turns on one long-lived session | Inconclusive | The first turn returned `Not logged in - Please run /login` instead of model output. |
| Deferred approval and same-session resume | Inconclusive | No `deferred_tool_use` was emitted because the unauthenticated run never proposed a tool call. |
| Interrupt and resume | Inconclusive | No assistant delta arrived before the bounded probe timeout because the run was unauthenticated. |
| Stable child-agent identity | Inconclusive | No child-agent start or stop event was emitted because the unauthenticated run never started an agent. |

A separate read-only `claude auth status --json` check returned `loggedIn: false`, `authMethod:
none`, and `apiProvider: firstParty`. This is an environment blocker, not evidence that any protocol
behavior is unsupported. The direct CLI adapter decision therefore remains pending and the Agent SDK
sidecar fallback is not selected.

To unblock the gate, authenticate the installed CLI interactively with `claude` followed by
`/login`, then rerun the command above. Adapter implementation, protocol fixtures, and any Claude
feature commit must remain blocked until all four behaviors are demonstrated.
