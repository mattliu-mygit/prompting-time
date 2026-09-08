# Folder-first conversation creation

Status: agreed interaction, awaiting written-spec review before implementation.

## Intent

Starting a conversation should require choosing a folder, not filling out metadata.
The ordinary flow is **Choose folder → native folder picker → ready to chat**.
There is no subsequent confirmation form or manual directory check. Selecting a
folder creates an empty conversation; it does not submit a prompt or start a provider.

## Observable behavior

New conversation presents a compact chooser with a primary **Choose folder** action,
a **Without a folder** alternative, and collapsed **Advanced** options. The primary
action opens the native macOS directory picker, restricted to a single folder.
Selecting a folder automatically checks it, creates the conversation, selects it in
the sidebar, and focuses the composer. Cancelling the picker creates nothing and
returns to the chooser without an error.

New conversations default to automatic provider selection with **Best fit**. Existing
conversations keep their saved routing profiles and provider overrides.

The selected folder's final path component supplies the initial title, with
**New conversation** as the fallback. Projectless conversations use that fallback.
Neither title nor objective is required from the user. The initial objective is empty;
the first ordinary user message supplies the task through existing message history.
Do not guess an objective from folder contents, duplicate that message into a new
metadata mechanism, or add a model call to name the conversation.

Git selections use the existing isolated-worktree behavior by default. Non-Git
directories are used directly. Preserve existing Git-root resolution and worktree
starting-state semantics; isolation does not copy uncommitted checkout edits.
**Without a folder** retains the existing app-owned projectless workspace.

Advanced options let the user explicitly choose **Current checkout** instead of the
Git default and override routing before choosing the folder or creating a projectless
conversation. Explain that the execution override only affects Git folders. Do not
restore mandatory title, objective, workspace-type, path, or inspection controls.

## Boundaries and failure handling

The desktop layer owns the native picker; the existing typed bridge and Rust
workspace service remain responsible for path validation, Git detection, preparation,
persistence, and rollback. Keep the provider runtime and canonical conversation model
unchanged except for new-conversation routing defaults. Do not infer Git status from
a filename or browser-supplied flag.

Only one picker/check/create operation can run at a time. Ignore stale results after
the chooser closes; a dismissed picker must never create a conversation later.
During committed creation, prevent duplicate submission and ambiguous cancellation.
Surface directory and worktree errors with a retry or choose-another-folder action.
Never silently fall back from failed isolation to modifying the current checkout.
Retain existing cleanup guarantees if persistence fails after workspace preparation.

Keyboard navigation, focus restoration, busy states, and actionable errors remain
accessible. A picker cancellation is distinct from a picker failure.

## Verification and scope

Cover native-picker result/cancellation/error handling, Git and non-Git automatic
selection, projectless creation, overrides, Best fit defaults, unchanged existing
settings, stale completion, duplicate admission, creation failure, and composer focus.
Check that a first message reaches normal provider handoff without an invented task.
Use existing component, bridge, Rust workspace, and routing test patterns. Build the
macOS bundle and verify native picker behavior where practical; label any browser-only
or mocked evidence honestly.

No private files are imported, no existing conversation is migrated, no provider or
account setting is changed, and nothing is published as part of this change. After
implementation, reconcile the canonical product specification and remove superseded
creation UI and temporary planning artifacts.
