# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0

from datetime import date
from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "advisory_gate", Path(__file__).resolve().parents[1] / "check-advisory-baseline.py"
)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)
TODAY = date(2026, 11, 26)
FINDING = {
    "kind": "vulnerability", "advisory": "RUSTSEC-2026-0007",
    "package": "bytes", "version": "1.11.0",
    "source": "registry+https://github.com/rust-lang/crates.io-index",
}


def diagnostic(finding=None, severity="error"):
    finding = finding or FINDING
    fields = {
        "code": finding["kind"], "severity": severity,
        "labels": [{"span": " ".join(finding[key] for key in ("package", "version", "source"))}],
    }
    if finding["kind"] != "yanked":
        fields["advisory"] = {"id": finding["advisory"], "package": finding["package"]}
    return {"type": "diagnostic", "fields": fields}


def scan(*diagnostics):
    summary = {"errors": 0, "warnings": 0, "helps": 0, "notes": 0}
    for entry in diagnostics:
        summary[entry["fields"]["severity"] + "s"] += 1
    records = [*diagnostics, {"type": "summary", "fields": {"advisories": summary}}]
    return "\n".join(map(json.dumps, records))


class AdvisoryGateTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.baseline_path = Path(self.directory.name) / "baseline.json"
        self.report_path = Path(self.directory.name) / "report.jsonl"
        self.baseline = {
            "format_version": 1, "recorded_on": "2026-11-26", "expires_on": "2026-12-26",
            "findings": [FINDING],
        }
        self.save_baseline()

    def save_baseline(self):
        self.baseline_path.write_text(json.dumps(self.baseline))

    def run_gate(self, report, code=1):
        result = subprocess.CompletedProcess([], code, stdout="", stderr=report)
        argv = ["gate", "--baseline", str(self.baseline_path), "--report", str(self.report_path)]
        with patch.object(GATE, "date") as clock, patch.object(GATE.subprocess, "run", return_value=result) as runner, \
                patch("sys.argv", argv), patch.dict("os.environ", {"GITHUB_STEP_SUMMARY": ""}), \
                redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            clock.today.return_value = TODAY
            clock.fromisoformat.side_effect = date.fromisoformat
            status = GATE.main()
            self.assertIn("--workspace", runner.call_args.args[0])
            return status

    def test_existing_backlog_reports_but_does_not_block(self):
        report = scan(diagnostic())
        self.assertEqual(self.run_gate(report), 0)
        self.assertEqual(self.report_path.read_text(), "\n" + report)

    def test_new_advisory_package_version_source_or_kind_blocks(self):
        changes = {
            "advisory": "RUSTSEC-2026-9999", "package": "other", "version": "1.11.1",
            "source": "git+https://example.test/fork", "kind": "unsound",
        }
        for key, value in changes.items():
            with self.subTest(key=key):
                new = dict(FINDING, **{key: value})
                self.assertEqual(self.run_gate(scan(diagnostic(), diagnostic(new))), 1)

    def test_resolved_advisories_do_not_have_to_remain(self):
        self.assertEqual(self.run_gate(scan(), code=0), 0)

    def test_yanked_warning_is_also_baselined_and_new_versions_block(self):
        yanked = dict(FINDING, kind="yanked", advisory="yanked")
        self.baseline["findings"] = [yanked]
        self.save_baseline()
        self.assertEqual(self.run_gate(scan(diagnostic(yanked, "warning")), code=0), 0)
        self.assertEqual(self.run_gate(scan(diagnostic(dict(yanked, version="2.0.0"), "warning")), code=0), 1)

    def test_output_on_either_stream_is_parsed(self):
        expected = {GATE.finding_key(FINDING)}
        self.assertEqual(GATE.read_scan(scan(diagnostic()), "", 1), expected)
        self.assertEqual(GATE.read_scan("", scan(diagnostic()), 1), expected)

    def test_non_json_truncated_or_empty_report_fails_closed(self):
        for report in ("", "network error", json.dumps(diagnostic()), scan(diagnostic())[:-1]):
            with self.subTest(report=report[:30]):
                self.assertEqual(self.run_gate(report), 1)

    def test_unknown_warning_or_scanner_error_is_not_ignored(self):
        unknown = diagnostic()
        unknown["fields"]["code"] = "database-fetch-failed"
        self.assertEqual(self.run_gate(scan(unknown)), 1)
        unknown["fields"]["severity"] = "warning"
        self.assertEqual(self.run_gate(scan(unknown), code=0), 1)

    def test_status_must_agree_with_completed_scan(self):
        for report, code in ((scan(diagnostic()), 0), (scan(diagnostic()), 101), (scan(), 1)):
            with self.subTest(code=code):
                self.assertEqual(self.run_gate(report, code), 1)

    def test_incomplete_summary_counts_and_duplicate_summary_fail(self):
        report = scan(diagnostic())
        summary = json.loads(report.splitlines()[-1])
        self.assertEqual(self.run_gate(report + "\n" + json.dumps(summary)), 1)
        summary["fields"]["advisories"]["errors"] = 2
        self.assertEqual(self.run_gate(json.dumps(diagnostic()) + "\n" + json.dumps(summary)), 1)

    def test_missing_or_mismatched_package_identity_fails(self):
        entry = diagnostic()
        entry["fields"]["labels"] = []
        self.assertEqual(self.run_gate(scan(entry)), 1)
        entry = diagnostic()
        entry["fields"]["advisory"]["package"] = "different"
        self.assertEqual(self.run_gate(scan(entry)), 1)

    def test_expired_future_or_overlong_baseline_fails(self):
        for start, end in (("2026-10-27", "2026-11-26"), ("2026-11-27", "2026-12-26"),
                           ("2026-11-26", "2027-01-01")):
            with self.subTest(start=start, end=end):
                self.baseline.update(recorded_on=start, expires_on=end)
                self.save_baseline()
                with self.assertRaises(ValueError):
                    GATE.read_baseline(self.baseline_path, TODAY)

    def test_malformed_or_duplicate_baseline_identity_fails(self):
        self.baseline["findings"] = [FINDING, FINDING]
        self.save_baseline()
        with self.assertRaises(ValueError):
            GATE.read_baseline(self.baseline_path, TODAY)
        for key in GATE.IDENTITY:
            malformed = dict(FINDING)
            del malformed[key]
            with self.assertRaises(ValueError):
                GATE.finding_key(malformed)

    def test_scanner_timeout_or_missing_executable_fails(self):
        argv = ["gate", "--baseline", str(self.baseline_path), "--report", str(self.report_path)]
        for error in (FileNotFoundError("cargo unavailable"), subprocess.TimeoutExpired("cargo", 300)):
            with patch.object(GATE, "read_baseline", return_value=set()), \
                    patch.object(GATE.subprocess, "run", side_effect=error), patch("sys.argv", argv), \
                    redirect_stderr(io.StringIO()):
                self.assertEqual(GATE.main(), 1)


if __name__ == "__main__":
    unittest.main()
