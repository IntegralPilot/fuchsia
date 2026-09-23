# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import tempfile
import unittest
import unittest.mock as mock

from parameterized import parameterized

import args
import config

# We need a place to create a temporary file that is used to ensure
# output directory checking works correctly. Place it here and then
# cleanup after the tests are done executing.
GLOBAL_TEMP_DIRECTORY = tempfile.TemporaryDirectory()
GLOBAL_FILE_NAME = os.path.join(GLOBAL_TEMP_DIRECTORY.name, "tempfile")
open(GLOBAL_FILE_NAME, "w").close()


class TestArgs(unittest.TestCase):
    @classmethod
    def tearDownClass(cls) -> None:
        GLOBAL_TEMP_DIRECTORY.cleanup()
        return super().tearDownClass()

    def test_empty(self) -> None:
        flags = args.parse_args([])
        flags.validate()
        self.assertTrue(flags.allow_empty_selection)

    def test_allow_empty_selection(self) -> None:
        flags = args.parse_args(["--allow-empty-selection"])
        self.assertTrue(flags.allow_empty_selection)

        flags_no = args.parse_args(["--no-allow-empty-selection"])
        self.assertFalse(flags_no.allow_empty_selection)

    @parameterized.expand(
        [
            (
                "cannot show status with --simple",
                ["--simple", "--status"],
            ),
            (
                "cannot show style with --simple",
                ["--simple", "--style"],
            ),
            (
                "cannot show status with --agent-output",
                ["--agent-output", "--status"],
            ),
            (
                "cannot show style with --agent-output",
                ["--agent-output", "--style"],
            ),
            (
                "cannot show status when terminal is not a TTY",
                ["--status"],
            ),
            (
                "cannot run only host and only device tests",
                ["--device", "--host"],
            ),
            (
                "cannot exceed maximum status frequency",
                ["--status-delay", ".00001"],
            ),
            (
                "cannot have a negative suggestion count",
                ["--suggestion-count", "-1"],
            ),
            (
                "cannot run tests 0 times",
                ["--count", "0"],
            ),
            (
                "cannot have a negative timeout",
                ["--timeout", "-3"],
            ),
            (
                "cannot output to a file",
                ["--ffx-output-directory", GLOBAL_FILE_NAME],
            ),
            (
                "cannot output to a file (artifact flag)",
                ["--artifact-output-directory", GLOBAL_FILE_NAME],
            ),
            (
                "cannot output to a file (outdir flag)",
                ["--outdir", GLOBAL_FILE_NAME],
            ),
            (
                "cannot set a negative --parallel",
                ["--parallel", "-1"],
            ),
            (
                "cannot set a negative --parallel-cases",
                ["--parallel-cases", "-1"],
            ),
            (
                "invalid environment variable formatting is checked",
                ["-e", "abcd"],
            ),
            (
                "--break-on-failure and --breakpoint flags are not supported with host tests.",
                ["--break-on-failure", "--host"],
            ),
            (
                "--break-on-failure and --breakpoint flags are not supported with host tests.",
                ["--breakpoint", "test.cc:123", "--host"],
            ),
            (
                "--breakpoint does not support --use-existing-debugger.",
                ["--breakpoint", "test.cc:123", "--use-existing-debugger"],
            ),
            (
                "--break-on-failure must be set when passing --use-existing-debugger.",
                ["--use-existing-debugger"],
            ),
        ]
    )
    @mock.patch("args.termout.is_valid", return_value=False)
    def test_validation_errors(
        self, _unused_name: str, arg_list: list[str], _mock: mock.Mock
    ) -> None:
        flags = args.parse_args(arg_list)
        try:
            with self.assertRaises(args.FlagError):
                flags.validate()
        except AssertionError:
            raise AssertionError("Expected FlagError from " + str(arg_list))

    def test_gemini_analysis(self) -> None:
        # test default behavior (no flag)
        flags = args.parse_args([])
        flags.validate()
        self.assertEqual(flags.gemini_analysis, None)

        # test flag without a value (should default to 1)
        flags = args.parse_args(["--gemini-analysis"])
        flags.validate()
        self.assertEqual(flags.gemini_analysis, 1)

        # test explicit values
        for i in range(1, 4):
            flags = args.parse_args([f"--gemini-analysis={i}"])
            flags.validate()
            self.assertEqual(flags.gemini_analysis, i)

        # test invalid value
        with self.assertRaises(argparse.ArgumentError):
            args.parse_args(["--gemini-analysis=0"])

        # test invalid value
        with self.assertRaises(argparse.ArgumentError):
            args.parse_args(["--gemini-analysis=5"])

    def test_gemini_model_arg(self) -> None:
        # test default behavior (no flag)
        flags = args.parse_args([])
        flags.validate()
        self.assertEqual(
            flags.gemini_model, "gemini-2.5-flash-lite-preview-09-2025"
        )

        # test explicit value
        flags = args.parse_args(["--gemini-model", "test-model"])
        flags.validate()
        self.assertEqual(flags.gemini_model, "test-model")

    def test_simple(self) -> None:
        flags = args.parse_args(["--simple"])
        flags.validate()
        self.assertEqual(flags.style, False)
        self.assertEqual(flags.status, False)

    def test_agent_detection(self) -> None:
        # Ensure agent env vars are NOT set
        env_without_agents = os.environ.copy()
        for var in [
            "ANTIGRAVITY_AGENT",
            "ANTIGRAVITY_EDITOR_APP_ROOT",
            "GEMINI_CLI",
        ]:
            if var in env_without_agents:
                del env_without_agents[var]

        with mock.patch.dict(os.environ, env_without_agents, clear=True):
            flags = args.parse_args([])
            flags.validate()
            self.assertEqual(flags.simple, False)
            self.assertEqual(flags.agent_output, False)

            flags = args.parse_args(["--quiet"])
            flags.validate()
            self.assertEqual(flags.quiet, True)

        # Test with each agent env var
        for var in [
            "ANTIGRAVITY_AGENT",
            "ANTIGRAVITY_EDITOR_APP_ROOT",
            "GEMINI_CLI",
        ]:
            env_with_agent = env_without_agents.copy()
            env_with_agent[var] = "true"
            with mock.patch.dict(os.environ, env_with_agent, clear=True):
                flags = args.parse_args([])
                flags.validate()
                self.assertEqual(flags.simple, True)
                self.assertEqual(flags.agent_output, True)

        # Test override with --no-simple
        for var in [
            "ANTIGRAVITY_AGENT",
            "ANTIGRAVITY_EDITOR_APP_ROOT",
            "GEMINI_CLI",
        ]:
            env_with_agent = env_without_agents.copy()
            env_with_agent[var] = "true"
            with mock.patch.dict(os.environ, env_with_agent, clear=True):
                flags = args.parse_args(["--no-simple"])
                flags.validate()
                self.assertEqual(flags.simple, False)

        # Test override with --no-agent-output
        for var in [
            "ANTIGRAVITY_AGENT",
            "ANTIGRAVITY_EDITOR_APP_ROOT",
            "GEMINI_CLI",
        ]:
            env_with_agent = env_without_agents.copy()
            env_with_agent[var] = "true"
            with mock.patch.dict(os.environ, env_with_agent, clear=True):
                flags = args.parse_args(["--no-agent-output"])
                flags.validate()
                self.assertEqual(flags.agent_output, False)

    def test_allow_temporary_emulator(self) -> None:
        # Default should be True
        flags = args.parse_args([])
        flags.validate()
        self.assertEqual(flags.allow_temporary_emulator, True)

        flags = args.parse_args(["--allow-temporary-emulator"])
        flags.validate()
        self.assertEqual(flags.allow_temporary_emulator, True)

        flags = args.parse_args(["--no-allow-temporary-emulator"])
        flags.validate()
        self.assertEqual(flags.allow_temporary_emulator, False)

    def test_capture_syslog(self) -> None:
        # Default should be True
        flags = args.parse_args([])
        flags.validate()
        self.assertEqual(flags.capture_syslog, True)

        flags = args.parse_args(["--capture-syslog"])
        flags.validate()
        self.assertEqual(flags.capture_syslog, True)

        flags = args.parse_args(["--no-capture-syslog"])
        flags.validate()
        self.assertEqual(flags.capture_syslog, False)

    def test_json(self) -> None:
        flags = args.parse_args(["--json"])
        flags.validate()
        self.assertEqual(flags.json, True)
        self.assertEqual(flags.logpath, args.LOG_TO_STDOUT_OPTION)

        flags = args.parse_args([])
        flags.validate()
        self.assertEqual(flags.json, False)
        self.assertIsNone(flags.logpath)

    def test_save_log_path_to_file(self) -> None:
        flags = args.parse_args(["--save-log-path-to-file", "/tmp/logpath_ptr"])
        flags.validate()
        self.assertEqual(flags.save_log_path_to_file, "/tmp/logpath_ptr")

        flags = args.parse_args([])
        flags.validate()
        self.assertIsNone(flags.save_log_path_to_file)

    def test_e2e(self) -> None:
        flags = args.parse_args(["--only-e2e"])
        flags.validate()
        self.assertEqual(flags.e2e, True)
        self.assertEqual(flags.only_e2e, True)

    def test_use_test_pilot(self) -> None:
        flags = args.parse_args(["--use-test-pilot"])
        flags.validate()
        self.assertEqual(flags.use_test_pilot, True)

        flags = args.parse_args([])
        flags.validate()
        self.assertEqual(flags.use_test_pilot, False)

    def test_expand_output_variable(self) -> None:
        flags = args.parse_args(["--outdir", "$FUCHSIA_OUT/test_out"])
        flags.validate()
        flags.update_artifacts_directory_with_out_path("out/default")
        self.assertEqual(
            flags.artifact_output_directory, "out/default/test_out"
        )

        flags = args.parse_args(["--outdir", "${FUCHSIA_OUT}/test_out2"])
        flags.validate()
        flags.update_artifacts_directory_with_out_path("out/default2")
        self.assertEqual(
            flags.artifact_output_directory, "out/default2/test_out2"
        )

    def test_default_merging(self) -> None:
        config_file = config.ConfigFile(
            "path", args.parse_args(["--parallel=10"])
        )
        flags = args.parse_args([], config_file.default_flags)
        self.assertEqual(flags.parallel, 10)
        flags = args.parse_args(["--parallel=1"], config_file.default_flags)
        self.assertEqual(flags.parallel, 1)

    def test_quiet_override(self) -> None:
        """
        You can override -q in a config file with --no-quiet on the command line
        """
        config_file = config.ConfigFile("path", args.parse_args(["-q"]))
        flags = args.parse_args([], config_file.default_flags)
        self.assertEqual(flags.quiet, True)
        flags = args.parse_args(["--no-quiet"], config_file.default_flags)
        self.assertEqual(flags.quiet, False)

    def test_selections_after_test_filter(self) -> None:
        """Passing more selections after a --test-filter works"""
        flags = args.parse_args(["foo", "--test-filter", "some*", "bar"])
        flags.validate()
        self.assertListEqual(flags.test_filter, ["some*"])
        self.assertListEqual(flags.selection, ["foo", "bar"])

        flags = args.parse_args(
            [
                "foo",
                "--test-filter",
                "some*",
                "bar",
                "-a",
                "baz",
                "alpha",
                "--test-filter",
                "something*",
                "beta",
                "-c",
                "gamma.cm",
            ]
        )
        flags.validate()
        self.assertListEqual(flags.test_filter, ["some*", "something*"])
        self.assertListEqual(
            flags.selection,
            [
                "foo",
                "bar",
                "--and",
                "baz",
                "alpha",
                "beta",
                "--component",
                "gamma.cm",
            ],
        )

    def test_exact_after_selections(self) -> None:
        """Passing --exact after a selection works"""
        flags = args.parse_args(["-p", "foo", "-a", "-c", "bar", "--exact"])
        flags.validate()
        self.assertEqual(flags.exact, True)
        self.assertListEqual(
            flags.selection,
            ["--package", "foo", "--and", "--component", "bar"],
        )

    @parameterized.expand(
        [
            ("default is None", [], [], None),
            ("config file overrides output", [], ["--output"], True),
            ("-o shows output", ["-o"], [], True),
            ("--output shows output", ["--output"], [], True),
            ("--no-output hides output", ["--no-output"], [], False),
            (
                "--no-output overrides config",
                ["--no-output"],
                ["--output"],
                False,
            ),
        ]
    )
    def test_output_toggle(
        self,
        _unused_name: str,
        arguments: list[str],
        config_arguments: list[str],
        expected_value: bool,
    ) -> None:
        config_file = config.ConfigFile(
            "path", args.parse_args(config_arguments)
        )
        flags = args.parse_args(arguments, config_file.default_flags)
        self.assertEqual(flags.output, expected_value)

    @parameterized.expand(
        [
            ("<no arguments>", [], False, False),
            ("--break-on-failure", ["--break-on-failure"], True, True),
            ("--breakpoint", ["--breakpoint", "test.cc:123"], True, True),
            (
                "--break-on-failure --use-existing-debugger",
                ["--break-on-failure", "--use-existing-debugger"],
                True,
                False,
            ),
            (
                "--breakpoint --break-on-failure",
                ["--break-on-failure", "--breakpoint", "test.cc:123"],
                True,
                True,
            ),
        ]
    )
    def test_should_start_debugger(
        self,
        _unused_name: str,
        arguments: list[str],
        debugger_will_attach: bool,
        debugger_should_spawn: bool,
    ) -> None:
        """Passing --break-on-failure or --breakpoint does not set debugger_should_spawn()."""
        flags = args.parse_args(arguments)
        flags.validate()
        self.assertEqual(flags.debugger_will_attach(), debugger_will_attach)
        self.assertEqual(flags.debugger_should_spawn(), debugger_should_spawn)

    def test_agent_debugging_mode(self) -> None:
        """Tests that --agent-debugging-mode implies --break-on-failure and --enable-debug-adapter."""
        flags = args.parse_args(["--agent-debugging-mode"])
        flags.validate()
        self.assertEqual(flags.agent_debugging_mode, True)
        self.assertEqual(flags.break_on_failure, True)
        self.assertEqual(flags.enable_debug_adapter, True)

    def test_break_on_failure_boolean_optional(self) -> None:
        """Tests that boolean optional flags work and raise FlagError on conflict."""
        # Test --no-break-on-failure correctly resolves to False
        flags = args.parse_args(["--no-break-on-failure"])
        flags.validate()
        self.assertEqual(flags.break_on_failure, False)

        # Test --no-enable-debug-adapter correctly resolves to False
        flags = args.parse_args(["--no-enable-debug-adapter"])
        flags.validate()
        self.assertEqual(flags.enable_debug_adapter, False)

        # Test that explicitly disabling either flag while enabling agent debugging raises FlagError
        with self.assertRaises(args.FlagError):
            flags = args.parse_args(
                ["--agent-debugging-mode", "--no-break-on-failure"]
            )
            flags.validate()

        with self.assertRaises(args.FlagError):
            flags = args.parse_args(
                ["--agent-debugging-mode", "--no-enable-debug-adapter"]
            )
            flags.validate()
