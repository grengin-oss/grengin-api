# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0

from pathlib import Path
import unittest


class ReleasePublicationGateTests(unittest.TestCase):
    def test_main_merges_build_and_test_without_publishing_images(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/docker-publish.yml").read_text()
        gate = "    if: github.event_name == 'workflow_dispatch' || startsWith(github.ref, 'refs/tags/v')"
        for job in ("build-and-push", "publish-manifest"):
            section = workflow.split(f"\n  {job}:\n", 1)[1].split("\n    steps:", 1)[0]
            self.assertIn(gate, section)
        build = workflow.split("\n  build:\n", 1)[1].split("\n  build-and-push:\n", 1)[0]
        self.assertIn("cargo build --locked --workspace", build)
        self.assertIn("cargo test --locked --workspace", build)

    def test_explicit_publication_keeps_both_platforms_and_distribution_checks(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/docker-publish.yml").read_text()
        for required in ("platform: linux/amd64", "platform: linux/arm64",
                         "scripts/verify-release-distribution.sh", "scripts/verify-hotfix-distribution.sh",
                         'test "$GITHUB_REF" = refs/heads/main'):
            self.assertIn(required, workflow)


if __name__ == "__main__":
    unittest.main()
