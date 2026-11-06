#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
node tests/connectors/slack-frontend-check.mjs
