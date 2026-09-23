#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Simple FFX host tool E2E test."""

import json
import logging

import ffxtestcase
from honeydew.transports.ffx.types import MachineFormat
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class FfxTest(ffxtestcase.FfxTestCase):
    """FFX host tool E2E test."""

    async def setup_class(self) -> None:
        await super().setup_class()
        self.isolate_dir = self.dut.ffx.config.isolate_dir.directory()

    def test_component_list(self) -> None:
        """Test `ffx component list` output returns as expected."""
        output = self.dut.ffx.run(["component", "list"])
        asserts.assert_true(
            len(output.splitlines()) > 0,
            f"stdout is unexpectedly empty: {output}",
        )

    def test_target_list_includes_port(self) -> None:
        """Test `ffx target list` output returns as expected."""
        # NOTE: This test fails if the device under test is an user-mode networking emulator.
        output = self.dut.ffx.run(
            ["target", "list", "--format", "a"], machine=MachineFormat.RAW
        )
        asserts.assert_true(
            ":22" in output, f"expected stdout to contain ':22',got {output}"
        )

    def test_target_show(self) -> None:
        """Test `ffx target show` output returns as expected."""
        output = self.dut.ffx.get_target_information()
        got_device_name = output.target.name
        # Assert FFX's target show device name matches Honeydew's.
        asserts.assert_equal(got_device_name, self.dut.device_name)

    # TODO(b/493680962): re-enable this test when we have a mechanism for disconnecting
    # from RCS.
    # async def test_target_echo_repeat(self) -> None:
    #     """Test `ffx target echo --repeat` is resilient to target disconnection."""
    #     with self.dut.ffx.popen(
    #         ["target", "echo", "--repeat"],
    #         stdout=subprocess.PIPE,
    #         text=False,
    #     ) as process:
    #         try:
    #             line = process.stdout.readline()
    #             asserts.assert_true(
    #                 line.startswith(b"SUCCESS"),
    #                 f"First ping didn't succeed: {line}",
    #             )
    #             # XXX somehow disconnect from RCS.
    #             # We used to stop the daemon, but the daemon has been removed.
    #             # We could reboot the device, but that doesn't
    #             # play well with Infra. Maybe a Fuchsia Controller request of
    #             # some sort?
    #             while True:
    #                 line = process.stdout.readline()
    #                 if not line.startswith(b"ERROR") and not line.startswith(
    #                     b"Waiting for"
    #                 ):
    #                     break
    #                 _LOGGER.debug(f"echo output: {line}")
    #             asserts.assert_true(
    #                 line.startswith(b"SUCCESS"),
    #                 f"Success didn't resume after error: {line}",
    #             )
    #         finally:
    #             process.kill()

    def test_machine_errors(self) -> None:
        """Test machine formattable errors."""
        cmd = [
            "--machine",
            "json",
            "-t",
            "this-should-not-exist",
            "-c",
            "proxy.timeout_secs=5",
            "target",
            "show",
        ]
        (code, stdout, stderr) = self.run_ffx_unchecked(cmd)
        output_json = json.loads(stdout)
        asserts.assert_equal(stderr, "")
        asserts.assert_equal(output_json["type"], "user")
        asserts.assert_equal(
            output_json["message"],
            (
                'non-fatal error encountered: Target specification "this-should-not-exist" was not found. '
                "Use `ffx target list` to list known targets, and use a different target query."
            ),
        )
        asserts.assert_equal(output_json["code"], 1)

    def test_machine_user_error(self) -> None:
        """Test machine formattable errors for a user error kind."""
        cmd = [
            "--machine",
            "json",
            "repository",
            "server",
            "start",
            "--background",
            "--foreground",
        ]
        (code, stdout, stderr) = self.run_ffx_unchecked(cmd)
        try:
            output_json = json.loads(stdout)
        except json.JSONDecodeError as e:
            raise ValueError(f"could not parse string as JSON: {e}.  {stdout}")

        # This is an expected message on stderr. The log file is changed for package servers.
        asserts.assert_in("Switching log file to", stderr)
        asserts.assert_equal(output_json["type"], "user")
        asserts.assert_equal(
            output_json["message"],
            "Mutually exclusive arguments: --background is mutually exclusive with --foreground and --disconnected",
        )
        asserts.assert_equal(output_json["code"], 1)

    def test_machine_config_error(self) -> None:
        """Test machine formattable errors for a user error kind."""
        cmd = [
            "--machine",
            "json",
            "-t",
            "foo",
            "-t",
            "bar",
            "target",
            "show",
        ]
        (code, stdout, stderr) = self.run_ffx_unchecked(cmd)
        output_json = json.loads(stdout)
        asserts.assert_equal(stderr, "")
        asserts.assert_equal(output_json["type"], "config")
        asserts.assert_equal(
            output_json["message"],
            "Error parsing option '-t' with value 'bar': duplicate values provided\n",
        )
        asserts.assert_equal(output_json["code"], 1)

    def test_arg_parse_error_formats(self) -> None:
        """Test machine formattable errors for a user error kind."""
        cmd = [
            "-t",
            "foo",
            "-t",
            "bar",
            "target",
            "show",
        ]
        (code, stdout, stderr) = self.run_ffx_unchecked(cmd)
        output_json = json.loads(stdout)
        asserts.assert_equal(stderr, "")
        asserts.assert_equal(output_json["type"], "config")
        asserts.assert_equal(
            output_json["message"],
            "Error parsing option '-t' with value 'bar': duplicate values provided\n",
        )
        asserts.assert_equal(output_json["code"], 1)

    def test_machine_unexpected_error(self) -> None:
        """Test machine formattable errors for a user error kind."""
        cmd = [
            "--machine",
            "json",
            "-t",
            "foo,bar",
            "target",
            "show",
        ]
        (code, stdout, stderr) = self.run_ffx_unchecked(cmd)
        output_json = json.loads(stdout)
        asserts.assert_equal(stderr, "")
        asserts.assert_equal(output_json["type"], "unexpected")
        asserts.assert_equal(
            output_json["message"],
            "--config must either be a file path, a valid JSON object, or comma separated key=value pairs.",
        )
        asserts.assert_equal(output_json["code"], 1)

    def test_machine_help(self) -> None:
        """Test machine formattable help."""
        cmd = [
            "--machine",
            "json",
            "--help",
        ]
        (code, stdout, stderr) = self.run_ffx_unchecked(cmd)
        json.loads(stdout)
        asserts.assert_equal(stderr, "")
        asserts.assert_equal(code, 0)

    def test_shared_data(self) -> None:
        """Test `ffx -c shared_dir=<dir>` will use the value passed in for $SHARED_DATA"""
        (_code, stdout, _stderr) = self.run_ffx_unchecked(
            ["-c", "shared_data=foo", "config", "get", "monitor.pid_file"]
        )
        asserts.assert_true(
            "foo/monitor" in stdout,
            "Expected SHARED_DATA to be correctly set",
            stdout,
        )


if __name__ == "__main__":
    test_runner.main()
