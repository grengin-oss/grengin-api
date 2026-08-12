#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

cargo test -p llm-plugin -j 2
cargo test -p grengin-api -j 2
cargo clippy -p llm-plugin --all-targets -j 2 -- -D warnings

if [[ "${1:-}" == "--docker" ]]; then
  docker build \
    --progress=plain \
    --build-arg TARGETARCH=amd64 \
    -t grengin-api:llm-plugin-test \
    .
fi
