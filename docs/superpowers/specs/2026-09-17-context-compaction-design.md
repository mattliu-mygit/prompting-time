# Selectable native context compaction

Status: implemented, including presets through 500k. Automated and synthetic-browser
checks cover the controls and protocol boundaries. Authenticated high-context
compaction and native-WebView acceptance remain unverified.

## Intent

Keep long-running conversations usable by configuring the native harness's automatic
context compaction. Prompting Time chooses the context budget and presents progress;
Codex or Claude owns summarization, retained model context, and safe compaction timing.
This does not replace either harness's prompts or compaction algorithm.

## Conversation controls

A compact **Context budget** selector sits beside Thinking. Choices are **200k, 300k,
400k, 500k**, and **Provider default**. One k means 1,000 tokens. The default saved
preference is **300k** for new and existing managed conversations after migration;
historical runs and previously accepted requests retain their prior provider-default
behavior. This is separate from Thinking effort and provider routing.

A selected number requests automatic compaction with that native context budget. It
is not an exact token-count trigger, a hard stop, a cumulative usage allowance, or an
increase to the model's actual context capacity. Providers may compact earlier to
reserve response space or respect a smaller model limit. Retain the selected budget
when switching providers and explain this qualification next to the control's help.

**Provider default** removes the app's compaction override and restores the provider's
underlying behavior, including its own enabled/disabled preference. It must not retain
a sticky budget from an earlier app-selected preset. Changing this preference must
not rewrite global CLI settings or alter another conversation.

Save per conversation, preserving its draft and current run. While saving, prevent a
new send from racing the change. Failed saves keep the previous preference and show
the relevant error. Observed child conversations remain read-only; native subagents
follow their provider's behavior rather than receiving invented app-owned controls.

## Turn and session boundaries

Preference changes apply to the next new turn, never by interrupting or reconfiguring
active work. Steering retains the active turn's configuration. A newly accepted send
captures its compaction preference so uncertain retries, recovery, and safe fallback
do not adopt subsequent preference changes. Switching providers preserves requested
intent, but each native session has its own context and compaction history.

An adapter must apply the selected preference before delivering the prompt, including
when returning to a previously loaded native session. Accepting a configuration-shaped
RPC is not sufficient evidence that a loaded session changed. Preserve native session
identity and concurrent conversations; do not implement this by replacing a session
with a fresh chat or restarting a shared process that owns active work.

For a numbered preset, request both native automatic compaction and its budget. Do
not report success if an inherited disable flag, provider restriction, or unsupported
CLI prevents it from taking effect. When the adapter cannot establish a supported,
safe application path, reject before prompt dispatch with an actionable explanation.
Provider default remains an explicit way to continue with unchanged native behavior.
No app-level summarization fallback or repeated compaction retry loop is introduced.

## Progress and retained history

Show one compact **Compacting…** indication and a **Context compacted** completion
entry when the provider reports those events. Missing start events must not prevent
a reported completion from being recorded. A successful settings request does not
mean compaction happened. A compaction error must not produce a success marker or
silently replay a potentially dispatched user request.

The local visible transcript remains intact: compaction changes provider working
context, not the app's saved messages. Correlate notices to their actual conversation
and native run; do not label a child's event as the parent's compaction. Deduplicate
replayed native events using the existing event-ownership boundaries.

Keep requested budget separate from any provider-reported effective configuration.
Show an adjusted limit or before/after token count only when evidence supports it;
otherwise explain that native limits and headroom apply. Lifetime billing totals
must never be shown as current context occupancy. A new live token meter is not part
of this initial feature.

## Architecture and compatibility

Extend the existing Rust conversation preference and frozen submission boundaries,
with generated bridge types and a small composer control. Provider adapters translate
the shared intent into native settings and normalize bounded compaction observations.
The frontend does not reproduce native token-estimation or summarization algorithms.

Numbered budgets are supported on the verified Codex **0.153.4** and Claude Code
**2.1.205** contracts. Other versions fail before dispatch rather than claiming an
unverified override; Provider default retains the adapter's baseline compatibility.
Supporting another version requires revalidating its native configuration behavior.

Codex applies initial configuration when creating the thread. Changing or resetting
a loaded thread requires an idle, owned reload with fresh native unload/reload evidence;
a successful resume response alone is insufficient. The thread identity is preserved,
and other conversations are not restarted. Claude uses process-local settings and
bounded effective-settings readback before sending the prompt. Native headroom and
model capacity still apply; requested-window readback is not an exact trigger value.

Do not inflate model capacity, edit global settings, change approval/permission/MCP
boundaries, persist complete provider settings, or import private resources. No model
picker, custom summarizer, manual Compact now button, unrelated layout changes, CLI
upgrade, or migration of a real private database is included.

## Acceptance and completion

- Preferences round-trip independently across conversations and restart; every preset
  through 500k is selectable, 300k is the default, and Provider default resets overrides.
- Old accepted requests/history retain their meaning. New requests freeze the selected
  budget across retry, recovery, and provider fallback; active steering is unchanged.
- Native fixtures verify first use, resume, loaded-session changes, reset, provider
  switching, smaller model limits, disabled/unsupported compaction, and no prompt
  dispatch on configuration failure. No global writes or unrelated session restarts.
- Compaction notices are truthful, correctly owned, deduplicated, and non-destructive
  to saved chat history. Failed compaction never becomes a completion notice.
- Browser checks cover save failures, navigation/drafts, keyboard conventions, and
  composer wrapping at the minimum window size and increased effective scale.

Use focused regression tests, generated-binding checks, proportionate final lint/build
checks, and an independent review. Synthetic fixtures and a successful macOS build do
not establish authenticated-provider or native-WebView acceptance; report those limits
separately. Any live provider probe needs a separately agreed scope and token budget.

This document is the feature's authoritative behavior specification. On completion,
mark its status implemented, link it from the main product specification, and remove
superseded temporary execution plans without duplicating the specification.

## Native references

- [Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
- [Codex App Server](https://learn.chatgpt.com/docs/app-server)
- [Claude context window and auto-compaction](https://code.claude.com/docs/en/model-config#context-window-and-auto-compaction)
- [Claude environment variables](https://code.claude.com/docs/en/env-vars)

Capability research inspected Codex 0.153.4 and Claude Code 2.1.205. Public documentation
can describe newer versions; it does not substitute for installed-version verification.
