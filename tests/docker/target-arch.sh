#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
scratch=$(mktemp -d /tmp/grengin-docker-arch.XXXXXX)
trap 'rm -rf -- "$scratch"' EXIT

block=$(awk '
  /^RUN (arch=|case )/ && !found { found=1; sub(/^RUN /, ""); print; next }
  found { if ($0 == "") exit; print }
' "$root/Dockerfile")
test -n "$block"
block=${block//\/tmp\/grengin-rust-target/$scratch/target}
printf '%s\n' "$block" | sh -n

stubs='uname() { test "$1" = -m; printf "%s\n" "$NATIVE_ARCH"; }
rustup() { test "$1" = target; test "$2" = add; printf "%s\n" "$3" > "$RUSTUP_RESULT"; }'

run_case() {
  local label=$1 requested=$2 native=$3 expected=$4
  : > "$scratch/result"
  : > "$scratch/target"
  local result=0
  (
    if [[ "$requested" == unset ]]; then unset TARGETARCH; else export TARGETARCH="$requested"; fi
    export NATIVE_ARCH="$native" RUSTUP_RESULT="$scratch/result"
    sh -ec "$stubs
$block"
  ) > "$scratch/output" 2>&1 || result=$?
  if [[ "$expected" == rejected ]]; then
    test "$result" -ne 0
    test ! -s "$scratch/result"
  else
    if [[ "$result" -ne 0 ]]; then cat "$scratch/output"; return 1; fi
    test "$(< "$scratch/result")" == "$expected"
    test "$(< "$scratch/target")" == "$expected"
  fi
  printf 'PASS: %s\n' "$label"
}

run_case 'legacy unset on x86_64' unset x86_64 x86_64-unknown-linux-musl
run_case 'legacy empty on x86_64' '' x86_64 x86_64-unknown-linux-musl
run_case 'legacy unset on aarch64' unset aarch64 aarch64-unknown-linux-musl
run_case 'BuildKit amd64 overrides host detection' amd64 aarch64 x86_64-unknown-linux-musl
run_case 'BuildKit arm64 overrides host detection' arm64 x86_64 aarch64-unknown-linux-musl
run_case 'explicit x86_64 alias' x86_64 aarch64 x86_64-unknown-linux-musl
run_case 'explicit aarch64 alias' aarch64 x86_64 aarch64-unknown-linux-musl
run_case 'unsupported explicit target is not silently replaced' ppc64le x86_64 rejected
run_case 'unsupported native architecture is rejected' unset s390x rejected

test "$(grep -c '^RUN RUST_TARGET="$(cat /tmp/grengin-rust-target)"' "$root/Dockerfile")" -eq 2
printf 'PASS: dependency and application builds reuse the resolved Rust target.\n'
