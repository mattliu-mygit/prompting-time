# Everyday UX follow-ups

Candidate work, not yet approved for implementation. Keep each change small
and validate its behavior before adding the next.

## Recommended order

1. **Preserve per-conversation drafts.** Switching away and back should not discard
   unsent text. The current composer owns its draft in local React state and is
   keyed by conversation, so changing conversations remounts it. Start with
   in-session preservation; disk persistence needs an explicit privacy/retention
   decision because drafts may contain sensitive material.
2. **Quick conversation switching and search.** A keyboard-opened switcher should
   find conversations by title/project and restore composer focus. Search must
   clearly distinguish loaded results from all stored conversations.
3. **Discoverable commands and shortcuts.** A small command palette can expose
   existing actions such as new conversation, pane toggles, focus composer, and
   zoom/reset. Reuse the existing action handlers instead of creating a parallel
   command implementation.
4. **Parent/child navigation.** Make jumping from an orchestrator to its children
   and back convenient without taking over arrow keys while editing text.

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
