#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Simple FFX host tool E2E test."""

import json
import logging
import re
from typing import Any, List, Optional

import ffxtestcase
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


def remove_ansi_escape_sequences(text: str) -> str:
    """
    Removes ANSI escape sequences from a string.
    """
    ansi_escape = re.compile(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")
    return ansi_escape.sub("", text)


class FfxDirectTest(ffxtestcase.FfxTestCase):
    """FFX host tool E2E test for Direct connections."""

    async def setup_class(self) -> None:
        # This just gets some things out of the way in case the daemon gets
        # turned on and off again.
        await super().setup_class()
        self.isolate_dir = self.dut.ffx.config.isolate_dir.directory()
        self.dut_ssh_address = self.dut.ffx.get_target_ssh_address()
        assert self.dut_ssh_address is not None

    # Run the ffx command with the --direct arg, and parse the results
    def _run_ffx_direct(
        self, cmd: List[str], target: Optional[str] = None
    ) -> Any:
        all_args = [
            "--isolate-dir",
            self.isolate_dir,
            "--direct",
            "--machine",
            "json",
        ]
        if target is not None:
            all_args += ["-t", target]
        all_args += cmd
        return json.loads(self.run_ffx(all_args))

    def test_direct_target_echo(self) -> None:
        """Test `ffx --direct target echo` works."""
        out = self._run_ffx_direct(
            [
                "target",
                "echo",
                "From a Test",
            ],
            f"{self.dut_ssh_address}",
        )

        asserts.assert_equal(out["message"], "From a Test")

    def test_direct_target_list(self) -> None:
        """Test `ffx --direct target list` works."""
        out = self._run_ffx_direct(
            [
                "target",
                "list",
                f"{self.dut_ssh_address}",
            ],
        )
        asserts.assert_equal(out[0]["target_state"], "Product")
        asserts.assert_equal(out[0]["rcs_state"], "Y")

    def test_direct_version(self) -> None:
        """Test `ffx --direct version -v` does not query the daemon."""
        out = self._run_ffx_direct(
            [
                "version",
                "-v",
            ],
        )
        asserts.assert_is_none(out["daemon_version"])
        asserts.assert_is_not_none(out["tool_version"])

    # This tests that an arbitrary tool using RCS, that was not specifically
    # modified to support direct mode, does on fact work in direct mode.
    def test_component_list(self) -> None:
        """Test `ffx --direct component list` works."""
        out = self._run_ffx_direct(
            [
                "component",
                "list",
            ],
            f"{self.dut_ssh_address}",
        )
        asserts.assert_greater(len(out["instances"]), 0)

    # This tests that a tool that uses non-RCS proxies works in direct mode.
    def test_repo_list(self) -> None:
        """Test `ffx --direct target repository list` works."""
        out = self._run_ffx_direct(
            [
                "target",
                "repository",
                "list",
            ],
            f"{self.dut_ssh_address}",
        )
        asserts.assert_true(
            "data" in out["ok"], "result does not contain ok[data]"
        )

    # This tests that a tool that uses a non-standard target connection flow
    # works in direct mode.
    def test_ffx_log(self) -> None:
        """Test `ffx --direct log` works."""
        # Can't run with _run_ffx_direct() because the output is not
        # valid JSON; instead each line is a JSON object.
        out = self.run_ffx(
            [
                "--isolate-dir",
                self.isolate_dir,
                "--direct",
                "--machine",
                "json",
                "-t",
                f"{self.dut_ssh_address}",
                "log",
                "--symbolize",
                "off",
                "--since-boot",
                "1",
                "--until-boot",
                "2",
            ],
        )
        # We can't actually guarantee that we get valid JSON from `ffx log`,
        # but we can note when we don't.
        try:
            logs = [json.loads(l) for l in out.strip().split("\n")]
            asserts.assert_greater(len(logs), 0)
        except json.decoder.JSONDecodeError:
            _LOGGER.info(f"Got bad JSON from ffx-log: {repr(out[:100])}")

    def test_ffx_doctor(self) -> None:
        """Test `ffx --direct doctor` works."""
        # Can't run with _run_ffx_direct() because `ffx doctor` does
        # not support JSON output.
        out = self.run_ffx(
            [
                "--isolate-dir",
                self.isolate_dir,
                "--direct",
                "doctor",
            ],
        )
        # Remove colors from doctor output
        out = remove_ansi_escape_sequences(out)

        # Make sure we actually find a target
        asserts.assert_regex(out, r"\[✓\] \d+ targets found")


if __name__ == "__main__":
    test_runner.main()
