#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
scratch=$(mktemp -d /tmp/grengin-distribution-gate-XXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
revision=1111111111111111111111111111111111111111
jq -n --arg revision "$revision" '{
  formatVersion:"1.0",grenginApiVersion:"0.9.5",grenginApiCommit:$revision,
  catalogCommit:$revision,authProviders:{providers:[{id:"google"}]},
  aiProviders:{providers:[{id:"openai"}]}
}' > "$scratch/valid.json"
bash "$root/scripts/verify-release-distribution.sh" "$scratch/valid.json" 0.9.5 "$revision"
printf 'PASS: matching published catalog is accepted\n'
for mutation in \
  '.grenginApiVersion="0.9.4"' \
  '.grenginApiCommit="2222222222222222222222222222222222222222"' \
  'del(.grenginApiCommit)' \
  '.formatVersion="2.0"' \
  '.catalogCommit="invalid"' \
  '.authProviders.providers=[]' \
  '.aiProviders.providers=[]'; do
  jq "$mutation" "$scratch/valid.json" > "$scratch/invalid.json"
  if bash "$root/scripts/verify-release-distribution.sh" "$scratch/invalid.json" 0.9.5 "$revision" 2>/dev/null; then
    printf 'FAIL: invalid catalog accepted: %s\n' "$mutation" >&2
    exit 1
  fi
  printf 'PASS: rejected %s\n' "$mutation"
done
if bash "$root/scripts/verify-release-distribution.sh" "$scratch/missing.json" 0.9.5 "$revision" 2>/dev/null; then
  printf 'FAIL: missing catalog accepted\n' >&2; exit 1
fi
printf 'PASS: missing catalog is rejected\n'
