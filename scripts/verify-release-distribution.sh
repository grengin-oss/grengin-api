#!/usr/bin/env bash
set -euo pipefail
[[ $# == 3 ]] || { printf 'Usage: %s <index.json> <version> <backend-commit>\n' "$0" >&2; exit 2; }
[[ "$2" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$3" =~ ^[a-f0-9]{40}$ ]] || exit 2
jq -e --arg version "$2" --arg revision "$3" '
  .formatVersion == "1.0"
  and .grenginApiVersion == $version
  and .grenginApiCommit == $revision
  and (.catalogCommit | test("^[a-f0-9]{40}$"))
  and (.authProviders.providers | type == "array" and length > 0)
  and (.aiProviders.providers | type == "array" and length > 0)
' "$1" >/dev/null
