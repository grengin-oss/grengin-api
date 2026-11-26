#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0

import argparse
from collections import Counter
from datetime import date
import json
import os
from pathlib import Path
import subprocess
import sys


KINDS = {"vulnerability", "unmaintained", "unsound", "yanked"}
IDENTITY = ("kind", "advisory", "package", "version", "source")
ROOT = Path(__file__).resolve().parents[1]


def finding_key(finding):
    if not isinstance(finding, dict) or set(finding) != set(IDENTITY):
        raise ValueError("invalid advisory identity")
    if any(not isinstance(finding[key], str) or not finding[key] for key in IDENTITY):
        raise ValueError("advisory identity fields must be nonempty strings")
    if finding["kind"] not in KINDS:
        raise ValueError("unknown advisory kind")
    if finding["kind"] == "yanked":
        if finding["advisory"] != "yanked":
            raise ValueError("invalid yanked identity")
    elif not finding["advisory"].startswith("RUSTSEC-"):
        raise ValueError("missing RustSec advisory ID")
    return tuple(finding[key] for key in IDENTITY)


def read_baseline(path, today):
    baseline = json.loads(path.read_text())
    if baseline.get("format_version") != 1:
        raise ValueError("unsupported advisory baseline format")
    reviewed = date.fromisoformat(baseline["recorded_on"])
    expires = date.fromisoformat(baseline["expires_on"])
    if not reviewed <= today < expires or not 0 < (expires - reviewed).days <= 30:
        raise ValueError("advisory baseline expired, future-dated, or longer than 30 days")
    findings = baseline["findings"]
    if not isinstance(findings, list):
        raise ValueError("baseline findings must be a list")
    keys = {finding_key(finding) for finding in findings}
    if len(keys) != len(findings):
        raise ValueError("duplicate advisory baseline identity")
    return keys


def read_scan(stdout, stderr, returncode):
    findings = set()
    counts = Counter()
    summary = None
    for output in (stdout, stderr):
        for line in output.splitlines():
            if not line.strip():
                continue
            record = json.loads(line)
            fields = record["fields"]
            if record["type"] == "summary":
                if summary is not None or set(fields) != {"advisories"}:
                    raise ValueError("unexpected scanner summary")
                summary = fields["advisories"]
                continue
            if record["type"] != "diagnostic" or fields.get("code") not in KINDS:
                raise ValueError("unexpected scanner diagnostic; inspect the report")
            severity = fields["severity"]
            if severity not in {"error", "warning"}:
                raise ValueError("unexpected advisory severity")
            counts[severity + "s"] += 1
            kind = fields["code"]
            advisory = fields.get("advisory", {})
            advisory_id = "yanked" if kind == "yanked" else advisory["id"]
            labels = fields["labels"]
            if not labels:
                raise ValueError("advisory has no package identity")
            for label in labels:
                package, version, source = label["span"].split(maxsplit=2)
                if kind != "yanked" and package != advisory["package"]:
                    raise ValueError("advisory package identity mismatch")
                findings.add(finding_key(dict(zip(
                    IDENTITY, (kind, advisory_id, package, version, source)
                ))))
    if not isinstance(summary, dict) or set(summary) != {"errors", "warnings", "notes", "helps"}:
        raise ValueError("scanner did not finish with a valid summary")
    if any(type(value) is not int or value < 0 for value in summary.values()):
        raise ValueError("invalid scanner counts")
    expected = {"errors": counts["errors"], "warnings": counts["warnings"], "notes": 0, "helps": 0}
    if summary != expected or returncode != (1 if counts["errors"] else 0):
        raise ValueError("scanner failed or returned an incomplete report")
    return findings


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, default=ROOT / ".github/advisory-baseline.json")
    parser.add_argument("--report", type=Path, default=ROOT / "target/advisory-report.jsonl")
    args = parser.parse_args()
    try:
        baseline = read_baseline(args.baseline, date.today())
        result = subprocess.run(
            ["cargo", "deny", "--workspace", "--format", "json", "check", "advisories", "--hide-inclusion-graph"],
            cwd=ROOT, capture_output=True, text=True, timeout=300, check=False,
        )
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(result.stdout + "\n" + result.stderr)
        findings = read_scan(result.stdout, result.stderr, result.returncode)
        new = findings - baseline
        message = (
            f"Advisories: {len(findings & baseline)} existing finding(s) reported under the "
            f"temporary baseline; {len(new)} new finding(s). This is not a clean security audit."
        )
        print(message)
        for finding in sorted(new):
            print("NEW: " + " | ".join(finding))
        if summary_path := os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(summary_path, "a") as summary:
                summary.write(message + "\n\nFull findings: advisory-report artifact. "
                              "Remediation: docs/security-advisory-baseline.md\n")
        return int(bool(new))
    except (OSError, ValueError, KeyError, TypeError, AttributeError, subprocess.TimeoutExpired) as error:
        print(f"Advisory gate failed closed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
