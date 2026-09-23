# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import dataclasses
import io
import typing
import unittest
from unittest import mock

import fuchsia_inspect
import main
import statusinfo


class TestSystemStatus(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        super().setUp()
        self.inspect_data = fuchsia_inspect.InspectDataCollection(
            data=[
                fuchsia_inspect.InspectData(
                    moniker="bootstrap/archivist",
                    metadata=fuchsia_inspect.InspectMetadata(
                        timestamp=fuchsia_inspect.Timestamp(int(16e9)),
                    ),
                    version=1,
                    payload=self._make_status_payload("OK", int(8e9)),
                )
            ]
        )

        # This mock simply returns self.inspect_data whenever ffx_cmd.inspect().sync is called.
        mock_command = mock.Mock()

        async def mock_sync(
            *args: typing.Any, **kwargs: typing.Any
        ) -> fuchsia_inspect.InspectDataCollection:
            return self.inspect_data

        mock_command.sync = mock.Mock(side_effect=mock_sync)
        mock_inspect = mock.Mock(
            return_value=mock_command,
        )
        self._patch = mock.patch("ffx_cmd.inspect", mock_inspect)
        self._patch.start()

    def tearDown(self) -> None:
        super().tearDown()
        self._patch.stop()

    def _make_status_payload(
        self, status: str, start_timestamp_nanos: int
    ) -> dict[str, typing.Any]:
        return {
            "root": {
                "fuchsia.inspect.Health": {
                    "start_timestamp_nanos": start_timestamp_nanos,
                    "status": status,
                }
            }
        }

    async def test_command_no_style(self) -> None:
        """test basic invocation of the command without style"""

        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            await main.main(["--no-style"])

        self.assertEqual(
            stdout.getvalue(),
            "\n".join(
                [
                    "bootstrap/archivist",
                    "  Status: OK",
                    "  Uptime: 8.0s\n",
                ]
            ),
        )

    async def test_command_style(self) -> None:
        """test invocation of the command with style"""

        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            await main.main(["--style"])

        self.assertEqual(
            stdout.getvalue(),
            "\n".join(
                [
                    statusinfo.green_highlight("bootstrap/archivist"),
                    f"  Status: {statusinfo.green_highlight('OK')}",
                    "  Uptime: 8.0s\n",
                ]
            ),
        )

    async def test_no_contents(self) -> None:
        """test command when there is no component found"""

        self.inspect_data = fuchsia_inspect.InspectDataCollection(data=[])
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            await main.main(["--no-style"])

        self.assertEqual(stdout.getvalue(), "No component health info found\n")

    async def test_multiple_components(self) -> None:
        """test with multiple components in output"""

        first = self.inspect_data.data[0]
        second = dataclasses.replace(
            first,
            moniker="core/testing",
            payload=self._make_status_payload("NOT HEALTHY", int(7e9)),
        )
        third = dataclasses.replace(
            first,
            moniker="core/other",
            payload=self._make_status_payload("STARTING_UP", int(1e9)),
        )
        self.inspect_data = fuchsia_inspect.InspectDataCollection(
            data=[first, second, third]
        )
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            await main.main(["--style"])

        self.assertEqual(
            stdout.getvalue(),
            "\n".join(
                [
                    statusinfo.green_highlight("bootstrap/archivist"),
                    "  Status: " + statusinfo.green_highlight("OK"),
                    "  Uptime: 8.0s",
                    statusinfo.error_highlight("core/testing"),
                    "  Status: " + statusinfo.error_highlight("NOT HEALTHY"),
                    "  Uptime: 9.0s",
                    statusinfo.warning("core/other"),
                    "  Status: " + statusinfo.warning("STARTING_UP"),
                    "  Uptime: 15.0s\n",
                ]
            ),
        )
