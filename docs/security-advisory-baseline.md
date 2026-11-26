# Temporary dependency-advisory backlog

Recorded: 2026-11-26. Expires: 2026-12-26. Owner: backend maintainers.

The current lockfile has 46 RustSec findings (40 vulnerabilities, four
unmaintained crates, two unsoundness findings) and two yanked versions.
They are tracked exactly in [.github/advisory-baseline.json](../.github/advisory-baseline.json).
This snapshot is not a reachability assessment, a finding dismissal, or a
security sign-off. The findings still need remediation before a secure-release claim.

CI reports this backlog without blocking unrelated fixes for at most 30 days.
It blocks any finding not in the snapshot, including the same advisory against
a different package version or source. Scanner errors, malformed/incomplete
reports, and an expired baseline fail closed. License enforcement is unchanged.
Reports are retained as the `advisory-report` CI artifact for seven days.

## Remediation tracking

- [ ] Upgrade network-facing dependencies first: rmcp, h2, rustls,
  rustls-webpki, webpki-roots, openssl, bytes and url; test stream termination,
  MCP cancellation, TLS and outbound-address restrictions.
- [ ] Upgrade untrusted-document/archive parsers: calamine and zip; test extraction
  limits, malformed inputs and path confinement. Check advisory-specific opt-ins;
  upgrading alone does not address every zip extraction finding.
- [ ] Address crypto/runtime findings in crypto-bigint, rsa, time, tokio and anyhow;
  assess upstream fixes or mitigations where no fixed version is available.
- [ ] Replace/upgrade unmaintained dependencies and yanked chacha20/spin versions.
- [ ] Rerun the full workspace and database regressions, prune resolved entries,
  and remove the temporary baseline when the backlog is cleared.

Run `python3 scripts/check-advisory-baseline.py` from a checkout with
`cargo-deny 0.20.2` installed. It writes the full scanner output to
`target/advisory-report.jsonl`. No findings are added to the baseline automatically.
Any baseline edit requires review of the exact new advisory/package/version/source;
do not roll its expiry forward just to make CI green. If remediation is incomplete
on the deadline, CI blocks until maintainers explicitly review the remaining risk.
