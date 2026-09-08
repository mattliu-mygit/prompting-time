# Everyday UX follow-ups

Candidate work, not yet approved for implementation. Keep each change small
and validate its behavior before adding the next.

## Recommended order

1. **Unread and attention indicators.** Distinguish new responses from approvals
   needing action, and navigate to the next conversation needing attention.
   Extend the existing background notifications rather than duplicating them.
2. **Conversation organization.** Rename folder-derived titles, pin frequent
   conversations, and expose archived conversations with a restore action.
3. **Find within conversation.** Search message contents with explicit coverage
   of older history, not just the currently mounted page.
4. **Optional durable drafts.** In-session preservation is implemented. Disk
   persistence needs an explicit privacy/retention decision because drafts may
   contain sensitive material.
5. **More palette actions.** The shared palette covers conversation switching and
   common shell actions. Extend it with existing actions such as zoom/reset only
   through their authoritative handlers.

## Separately scoped retry issue

Code review confirmed an inherited edge case: if a send receives an ambiguous
transport result but the accepted run later appears active, Composer may route a
retry through steering instead of retrying the original send command identity.
This routing existed before draft preservation. Reproduce it with a targeted
accepted-but-response-lost fixture and fix it as a separate submission-state change;
the current preservation feature does not establish exactly-once behavior across
that route transition.

## Inspiration and boundaries

OpenCode documents session listing, command discovery, and parent/child navigation
in its [keybindings reference](https://opencode.ai/docs/keybinds). Its
[session navigation reference](https://opencode.ai/v2/docs/cli/keybinds/) also
exposes paging and first/last-message movement. Adapt the useful actions to macOS
conventions; do not copy terminal leader-key sequences or override native text
editing shortcuts mechanically.

Undo/retry, message editing, and queueing are larger workflow changes, not merely
new buttons: provider continuity, already-applied file edits, duplicate execution,
and active-run state require their own design. They are deferred from this pass.
