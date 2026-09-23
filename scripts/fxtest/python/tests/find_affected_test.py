# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from unittest import mock

import find_affected


class TestFindAffected(unittest.IsolatedAsyncioTestCase):
    def test_format_affected_targets(self) -> None:
        """Tests the label aggregation and mapping algorithm to execution commands."""
        input_dict = {
            "//src/my_test:foo": find_affected.FormattedResult(
                False, ["core.x64", "core.arm64"]
            ),
            "//src/other:bar": find_affected.FormattedResult(
                False, ["core.x64"]
            ),
            "//src/host:x64": find_affected.FormattedResult(True, ["core.x64"]),
            "//src/arm:arm64": find_affected.FormattedResult(
                False, ["core.arm64"]
            ),
        }

        output = find_affected.format_affected_targets(input_dict)
        self.assertEqual(len(output), 4)

        # Expected to be sorted alphabetically by label!
        self.assertEqual(output[0].pure_label, "//src/arm:arm64")
        self.assertEqual(output[0].command, "fx add-test //src/arm:arm64")
        self.assertListEqual(output[0].pb_configs, ["core.arm64"])

        self.assertEqual(output[1].pure_label, "//src/host:x64")
        self.assertEqual(output[1].command, "fx add-host-test //src/host:x64")
        self.assertListEqual(output[1].pb_configs, ["core.x64"])

        self.assertEqual(output[2].pure_label, "//src/my_test:foo")
        self.assertEqual(output[2].command, "fx add-test //src/my_test:foo")
        self.assertListEqual(output[2].pb_configs, ["core.x64", "core.arm64"])

        self.assertEqual(output[3].pure_label, "//src/other:bar")
        self.assertEqual(output[3].command, "fx add-test //src/other:bar")
        self.assertListEqual(output[3].pb_configs, ["core.x64"])

    def test_format_affected_commands(self) -> None:
        """Tests formatting grouped add-test / add-host-test command lines for affected targets."""
        targets = [
            find_affected.AffectedTarget(
                "//src/arm:arm64",
                ["core.arm64"],
                "fx add-test //src/arm:arm64",
                is_host=False,
            ),
            find_affected.AffectedTarget(
                "//src/host:x64",
                ["core.x64"],
                "fx add-host-test //src/host:x64",
                is_host=True,
            ),
            find_affected.AffectedTarget(
                "//src/my_test:foo",
                ["core.x64"],
                "fx add-test //src/my_test:foo",
                is_host=False,
            ),
        ]
        cmds = find_affected.format_affected_commands(targets)
        self.assertEqual(
            cmds,
            [
                "fx add-test //src/arm:arm64 //src/my_test:foo",
                "fx add-host-test //src/host:x64",
            ],
        )

    def test_clean_gathered_results(self) -> None:
        """Tests merging results from multiple build invocations into a target-to-product mapping."""
        # Simulated raw output from run_find_affected
        results = [
            find_affected.GatheredResult(
                "core.x64",
                [
                    find_affected.AffectedResult("//src/my_test:foo", False),
                    find_affected.AffectedResult("//src/other:bar", False),
                ],
            ),
            find_affected.GatheredResult(
                "core.arm64",
                [
                    find_affected.AffectedResult("//src/my_test:foo", False),
                    find_affected.AffectedResult("not_a_label", False),
                ],
            ),
            find_affected.GatheredResult("minimal.x64", []),
        ]

        mapped = find_affected.clean_gathered_results(results)

        self.assertEqual(len(mapped), 2)
        # Should drop 'not_a_label' and aggregate the configs.
        self.assertListEqual(
            mapped["//src/my_test:foo"].pb_configs, ["core.x64", "core.arm64"]
        )
        self.assertFalse(mapped["//src/my_test:foo"].is_host)
        self.assertListEqual(mapped["//src/other:bar"].pb_configs, ["core.x64"])
        self.assertFalse(mapped["//src/other:bar"].is_host)

    @mock.patch("find_affected.FxCmd")
    @mock.patch("execution.run_command")
    async def test_find_affected_tests(
        self, mock_run_command: mock.AsyncMock, mock_fx_cmd_class: mock.Mock
    ) -> None:
        """Tests the full find_affected_tests flow with successful command execution."""
        mock_fx = mock_fx_cmd_class.return_value
        mock_fx.start = mock.AsyncMock()
        mock_fx_running = mock_fx.start.return_value
        mock_fx_running.run_to_completion = mock.AsyncMock(
            return_value=mock.Mock(return_code=0)
        )

        mock_run_command.return_value = mock.Mock(
            stdout="//src/test1:foo,device\n//src/test2:bar,host\n",
            return_code=0,
        )

        gathered = await find_affected.find_affected_tests(
            "/fuchsia", "core.x64", "/out/core_x64", ["//bundle"], "/tmp/files"
        )

        self.assertEqual(gathered.product_board, "core.x64")
        self.assertListEqual(
            gathered.affected_results,
            [
                find_affected.AffectedResult("//src/test1:foo", False),
                find_affected.AffectedResult("//src/test2:bar", True),
            ],
        )

        # Verify FxCmd was initialized correctly
        mock_fx_cmd_class.assert_called_once_with(
            build_directory="/out/core_x64"
        )
        # Verify fx set was called
        mock_fx.start.assert_called_once_with(
            "set",
            "core.x64",
            "--no-change-env",
            "--rbe-mode=off",
            "--with",
            "//bundle",
        )
        # Verify build-api-client was called
        mock_run_command.assert_called_once()
        args = mock_run_command.call_args[0]
        self.assertIn("/fuchsia/build/api/client", args[0])
        self.assertIn("affected_tests", args)

    @mock.patch("find_affected.FxCmd")
    async def test_find_affected_tests_fx_set_failure(
        self, mock_fx_cmd_class: mock.Mock
    ) -> None:
        """Tests that find_affected_tests returns empty results if fx set fails."""
        mock_fx = mock_fx_cmd_class.return_value
        mock_fx.start = mock.AsyncMock()
        mock_fx_running = mock_fx.start.return_value
        # Return non-zero return code for fx set.
        mock_fx_running.run_to_completion = mock.AsyncMock(
            return_value=mock.Mock(return_code=1)
        )

        gathered = await find_affected.find_affected_tests(
            "/fuchsia", "core.x64", "/out/core_x64", [], "/tmp/files"
        )

        self.assertEqual(gathered.product_board, "core.x64")
        self.assertListEqual(gathered.affected_results, [])

    @mock.patch("find_affected.FxCmd")
    async def test_find_affected_tests_exception(
        self, mock_fx_cmd_class: mock.Mock
    ) -> None:
        """Tests that find_affected_tests handles unexpected exceptions gracefully."""
        mock_fx = mock_fx_cmd_class.return_value
        # Raise an exception when starting the command.
        mock_fx.start = mock.AsyncMock(
            side_effect=RuntimeError("Generic error")
        )

        gathered = await find_affected.find_affected_tests(
            "/fuchsia", "core.x64", "/out/core_x64", [], "/tmp/files"
        )

        self.assertEqual(gathered.product_board, "core.x64")
        self.assertListEqual(gathered.affected_results, [])
