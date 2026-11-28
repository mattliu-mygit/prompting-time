# Plugin contract

What Prompting Time expects of a plugin beyond its manifest, and what a plugin can expect of
Prompting Time. The manifest's format is [`prompting-time-plugin.schema.json`](prompting-time-plugin.schema.json),
which a plugin ships as `prompting-time-plugin.json` at the top of its folder; the output of a
`status` action in format 2 is [`plugin-status-v2.schema.json`](plugin-status-v2.schema.json). The
README's Plugins section describes what the user sees.

## Install

- A release is a `.tar.gz` (or any archive macOS's `tar` reads) holding exactly one folder, such as
  `whooper-0.1.0/`, with the manifest and the files it names. Prompting Time refuses an archive whose
  SHA-256 is not the one the user gives, a download over 256 MiB, and an archive that unpacks to more
  than 1 GiB or 100,000 files, folders, and links.
- An archive install is copied to `<Application Support>/plugins/<name>/`. A folder install, for
  development, runs the folder in place.
- Installing over a plugin waits for the plugin's running actions to end, stops the copy of its
  start command Prompting Time started, then replaces its files; actions called meanwhile wait for
  the new copy.

## Commands

Every command Prompting Time runs, the start command and actions alike:

- runs in the plugin's folder, in a process group of its own, with stdin closed;
- gets Prompting Time's `PATH` (the README's Development prerequisites list it) and
  `XDG_STATE_HOME`, the folder under which the plugin keeps its state;
- gets SIGTERM to its process group when Prompting Time quits, and SIGKILL 5 seconds later.

## State folder

A plugin keeps its state in `$XDG_STATE_HOME/<name>`, by default `~/.local/state/<name>`.
Prompting Time watches that folder and tells the window, and the plugin's page, which files changed;
after a burst of more than 100 files, it says instead that anything may have changed.

## Start command and run lock

- A running start command holds `<state folder>/run.lock/pid`, which holds its process ID in decimal.
- Prompting Time counts the lock as held when that process is alive, its ID is above 1, and its
  command line, as `ps -o command=` prints it, names the start program (the file name of `start[0]`,
  such as `whooper-run`) as a whole word or as a path's last component, as a script's interpreter
  shows it: `/bin/sh /…/bin/whooper-run`. A lock whose process ended, or whose process ID now
  belongs to another program, is stale; the start command should apply the same rule to a lock it
  finds and take a stale one over.
- When Prompting Time opens, installs the plugin, or the user chooses Start, it starts the start
  command unless the lock is held, a needed prerequisite is missing, or `get-settings` fails.
  It does not stop or restart a copy it did not start, such as one started from a terminal.
- The start command's output goes to `<Application Support>/plugins/<name>.log`, which each start
  empties. The Logs tab shows its last 256 KiB.

## Actions

- Prompting Time runs an action's `command`, then the arguments the caller adds, at most 256 KiB of
  them together. It returns the exit code, the first 8 MiB of stdout, saying whether there was more,
  and the last 1 MiB of stderr. An action runs until it exits; nothing times it out, except
  `get-settings` when it decides which prerequisites are needed (120 seconds).
- A refusal is a nonzero exit with the reason on stderr, which the user sees.

### Action names

Some names have a meaning of their own. Each is optional, and none may be marked `confirm`.

| Name | Prompting Time runs it | It should |
| --- | --- | --- |
| `status` | with no arguments, whenever the plugin's files or process change, at most every 2 seconds | print JSON: format 2 when the manifest has a `status` key |
| `round` | for Poll now | run a round of the plugin's work now |
| `pause`, `resume` | for Pause and Resume, offered only when both exist; Resume while `status` says `"paused": true` | pause and resume the plugin's scheduled work |
| `get-settings` | for the Settings tab, and to decide which prerequisites with `only_when` are needed | print the settings as one JSON object |
| `save-settings` | with the settings to change as one JSON object argument, from the Settings tab | check and save them, or refuse with a nonzero exit and the reason on stderr |

The Settings tab can edit settings only when both `get-settings` and `save-settings` exist. A
manifest with a prerequisite's `only_when` must have `get-settings`.

### Row actions and `--item`

An action that the manifest's `status.item_actions` names takes `--item <id>` after its fixed
arguments, naming one item of the `status` output. Prompting Time passes it as two ordinary
arguments, so a confirm action still gets `--print` or `--expect` after them. A blocker's `action`
runs with no extra arguments.

### Confirm actions

An action with `"confirm": true` runs only after the user confirms what it will do:

1. Prompting Time runs it with `--print` last. With `--print` it must change nothing outside this
   Mac, save the request it would run in a file inside the state folder, name that file by its
   absolute path in its output (the path ends at the next whitespace, so it cannot hold a space),
   and print `fingerprint: <hex>`, the file's SHA-256 in lowercase, on its last line. The file must be
   UTF-8 and at most 1 MiB. Prompting Time shows its text in the confirmation.
2. Once the user confirms, Prompting Time runs it again with the same arguments and `--expect <hex>`,
   the fingerprint the user confirmed, as two arguments. The action must refuse to act when the
   request it would run now has another fingerprint, and exit 13 when it does: the confirmation
   then shows the new request and asks again. Any other refusal shows the exit code and stderr.

For a bulk action on several items, Prompting Time previews each item in turn, shows one
confirmation for all of them, and then runs each with its own `--expect`. A later preview may
replace the file an earlier one saved, so the `--expect` check must compare the request the action
would run now, not the saved file.

Prompting Time adds `--print` and `--expect` itself and refuses a request whose arguments hold
`--print`, `--expect`, `--print=…`, or `--expect=…`, from a page, a row's item ID, or anything
else. The action should accept those flags only spelled out in full, not abbreviated, so no other
argument can stand for them.

## Page

The manifest's `page.entry` is an HTML file in a folder of its own, such as `page/dist/index.html`.
Prompting Time serves the files of that folder, each up to 64 MiB, at `pt-plugin://<name>/…` to a
frame sandboxed to scripts only, under a Content Security Policy that allows the page's own files,
inline styles, and data URLs, and no fetches. HTML files load only at the address Prompting Time
gives the frame, which carries a token in its query (`?pt=…`), so a page can reload itself but
cannot load another plugin's page, and another of its own HTML files loads only with the same
query; build the page as one HTML file with relative asset paths (Vite's `base: "./"`). The page speaks page protocol 2, which the README's Page messages describe:
each call holds at most 64 arguments of at most 64 KiB together, at most 16 of a page's calls run at
once, and a call whose output passes 8 MiB gets an error instead of a cut-off part.
