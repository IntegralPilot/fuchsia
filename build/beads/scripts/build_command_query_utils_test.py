# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import tempfile
import unittest
from pathlib import Path

import build_command_query_utils
import build_utils
from build_utils import MockCommandRunner, NinjaRunner


class TestBuildCommandQueryUtils(unittest.TestCase):
    MOCK_NINJA_BIN = "/mock-ninja"

    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.root_dir = Path(self._td.name)
        self.build_dir = self.root_dir / "out/default"
        self.build_dir.mkdir(parents=True)
        self.ninja_outputs_json = self.build_dir / "ninja_outputs.json"
        self.ninja_outputs_json.write_text("{}")

    def tearDown(self) -> None:
        self._td.cleanup()

    def _write_ninja_outputs(self, mapping: dict[str, list[str]]) -> None:
        """Write new ninja_outputs.json file."""
        with self.ninja_outputs_json.open("w") as f:
            json.dump(mapping, f)

    def _get_ninja_runners(
        self, output: None | str = None
    ) -> tuple[MockCommandRunner, NinjaRunner]:
        """Return a MockCommandRunner and mock NinjaRunner.

        Args:
            output: Optional string. If not None, a (0, output, "")
               result will be pushed to mock_runner before this function exits.
        Returns:
            a (MockCommandRunner, NinjaRunner) tuple.
        """
        mock_runner = MockCommandRunner()
        mock_ninja = NinjaRunner(
            Path(self.MOCK_NINJA_BIN), self.build_dir, mock_runner
        )
        if output is not None:
            mock_runner.push_result(0, output, "")
        return mock_runner, mock_ninja

    def test_query_ninja_commands(self) -> None:
        mock_runner, mock_ninja = self._get_ninja_runners(
            "rustc --crate-name bar obj/foo/bar.o\n"
            + "rustc --crate-name baz obj/foo/baz.o\n",
        )

        self._write_ninja_outputs(
            {
                "//foo:foo": ["obj/foo/foo.o"],
                "//foo:bar": ["obj/foo/bar.o"],
                "//foo:baz": ["obj/foo/baz.o"],
            }
        )
        self.assertDictEqual(
            build_command_query_utils.query_ninja_commands(
                mock_ninja,
                ["//foo:bar", "//foo:baz"],
            ),
            {
                "//foo:bar": "rustc --crate-name bar obj/foo/bar.o",
                "//foo:baz": "rustc --crate-name baz obj/foo/baz.o",
            },
        )

        self.assertListEqual(
            mock_runner.results[-1].args,
            [
                self.MOCK_NINJA_BIN,
                "-C",
                str(self.build_dir),
                "-t",
                "commands",
                "-s",
                "obj/foo/bar.o",
                "obj/foo/baz.o",
            ],
        )

    def test_query_ninja_commands_empty_labels(self) -> None:
        mock_runner, mock_ninja = self._get_ninja_runners("")

        self.assertDictEqual(
            build_command_query_utils.query_ninja_commands(mock_ninja, []),
            {},
        )
        self.assertListEqual(mock_runner.commands, [])

    def test_query_ninja_commands_ninja_error(self) -> None:
        mock_runner, mock_ninja = self._get_ninja_runners()
        mock_runner.push_result(1, "", "Ninja failed!")

        self._write_ninja_outputs(
            {
                "//foo:bar": ["obj/foo/bar.o"],
            }
        )
        with self.assertRaisesRegex(
            ValueError, "Could not find command for label: //foo:bar"
        ) as cm:
            build_command_query_utils.query_ninja_commands(
                mock_ninja, ["//foo:bar"]
            )

    def test_query_ninja_commands_mismatch(self) -> None:
        _, mock_ninja = self._get_ninja_runners(
            "rustc --crate-name bar obj/foo/BLOOP.o\n"
            + "rustc --crate-name baz obj/foo/baz.o\n",
        )

        self._write_ninja_outputs(
            {
                "//foo:bar": ["obj/foo/bar.o"],
                "//foo:baz": ["obj/foo/baz.o"],
            }
        )
        with self.assertRaisesRegex(ValueError, "Could not find command"):
            build_command_query_utils.query_ninja_commands(
                mock_ninja,
                ["//foo:bar", "//foo:baz"],
            )

    def test_query_ninja_commands_missing_command(self) -> None:
        _, mock_ninja = self._get_ninja_runners(
            "rustc --crate-name bar obj/foo/bar.o\n",
        )
        self._write_ninja_outputs(
            {
                "//foo:bar": ["obj/foo/bar.o"],
                "//foo:baz": ["obj/foo/baz.o"],
            }
        )
        with self.assertRaisesRegex(ValueError, "Could not find command"):
            build_command_query_utils.query_ninja_commands(
                mock_ninja,
                ["//foo:bar", "//foo:baz"],
            )

    def test_query_ninja_commands_missing_label(self) -> None:
        mock_runner, mock_ninja = self._get_ninja_runners()

        self._write_ninja_outputs(
            {
                "//foo:bar": ["obj/foo/bar.o"],
            }
        )
        with self.assertRaisesRegex(
            ValueError, "Could not find outputs for label"
        ):
            build_command_query_utils.query_ninja_commands(
                mock_ninja,
                ["//foo:baz"],
            )

    def test_query_bazel_commands(self) -> None:
        mock_bazel_launcher = build_utils.MockBazelLauncher()
        mock_bazel_launcher.push_expected_outputs(
            [
                json.dumps(
                    {
                        "targets": [
                            {"id": "1", "label": "//foo:bar"},
                            {"id": "2", "label": "//foo:baz"},
                        ],
                        "actions": [
                            {
                                "targetId": "1",
                                "arguments": ["rustc", "--crate-name", "bar"],
                                "mnemonic": "Rustc",
                            },
                            {
                                "targetId": "2",
                                "arguments": ["rustc", "--crate-name", "baz"],
                                "mnemonic": "Rustc",
                            },
                        ],
                    }
                )
            ]
        )

        self.assertDictEqual(
            build_command_query_utils.query_bazel_commands(
                mock_bazel_launcher, "execroot", ["//foo:bar", "//foo:baz"]
            ),
            {
                "//foo:bar": "rustc --crate-name bar",
                "//foo:baz": "rustc --crate-name baz",
            },
        )

        last_args = mock_bazel_launcher.command_runner.results[0].args
        self.assertEqual(
            last_args,
            [
                "bazel",
                "aquery",
                "--config=host",
                "--config=quiet",
                "--consistent_labels",
                "--output=jsonproto",
                'mnemonic("Rustc", //foo:bar + //foo:baz)',
            ],
        )

    def test_query_bazel_commands_with_env_vars(self) -> None:
        mock_bazel_launcher = build_utils.MockBazelLauncher()
        mock_bazel_launcher.push_expected_outputs(
            [
                json.dumps(
                    {
                        "targets": [
                            {"id": "1", "label": "//foo:bar"},
                        ],
                        "actions": [
                            {
                                "targetId": "1",
                                "arguments": ["rustc", "--crate-name", "bar"],
                                "environmentVariables": [
                                    {"key": "CARGO_PKG_NAME", "value": "bar"}
                                ],
                                "mnemonic": "Rustc",
                            },
                        ],
                    }
                )
            ]
        )

        self.assertDictEqual(
            build_command_query_utils.query_bazel_commands(
                mock_bazel_launcher, "execroot", ["//foo:bar"]
            ),
            {
                "//foo:bar": "CARGO_PKG_NAME=bar rustc --crate-name bar",
            },
        )

    def test_query_bazel_commands_normalized_label(self) -> None:
        mock_bazel_launcher = build_utils.MockBazelLauncher()
        mock_bazel_launcher.push_expected_outputs(
            [
                json.dumps(
                    {
                        "targets": [
                            {"id": "1", "label": "@@//foo:bar"},
                        ],
                        "actions": [
                            {
                                "targetId": "1",
                                "arguments": ["rustc", "--crate-name", "bar"],
                                "mnemonic": "Rustc",
                            },
                        ],
                    }
                )
            ]
        )

        self.assertDictEqual(
            build_command_query_utils.query_bazel_commands(
                mock_bazel_launcher, "execroot", ["//foo:bar"]
            ),
            {
                "//foo:bar": "rustc --crate-name bar",
            },
        )

    def test_query_bazel_commands_error(self) -> None:
        mock_bazel_launcher = build_utils.MockBazelLauncher()
        mock_bazel_launcher.command_runner.push_result(returncode=1)

        with self.assertRaisesRegex(
            ValueError, "Failed to run bazel action expansion"
        ):
            build_command_query_utils.query_bazel_commands(
                mock_bazel_launcher, "execroot", ["//foo:bar"]
            )

    def test_query_bazel_commands_empty_labels(self) -> None:
        mock_launcher = build_utils.MockBazelLauncher()
        self.assertDictEqual(
            build_command_query_utils.query_bazel_commands(
                mock_launcher, "execroot", []
            ),
            {},
        )
        self.assertEqual(len(mock_launcher.command_runner.results), 0)

    def test_query_bazel_commands_invalid_json(self) -> None:
        mock_launcher = build_utils.MockBazelLauncher()
        mock_launcher.push_expected_outputs(["invalid json"])
        with self.assertRaisesRegex(ValueError, "Could not find command"):
            build_command_query_utils.query_bazel_commands(
                mock_launcher, "execroot", ["//foo:bar"]
            )

    def test_query_bazel_commands_missing_actions(self) -> None:
        mock_launcher = build_utils.MockBazelLauncher()
        mock_launcher.push_expected_outputs(
            [
                json.dumps(
                    {
                        "targets": [{"id": "1", "label": "//foo:bar"}],
                    }
                )
            ]
        )
        with self.assertRaisesRegex(ValueError, "Could not find command"):
            build_command_query_utils.query_bazel_commands(
                mock_launcher, "execroot", ["//foo:bar"]
            )

    def test_query_bazel_commands_target_not_in_results(self) -> None:
        mock_launcher = build_utils.MockBazelLauncher()
        mock_launcher.push_expected_outputs(
            [
                json.dumps(
                    {
                        "targets": [{"id": "1", "label": "//foo:baz"}],
                        "actions": [
                            {
                                "targetId": "1",
                                "arguments": ["rustc", "baz"],
                                "mnemonic": "Rustc",
                            }
                        ],
                    }
                )
            ]
        )
        with self.assertRaisesRegex(ValueError, "Could not find command"):
            build_command_query_utils.query_bazel_commands(
                mock_launcher, "execroot", ["//foo:bar"]
            )

    def test_query_bazel_commands_empty_arguments(self) -> None:
        mock_launcher = build_utils.MockBazelLauncher()
        mock_launcher.push_expected_outputs(
            [
                json.dumps(
                    {
                        "targets": [{"id": "1", "label": "//foo:bar"}],
                        "actions": [
                            {
                                "targetId": "1",
                                "arguments": [],
                                "mnemonic": "Rustc",
                            }
                        ],
                    }
                )
            ]
        )
        with self.assertRaisesRegex(ValueError, "Could not find command"):
            build_command_query_utils.query_bazel_commands(
                mock_launcher, "execroot", ["//foo:bar"]
            )

    def test_query_ninja_and_bazel_commands(self) -> None:
        mock_runner, mock_ninja = self._get_ninja_runners(
            "rustc --crate-name bar obj/foo/bar.o\n"
            + "rustc --crate-name baz obj/foo/baz.o\n",
        )

        self._write_ninja_outputs(
            {
                "//foo:foo": ["obj/foo/foo.o"],
                "//foo:bar": ["obj/foo/bar.o"],
                "//foo:baz": ["obj/foo/baz.o"],
            }
        )

        mock_bazel_launcher = build_utils.MockBazelLauncher()
        mock_bazel_launcher.push_expected_outputs(
            [
                json.dumps(
                    {
                        "targets": [
                            {"id": "1", "label": "//foo:bar"},
                            {"id": "2", "label": "//foo:baz"},
                        ],
                        "actions": [
                            {
                                "targetId": "1",
                                "arguments": ["rustc", "--crate-name", "bar"],
                                "mnemonic": "Rustc",
                            },
                            {
                                "targetId": "2",
                                "arguments": ["rustc", "--crate-name", "baz"],
                                "mnemonic": "Rustc",
                            },
                        ],
                    }
                )
            ]
        )

        (
            gn_cmds_map,
            bazel_cmds_map,
        ) = build_command_query_utils.query_ninja_and_bazel_commands(
            ["//foo:bar", "//foo:baz"],
            ["//foo:bar", "//foo:baz"],
            mock_ninja,
            mock_bazel_launcher,
            "execroot",
        )

        self.assertDictEqual(
            gn_cmds_map,
            {
                "//foo:bar": "rustc --crate-name bar obj/foo/bar.o",
                "//foo:baz": "rustc --crate-name baz obj/foo/baz.o",
            },
        )

        self.assertDictEqual(
            bazel_cmds_map,
            {
                "//foo:bar": "rustc --crate-name bar",
                "//foo:baz": "rustc --crate-name baz",
            },
        )

        self.assertListEqual(
            mock_runner.results[-1].args,
            [
                "/mock-ninja",
                "-C",
                str(self.build_dir),
                "-t",
                "commands",
                "-s",
                "obj/foo/bar.o",
                "obj/foo/baz.o",
            ],
        )


if __name__ == "__main__":
    unittest.main()
