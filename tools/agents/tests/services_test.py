# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for daemon service discovery and management."""

from __future__ import annotations

import unittest
from unittest import mock

from agents.lib import services
from agents_testing.base import BaseTestCase


class ServicesTest(BaseTestCase):
    """Hermetic unit tests for daemon service discovery and restart."""

    def test_find_daemon_services(self) -> None:
        self.write_file(
            ".agents/config/services.txt", "service_a\nservice_b\n# comment\n"
        )
        self.write_file(
            "vendor/google/.agents/config/services.txt",
            "service_c\nservice_a\n",
        )

        found = services.find_daemon_services(self.test_dir)
        self.assertEqual(found, ["service_a", "service_b", "service_c"])

    def test_restart_daemons_dry_run(self) -> None:
        with mock.patch("shutil.which", return_value="/bin/systemctl"):
            with mock.patch("subprocess.run") as mock_run:
                services.restart_daemons(["foo-service"], dry_run=True)
                mock_run.assert_not_called()
                self.assertIn(
                    "Would run: systemctl --user try-restart foo-service",
                    self.stdout,
                )

    def test_restart_daemons_actual(self) -> None:
        with mock.patch("shutil.which", return_value="/bin/systemctl"):
            with mock.patch("subprocess.run") as mock_run:
                services.restart_daemons(["foo-service"], dry_run=False)
                mock_run.assert_called_once_with(
                    ["systemctl", "--user", "try-restart", "foo-service"],
                    capture_output=True,
                    check=False,
                )

    def test_restart_daemons_no_systemctl(self) -> None:
        with mock.patch("shutil.which", return_value=None):
            with mock.patch("subprocess.run") as mock_run:
                services.restart_daemons(["service_a"])
                mock_run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
