# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import shutil
import tempfile
import unittest
import unittest.mock as mock

from parameterized import parameterized

import args
import environment


class TestExecutionEnvironment(unittest.TestCase):
    def _make_test_files(
        self,
        tmp: str,
        real_out_dir: str | None = None,
        out_dir_in_file: str | None = None,
    ) -> None:
        out_dir = (
            os.path.join(tmp, "out", "foo")
            if not real_out_dir
            else real_out_dir
        )
        os.makedirs(out_dir)
        with open(os.path.join(tmp, ".fx-build-dir"), "w") as f:
            f.write(out_dir if not out_dir_in_file else out_dir_in_file)

        open(os.path.join(out_dir, "tests.json"), "a").close()
        open(os.path.join(out_dir, "test-list.json"), "a").close()
        open(os.path.join(out_dir, "package-repositories.json"), "a").close()

        sdk_ctf_dir = os.path.join(tmp, "sdk", "ctf")
        os.makedirs(sdk_ctf_dir)
        open(os.path.join(sdk_ctf_dir, "disabled_tests.json"), "a").close()

    def test_process_environment(self) -> None:
        """Test that we can load and use an environment."""
        with tempfile.TemporaryDirectory() as tmp:
            self._make_test_files(tmp)

            out_dir = os.path.join(tmp, "out", "foo")
            default_flags = args.parse_args([])

            with mock.patch.dict(
                os.environ,
                {"FUCHSIA_DIR": tmp, "FUCHSIA_BUILD_DIR_FROM_FX": ""},
            ):
                env = environment.ExecutionEnvironment.initialize_from_args(
                    default_flags
                )
                self.assertEqual(env.fuchsia_dir, tmp)
                self.assertEqual(env.out_dir, out_dir)
                self.assertTrue(
                    env.log_file and env.log_file.startswith(out_dir), str(env)
                )
                self.assertTrue(
                    env.log_file and "fxtest" in env.log_file, str(env)
                )
                self.assertEqual(
                    env.test_json_file, os.path.join(out_dir, "tests.json")
                )
                self.assertIsNone(env.test_list_file)

                self.assertEqual(
                    env.relative_to_root(os.path.join(tmp, "foo", "bar")),
                    os.path.join("foo", "bar"),
                )

    def test_process_environment_with_fx_set_build_dir(self) -> None:
        """Test that we can load and use an environment with a build directory set by fx"""
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = os.path.join(tmp, "out", "baz")
            self._make_test_files(
                tmp,
                real_out_dir=out_dir,
                out_dir_in_file=os.path.join(tmp, "out", "foo"),
            )

            default_flags = args.parse_args([])

            with mock.patch.dict(
                os.environ,
                {"FUCHSIA_DIR": tmp, "FUCHSIA_BUILD_DIR_FROM_FX": out_dir},
            ):
                env = environment.ExecutionEnvironment.initialize_from_args(
                    default_flags
                )
                self.assertEqual(env.fuchsia_dir, tmp)
                self.assertEqual(env.out_dir, out_dir)
                self.assertTrue(
                    env.log_file and env.log_file.startswith(out_dir), str(env)
                )
                self.assertTrue(
                    env.log_file and "fxtest" in env.log_file, str(env)
                )
                self.assertEqual(
                    env.test_json_file, os.path.join(out_dir, "tests.json")
                )
                self.assertIsNone(env.test_list_file)

                self.assertEqual(
                    env.relative_to_root(os.path.join(tmp, "foo", "bar")),
                    os.path.join("foo", "bar"),
                )

    def test_no_fuchsia_dir(self) -> None:
        with mock.patch.dict(os.environ, {"FUCHSIA_DIR": ""}, clear=True):
            default_flags = args.parse_args([])
            self.assertRaisesRegex(
                environment.EnvironmentError,
                r"FUCHSIA_DIR",
                lambda: environment.ExecutionEnvironment.initialize_from_args(
                    default_flags
                ),
            )

    def test_missing_build_dir_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self._make_test_files(tmp)
            os.remove(os.path.join(tmp, ".fx-build-dir"))

            with mock.patch.dict(os.environ, {"FUCHSIA_DIR": tmp}, clear=True):
                default_flags = args.parse_args([])
                self.assertRaisesRegex(
                    environment.EnvironmentError,
                    r".fx-build-dir",
                    lambda: environment.ExecutionEnvironment.initialize_from_args(
                        default_flags
                    ),
                )

    def test_missing_build_dir(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self._make_test_files(tmp)
            shutil.rmtree(os.path.join(tmp, "out", "foo"))

            with mock.patch.dict(os.environ, {"FUCHSIA_DIR": tmp}, clear=True):
                default_flags = args.parse_args([])
                self.assertRaisesRegex(
                    environment.EnvironmentError,
                    r"Expected directory at.*Ensure you have set up your build directory correctly using 'fx set'.",
                    lambda: environment.ExecutionEnvironment.initialize_from_args(
                        default_flags
                    ),
                )

    def test_missing_tests_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self._make_test_files(tmp)
            os.remove(os.path.join(tmp, "out", "foo", "tests.json"))

            with mock.patch.dict(os.environ, {"FUCHSIA_DIR": tmp}, clear=True):
                default_flags = args.parse_args([])
                self.assertRaisesRegex(
                    environment.EnvironmentError,
                    r"tests.json",
                    lambda: environment.ExecutionEnvironment.initialize_from_args(
                        default_flags
                    ),
                )

    @parameterized.expand(
        [
            (
                "unset_ffx",
                {},
                ["ffx", "target", "list"],
                [
                    "fx",
                    "--dir",
                    "/fuchsia/out/default",
                    "ffx",
                    "target",
                    "list",
                ],
            ),
            (
                "unset_serve",
                {},
                ["serve"],
                ["fx", "--dir", "/fuchsia/out/default", "serve"],
            ),
            (
                "unset_build",
                {},
                ["build", "test"],
                ["fx", "--dir", "/fuchsia/out/default", "build", "test"],
            ),
            ("unset_empty", {}, [], ["fx", "--dir", "/fuchsia/out/default"]),
            (
                "set_ffx",
                {"FUCHSIA_NODENAME": "dev"},
                ["ffx", "target", "list"],
                [
                    "fx",
                    "--dir",
                    "/fuchsia/out/default",
                    "-t",
                    "dev",
                    "ffx",
                    "target",
                    "list",
                ],
            ),
            (
                "set_serve",
                {"FUCHSIA_NODENAME": "dev"},
                ["serve", "-v"],
                [
                    "fx",
                    "--dir",
                    "/fuchsia/out/default",
                    "-t",
                    "dev",
                    "serve",
                    "-v",
                ],
            ),
            (
                "set_build",
                {"FUCHSIA_NODENAME": "dev"},
                ["build", "test"],
                [
                    "fx",
                    "--dir",
                    "/fuchsia/out/default",
                    "-t",
                    "dev",
                    "build",
                    "test",
                ],
            ),
            (
                "has_short_flag",
                {"FUCHSIA_NODENAME": "dev"},
                ["ffx", "-t", "other", "target", "list"],
                [
                    "fx",
                    "--dir",
                    "/fuchsia/out/default",
                    "ffx",
                    "-t",
                    "other",
                    "target",
                    "list",
                ],
            ),
            (
                "has_long_flag",
                {"FUCHSIA_NODENAME": "dev"},
                ["ffx", "--target", "other", "target", "list"],
                [
                    "fx",
                    "--dir",
                    "/fuchsia/out/default",
                    "ffx",
                    "--target",
                    "other",
                    "target",
                    "list",
                ],
            ),
        ]
    )
    def test_fx_cmd_line(
        self,
        _name: str,
        env_dict: dict[str, str],
        input_args: list[str],
        expected: list[str],
    ) -> None:
        env = environment.ExecutionEnvironment(
            fuchsia_dir="/fuchsia",
            out_dir="/fuchsia/out/default",
            test_json_file="/fuchsia/out/default/tests.json",
            disabled_ctf_tests_file="/fuchsia/sdk/ctf/disabled_tests.json",
        )
        with mock.patch.dict(os.environ, env_dict, clear=True):
            self.assertEqual(env.fx_cmd_line(*input_args), expected)

    def test_get_most_recent_log_stdout_option(self) -> None:
        """When log_file is '-' (stdout option), get_most_recent_log should search out_dir or raise EnvironmentError."""
        with tempfile.TemporaryDirectory() as tmp:
            env = environment.ExecutionEnvironment(
                fuchsia_dir=tmp,
                out_dir=tmp,
                test_json_file=os.path.join(tmp, "tests.json"),
                disabled_ctf_tests_file="",
                log_file=args.LOG_TO_STDOUT_OPTION,
            )
            self.assertRaises(
                environment.EnvironmentError,
                lambda: env.get_most_recent_log(),
            )
            log_path = os.path.join(tmp, "fxtest-2026-01-01T00:00:00.json.gz")
            with open(log_path, "w") as f:
                f.write("test")
            self.assertEqual(env.get_most_recent_log(), log_path)
