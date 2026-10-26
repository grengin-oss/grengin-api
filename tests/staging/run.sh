#!/usr/bin/env bash
set -euo pipefail
set +x
cd "$(dirname "$0")/../.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target-security}"
report="${GRENGIN_STAGING_REPORT_DIR:-$(mktemp -d /tmp/grengin-staging-validation-XXXXXX)}"
mkdir -p "$report"
run() {
  local name="$1"; shift
  printf '\n== %s ==\n' "$name"
  "$@" 2>&1 | tee "$report/$name.log"
}
printf 'Reports: %s\n' "$report"
git rev-parse HEAD > "$report/backend-commit.txt"
run format cargo fmt --all --check
run distribution-gate bash tests/staging/test-distribution-gate.sh
run workspace cargo test --locked --workspace --jobs 2
run build cargo build --locked --workspace --jobs 2
run clippy cargo clippy --locked --workspace --all-targets --jobs 2
run oidc bash tests/auth/mock_oauth2.sh
run social-oidc bash tests/auth/mock_social_oauth.sh
export GRENGIN_EXTENSIVE_TESTS=1 GRENGIN_SHARE_CHAT_TESTS=1
run live bash tests/security/run.sh --live-only
run diff git diff --check
printf '\nCombined single-instance staging gates passed. Reports: %s\n' "$report"
