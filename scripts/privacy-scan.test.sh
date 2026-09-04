#!/usr/bin/env bash
set -euo pipefail

scanner="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/privacy-scan.sh"
fixture_root="$(mktemp -d)"
trap 'rm -rf -- "$fixture_root"' EXIT

repo="$fixture_root/repository"
mkdir -p "$repo/scripts"
cp "$scanner" "$repo/scripts/privacy-scan.sh"
git -C "$repo" init -q
git -C "$repo" config user.name "Prompting Time Test"
git -C "$repo" config user.email "prompting-time@example.test"
printf '%s\n' 'safe fixture' > "$repo/README.md"
git -C "$repo" add README.md scripts/privacy-scan.sh
git -C "$repo" commit -qm "safe"

bash "$repo/scripts/privacy-scan.sh"

scanner_secret="s""k-abcdefghijklmnopqrstuvwxyz"
printf '\n# %s\n' "$scanner_secret" >> "$repo/scripts/privacy-scan.sh"
git -C "$repo" add scripts/privacy-scan.sh
if output="$(bash "$repo/scripts/privacy-scan.sh" 2>&1)"; then
  echo "privacy scanner accepted a credential in its own tracked script" >&2
  exit 1
fi
if printf '%s' "$output" | grep -Fq "$scanner_secret"; then
  echo "privacy scanner printed credential content from its own script" >&2
  exit 1
fi
cp "$scanner" "$repo/scripts/privacy-scan.sh"
git -C "$repo" add scripts/privacy-scan.sh

private_path="/""Users/example/private-project"
printf '%s\n' "$private_path" > "$repo/forbidden.txt"
git -C "$repo" add forbidden.txt
if output="$(bash "$repo/scripts/privacy-scan.sh" 2>&1)"; then
  echo "privacy scanner accepted a tracked private home path" >&2
  exit 1
fi
if printf '%s' "$output" | grep -Fq "$private_path"; then
  echo "privacy scanner printed private matched content" >&2
  exit 1
fi

git -C "$repo" rm -qf forbidden.txt
bash "$repo/scripts/privacy-scan.sh"

private_home="/""Users/privateuser"
printf '%s\n' "$private_home" > "$repo/forbidden.txt"
git -C "$repo" add forbidden.txt
if output="$(bash "$repo/scripts/privacy-scan.sh" 2>&1)"; then
  echo "privacy scanner accepted a private home directory without a trailing slash" >&2
  exit 1
fi
if printf '%s' "$output" | grep -Fq "$private_home"; then
  echo "privacy scanner printed a private home directory" >&2
  exit 1
fi
git -C "$repo" rm -qf forbidden.txt

printf 'untracked %s\n' "$private_path" > "$repo/untracked.txt"
bash "$repo/scripts/privacy-scan.sh"

denylist="$fixture_root/private-terms.txt"
printf '%s\n' 'fixture-private-literal' > "$denylist"
printf '%s\n' 'fixture-private-literal' > "$repo/private.txt"
git -C "$repo" add private.txt
if output="$(PROMPTING_TIME_PRIVATE_TERMS_FILE="$denylist" bash "$repo/scripts/privacy-scan.sh" 2>&1)"; then
  echo "privacy scanner accepted a tracked private deny term" >&2
  exit 1
fi
if printf '%s' "$output" | grep -Fq 'fixture-private-literal'; then
  echo "privacy scanner printed a private deny term" >&2
  exit 1
fi

if PROMPTING_TIME_PRIVATE_TERMS_FILE="$repo/private.txt" bash "$repo/scripts/privacy-scan.sh" >/dev/null 2>&1; then
  echo "privacy scanner accepted an in-repository denylist" >&2
  exit 1
fi

git -C "$repo" rm -qf private.txt
printf '%s\n' 'sqlite runtime' > "$repo/state.sqlite3"
git -C "$repo" add state.sqlite3
if bash "$repo/scripts/privacy-scan.sh" >/dev/null 2>&1; then
  echo "privacy scanner accepted a tracked runtime artifact" >&2
  exit 1
fi
git -C "$repo" rm -qf state.sqlite3

claude_state="top-level Claude state fixture"
printf '%s\n' "$claude_state" > "$repo/.claude.json"
git -C "$repo" add .claude.json
claude_failures=0
if output="$(bash "$repo/scripts/privacy-scan.sh" 2>&1)"; then
  echo "privacy scanner accepted a tracked top-level .claude.json" >&2
  claude_failures=1
elif printf '%s' "$output" | grep -Fq "$claude_state"; then
  echo "privacy scanner printed top-level Claude state content" >&2
  claude_failures=1
fi
git -C "$repo" rm -qf .claude.json

claude_backup="top-level Claude backup fixture"
printf '%s\n' "$claude_backup" > "$repo/.claude.json.backup"
git -C "$repo" add .claude.json.backup
if output="$(bash "$repo/scripts/privacy-scan.sh" 2>&1)"; then
  echo "privacy scanner accepted a tracked top-level .claude.json.backup" >&2
  claude_failures=1
elif printf '%s' "$output" | grep -Fq "$claude_backup"; then
  echo "privacy scanner printed top-level Claude backup content" >&2
  claude_failures=1
fi
git -C "$repo" rm -qf .claude.json.backup

mkdir -p "$repo/archive/session"
nested_claude_state="nested Claude state fixture"
printf '%s\n' "$nested_claude_state" > "$repo/archive/session/.claude.json"
git -C "$repo" add -f archive/session/.claude.json
if output="$(bash "$repo/scripts/privacy-scan.sh" 2>&1)"; then
  echo "privacy scanner accepted a tracked nested .claude.json" >&2
  claude_failures=1
elif printf '%s' "$output" | grep -Fq "$nested_claude_state"; then
  echo "privacy scanner printed nested Claude state content" >&2
  claude_failures=1
fi
git -C "$repo" rm -qf archive/session/.claude.json

nested_claude_backup="nested Claude backup fixture"
mkdir -p "$repo/archive/session"
printf '%s\n' "$nested_claude_backup" > "$repo/archive/session/.claude.json.backup"
git -C "$repo" add -f archive/session/.claude.json.backup
if output="$(bash "$repo/scripts/privacy-scan.sh" 2>&1)"; then
  echo "privacy scanner accepted a tracked nested .claude.json.backup" >&2
  claude_failures=1
elif printf '%s' "$output" | grep -Fq "$nested_claude_backup"; then
  echo "privacy scanner printed nested Claude backup content" >&2
  claude_failures=1
fi
if (( claude_failures != 0 )); then
  exit 1
fi

echo "privacy scanner tests passed"
