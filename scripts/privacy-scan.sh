#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(git -C "$script_directory" rev-parse --show-toplevel 2>/dev/null)" || {
  echo "privacy scan: run this script inside a Git repository" >&2
  exit 2
}
repo_root="$(realpath "$repo_root")"
cd "$repo_root"

failed=0

fail_category() {
  echo "privacy scan: rejected tracked content ($1)" >&2
  failed=1
}

scan_regex() {
  local label="$1"
  local expression="$2"
  if git grep -I -q -E -e "$expression" -- .; then
    fail_category "$label"
  fi
}

users_home_regex='/''Users/[^/[:space:]]+(/|$)'
unix_home_regex='/''home/[^/[:space:]]+(/|$)'
scan_regex "private absolute home path" "$users_home_regex"
scan_regex "private absolute home path" "$unix_home_regex"
scan_regex "AWS access key" 'AKIA[0-9A-Z]{16}'
scan_regex "GitHub credential" 'gh[pousr]_[A-Za-z0-9]{20,}'
scan_regex "OpenAI credential" 'sk-[A-Za-z0-9_-]{20,}'
scan_regex "Anthropic credential" 'sk-ant-[A-Za-z0-9_-]{20,}'
scan_regex "private key material" '-----BEGIN ([A-Z0-9 ]+ )?PRIVATE KEY-----'
scan_regex "provider authentication state" '(^|/)[.]codex/(auth[.]json|sessions?/|state/)'
scan_regex "provider authentication state" '(^|/)[.]claude/(credentials|projects?/|sessions?/|state/)'

while IFS= read -r -d '' tracked_path; do
  case "$tracked_path" in
    crates/prompting-time-core/tests/fixtures/codex/session.jsonl|crates/prompting-time-core/tests/fixtures/codex/unknown_notification.jsonl)
      continue
      ;;
  esac
  case "$tracked_path" in
    .claude.json|*/.claude.json|.claude.json.backup|*/.claude.json.backup)
      fail_category "provider runtime file"
      ;;
  esac
  case "/$tracked_path" in
    */.codex/*|*/.claude/*)
      fail_category "provider runtime directory"
      ;;
  esac
  case "$tracked_path" in
    *.sqlite|*.sqlite3|*.sqlite-shm|*.sqlite-wal|*.db|*.db-shm|*.db-wal|*.jsonl|*.pem|*.key|*.p12|*.pfx)
      fail_category "runtime or credential artifact"
      ;;
  esac
done < <(git ls-files -z)

if [[ -n "${PROMPTING_TIME_PRIVATE_TERMS_FILE:-}" ]]; then
  denylist="$PROMPTING_TIME_PRIVATE_TERMS_FILE"
  if [[ ! -f "$denylist" ]]; then
    echo "privacy scan: private denylist is not a readable file" >&2
    exit 2
  fi
  denylist_path="$(realpath "$denylist")"
  case "$denylist_path" in
    "$repo_root"|"$repo_root"/*)
      echo "privacy scan: private denylist must be outside the repository" >&2
      exit 2
      ;;
  esac

  while IFS= read -r private_term || [[ -n "$private_term" ]]; do
    [[ -z "$private_term" ]] && continue
    if git grep -I -q -F -e "$private_term" -- .; then
      fail_category "private denylist term"
    fi
  done < "$denylist_path"
fi

if (( failed != 0 )); then
  exit 1
fi

echo "privacy scan: tracked files passed"
