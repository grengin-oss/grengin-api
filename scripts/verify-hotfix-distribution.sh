#!/usr/bin/env bash
set -euo pipefail
[[ $# == 4 ]] || {
  printf 'Usage: %s <index.json> <version> <original-revision> <hotfix-revision>\n' "$0" >&2; exit 2;
}
root=$(cd "$(dirname "$0")/.." && pwd)
bash "$root/scripts/verify-release-distribution.sh" "$1" "$2" "$3"
[[ "$4" =~ ^[a-f0-9]{40}$ ]]
git -C "$root" merge-base --is-ancestor "$3" "$4"
jq -e '
  [.authProviders.providers[], .aiProviders.providers[]]
  | all(.[]; (.id | test("^[a-z][a-z0-9-]{0,62}$"))
      and (.versions | type == "array" and length > 0)
      and all(.versions[];
        (.version | test("^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(\\.(0|[1-9][0-9]*))?$"))
        and (.sha256 | test("^[a-f0-9]{64}$"))))
' "$1" >/dev/null
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
mkdir -p "$scratch/distributions/grengin-api/$2"
cp "$1" "$scratch/distributions/grengin-api/$2/index.json"
while IFS=$'\t' read -r path digest; do
  mkdir -p "$(dirname "$scratch/$path")"
  curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
    --retry 3 --max-time 30 "https://meta.grengin.com/$path" --output "$scratch/$path"
  printf '%s  %s\n' "$digest" "$scratch/$path" | sha256sum --check --status
done < <(jq -r '
  (.authProviders.providers[] as $provider | $provider.versions[]
    | ["auth-providers/\($provider.id)/versions/\(.version)/provider.json", .sha256]),
  (.aiProviders.providers[] as $provider | $provider.versions[]
    | ["ai-providers/\($provider.id)/versions/\(.version)/plugin.json", .sha256])
  | @tsv
' "$1")
GRENGIN_PROVIDER_CATALOG_ROOT="$scratch" cargo test \
  --manifest-path "$root/Cargo.toml" --locked --jobs 2 \
  services::discovery_catalog::tests::every_distribution_package_matches_the_runtime_contract \
  -- --ignored --exact
