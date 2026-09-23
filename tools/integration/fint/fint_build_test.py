#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for fint_build.py."""

import pathlib
import sys

# Find the fuchsia root containing prebuilt/third_party/protobuf-py3.
# This search is necessary because in hermetic test sandboxes (like fx_build_test),
# the directory structure is flattened to 3 levels deep instead of the usual 4,
# so a fixed relative path lookup (parent.parent.parent.parent) would fail.
fint_dir = pathlib.Path(__file__).resolve().parent
fuchsia_root = fint_dir
for _ in range(5):
    if (fuchsia_root / "prebuilt" / "third_party" / "protobuf-py3").exists():
        break
    fuchsia_root = fuchsia_root.parent

# Append fuchsia_root to sys.path so we can import tools.integration.fint.proto
if fuchsia_root.exists() and str(fuchsia_root) not in sys.path:
    sys.path.insert(0, str(fuchsia_root))

protobuf_wheel = fuchsia_root / "prebuilt" / "third_party" / "protobuf-py3"
if protobuf_wheel.exists() and str(protobuf_wheel) not in sys.path:
    sys.path.insert(0, str(protobuf_wheel))

# Bypass strict protobuf gencode/runtime version check to align prebuilts at build-time (see b/537501139)
try:
    import google.protobuf.runtime_version as rv  # type: ignore

    rv.ValidateProtobufRuntimeVersion = lambda *args, **kwargs: None
except ImportError:
    pass

import contextlib
import io
import json
import os
import subprocess
import tempfile
import time
import unittest
from typing import Any, Generator
from unittest import mock
from unittest.mock import MagicMock

import fint_build
from build.scripts import signal_utils
from google.protobuf import text_format
from tools.integration.fint.proto import context_pb2, static_pb2


class SpecificationsParserTest(unittest.TestCase):
    """Tests the parsing of protobuf specifications."""

    def test_parse_specs(self) -> None:
        """Verifies that static_pb2 textproto parsing works correctly."""
        static_text = 'incremental: true\nninja_targets: "target1"\nninja_targets: "target2"\n'
        static_spec = static_pb2.Static()
        text_format.Merge(static_text, static_spec)
        self.assertTrue(static_spec.incremental)
        self.assertEqual(
            list(static_spec.ninja_targets), ["target1", "target2"]
        )


class BuildArtifactsTest(unittest.TestCase):
    """Tests the produce_build_artifacts helper function."""

    def test_produce_build_artifacts(self) -> None:
        """Verifies that produce_build_artifacts successfully writes build_artifacts.json."""
        with tempfile.TemporaryDirectory() as artifact_dir:
            fint_build.produce_build_artifacts(pathlib.Path(artifact_dir), 42)
            manifest_path = (
                pathlib.Path(artifact_dir) / fint_build.BUILD_ARTIFACTS_JSON
            )
            self.assertTrue(manifest_path.exists())

            # Load and verify content
            manifest_content = json.loads(manifest_path.read_text())
            self.assertEqual(manifest_content.get("ninjaDurationSeconds"), 42)

    def test_produce_build_artifacts_with_failure(self) -> None:
        """Verifies that produce_build_artifacts successfully writes build_artifacts.json with a failure_summary."""
        with tempfile.TemporaryDirectory() as artifact_dir:
            fint_build.produce_build_artifacts(
                pathlib.Path(artifact_dir), 42, failure_summary="test failure"
            )
            manifest_path = (
                pathlib.Path(artifact_dir) / fint_build.BUILD_ARTIFACTS_JSON
            )
            self.assertTrue(manifest_path.exists())

            # Load and verify content
            manifest_content = json.loads(manifest_path.read_text())
            self.assertEqual(manifest_content.get("ninjaDurationSeconds"), 42)
            self.assertEqual(
                manifest_content.get("failureSummary"), "test failure"
            )


class ParseNinjaFailuresTest(unittest.TestCase):
    """Tests the parse_ninja_failures, format_ninja_failures, and NinjaFailure classes."""

    def test_ninja_failure_dataclass(self) -> None:
        """Verifies individual NinjaFailure methods and properties behave correctly."""
        # Test creation from valid dict
        failure = fint_build.NinjaFailure.from_dict(
            {
                "artifacts": ["out/default/foo.o"],
                "exit_code": 1,
                "output": "some error\n",
            }
        )
        self.assertIsNotNone(failure)
        self.assertEqual(failure.artifacts, ["out/default/foo.o"])
        self.assertEqual(failure.exit_code, 1)
        self.assertEqual(failure.output, "some error")
        self.assertFalse(failure.is_eligible_for_deduplication)
        self.assertEqual(
            failure.format(), "FAILED: [code=1] out/default/foo.o\n\nsome error"
        )
        self.assertEqual(
            failure.format(include_output=False),
            "FAILED: [code=1] out/default/foo.o",
        )

        # Test line-count based eligibility
        long_failure = fint_build.NinjaFailure.from_dict(
            {
                "artifacts": ["bin/bar"],
                "exit_code": 2,
                "output": "1\n2\n3\n4\n5\n6",
            }
        )
        self.assertTrue(long_failure.is_eligible_for_deduplication)

    def test_format_valid_failures(self) -> None:
        """Verifies format_ninja_failures correctly formats valid failures."""
        mock_data = {
            "version": 1,
            "failures": [
                {
                    "artifacts": ["bin/foo"],
                    "exit_code": 1,
                    "output": "compiler error",
                }
            ],
        }
        res = fint_build.format_ninja_failures(mock_data)
        self.assertEqual(res, "FAILED: [code=1] bin/foo\n\ncompiler error")

    def test_format_deduplication(self) -> None:
        """Verifies format_ninja_failures deduplicates long compiler errors, but preserves headers."""
        mock_data = {
            "version": 1,
            "failures": [
                {
                    "artifacts": ["bin/foo"],
                    "exit_code": 1,
                    "output": "line1\nline2\nline3\nline4\nline5\nline6",
                },
                {
                    "artifacts": ["bin/bar"],
                    "exit_code": 1,
                    "output": "line1\nline2\nline3\nline4\nline5\nline6",
                },
            ],
        }
        res = fint_build.format_ninja_failures(mock_data)
        expected = (
            "FAILED: [code=1] bin/foo\n\n"
            "line1\nline2\nline3\nline4\nline5\nline6\n\n"
            "FAILED: [code=1] bin/bar"
        )
        self.assertEqual(res, expected)

    def test_format_invalid_version_or_malformed(self) -> None:
        """Verifies format_ninja_failures returns None for malformed JSON or unsupported versions."""
        # Invalid version
        self.assertIsNone(
            fint_build.format_ninja_failures({"version": 2, "failures": []})
        )
        # Missing failures
        self.assertIsNone(fint_build.format_ninja_failures({"version": 1}))

    def test_parse_file_handling(self) -> None:
        """Verifies parse_ninja_failures correctly loads files and delegates."""
        with tempfile.TemporaryDirectory() as tmp_dir:
            errors_json_path = (
                pathlib.Path(tmp_dir) / fint_build.NINJA_ERRORS_JSON
            )

            # Malformed JSON
            errors_json_path.write_text("{invalid")
            self.assertIsNone(fint_build.parse_ninja_failures(errors_json_path))

            # Non-existent file
            self.assertIsNone(
                fint_build.parse_ninja_failures(
                    pathlib.Path("/non/existent/file")
                )
            )

            # Valid file
            mock_data = {
                "version": 1,
                "failures": [
                    {
                        "artifacts": ["bin/foo"],
                        "exit_code": 1,
                        "output": "error",
                    }
                ],
            }
            errors_json_path.write_text(json.dumps(mock_data))
            res = fint_build.parse_ninja_failures(errors_json_path)
            self.assertEqual(res, "FAILED: [code=1] bin/foo\n\nerror")


class RunGnCheckTest(unittest.TestCase):
    """Tests the run_gn_check helper function."""

    @mock.patch.object(subprocess, "run")
    def test_run_gn_check_success(self, mock_run: MagicMock) -> None:
        """Verifies run_gn_check returns 0 on success."""
        mock_run.return_value.returncode = 0
        host = fint_build.HostProperties(os="linux", cpu="x64")
        status = fint_build.run_gn_check(
            pathlib.Path("fake_checkout"), pathlib.Path("fake_build"), host
        )
        self.assertEqual(status, 0)
        mock_run.assert_called_once()

    @mock.patch.object(subprocess, "run")
    def test_run_gn_check_failure(self, mock_run: MagicMock) -> None:
        """Verifies run_gn_check returns non-zero on failure."""
        mock_run.return_value.returncode = 12
        host = fint_build.HostProperties(os="linux", cpu="x64")
        status = fint_build.run_gn_check(
            pathlib.Path("fake_checkout"), pathlib.Path("fake_build"), host
        )
        self.assertEqual(status, 12)


class CheckNinjaNoopTest(unittest.TestCase):
    """Tests the check_ninja_noop helper function."""

    @mock.patch.object(subprocess, "run")
    def test_check_ninja_noop_converges(self, mock_run: MagicMock) -> None:
        """Verifies check_ninja_noop returns 0 if ninja converges."""
        mock_run.return_value.returncode = 0
        mock_run.return_value.stdout = (
            "ninja: Entering directory ...\nninja: no work to do."
        )
        host = fint_build.HostProperties(os="linux", cpu="x64")
        status = fint_build.check_ninja_noop(
            pathlib.Path("fake_checkout"),
            pathlib.Path("fake_build"),
            host,
            [],
        )
        self.assertEqual(status, 0)

    @mock.patch.object(subprocess, "run")
    def test_check_ninja_noop_diverges(self, mock_run: MagicMock) -> None:
        """Verifies check_ninja_noop returns non-zero if ninja diverges and handles dirty sources."""

        def mock_run_impl(
            cmd: list[str], *args: Any, **kwargs: Any
        ) -> MagicMock:
            # Locate `--dirty_sources_list` and write simulated dirty files into its path
            for i, arg in enumerate(cmd):
                if arg == "--dirty_sources_list" and i + 1 < len(cmd):
                    path = cmd[i + 1]
                    pathlib.Path(path).write_text("sdk/lib/fdio/private.h\n")
            return MagicMock(
                returncode=0,
                stdout="ninja: Entering directory ...\nbuilding target foo.o",
            )

        mock_run.side_effect = mock_run_impl
        host = fint_build.HostProperties(os="linux", cpu="x64")
        status = fint_build.check_ninja_noop(
            pathlib.Path("fake_checkout"),
            pathlib.Path("fake_build"),
            host,
            [],
        )
        self.assertEqual(status, 1)

    @mock.patch.object(subprocess, "run")
    def test_check_ninja_noop_subprocess_fails(
        self, mock_run: MagicMock
    ) -> None:
        """Verifies check_ninja_noop returns ninja's exit code if subprocess fails."""
        mock_run.return_value.returncode = 2
        mock_run.return_value.stdout = "ninja: error: ..."
        host = fint_build.HostProperties(os="linux", cpu="x64")
        status = fint_build.check_ninja_noop(
            pathlib.Path("fake_checkout"),
            pathlib.Path("fake_build"),
            host,
            [],
        )
        self.assertEqual(status, 2)


class HostPropertiesTest(unittest.TestCase):
    """Tests the HostProperties domain model detection, properties, and matching."""

    def test_host_properties_detection_and_matching(self) -> None:
        """Verifies HostProperties attributes, properties, and matches_tool method."""
        host = fint_build.HostProperties(os="linux", cpu="x64")
        self.assertEqual(host.os, "linux")
        self.assertEqual(host.cpu, "x64")
        self.assertTrue(host.is_linux)
        self.assertFalse(host.is_mac)
        self.assertEqual(host.platform_dir, "linux-x64")
        self.assertEqual(
            host.gn_relative_path,
            pathlib.Path("prebuilt/third_party/gn/linux-x64/gn"),
        )
        self.assertEqual(
            host.ninja_relative_path,
            pathlib.Path("prebuilt/third_party/ninja/linux-x64/ninja"),
        )

        # Test matches_tool
        tool_matching = {"os": "linux", "cpu": "x64", "path": "path/to/tool"}
        tool_mismatch_os = {"os": "mac", "cpu": "x64", "path": "path/to/tool"}
        tool_mismatch_cpu = {
            "os": "linux",
            "cpu": "arm64",
            "path": "path/to/tool",
        }

        self.assertTrue(host.matches_tool(tool_matching))
        self.assertFalse(host.matches_tool(tool_mismatch_os))
        self.assertFalse(host.matches_tool(tool_mismatch_cpu))


class MainArgParserTest(unittest.TestCase):
    """Tests the command-line argument parser construction and parsing validation."""

    def test_main_arg_parser(self) -> None:
        """Verifies that the command-line argument parser works as expected."""
        parser = fint_build._main_arg_parser()
        args = parser.parse_args(
            [
                "--static",
                "/path/to/static",
                "--context",
                "/path/to/context",
                "--",
                "ninja",
                "target",
            ]
        )
        self.assertEqual(args.static, pathlib.Path("/path/to/static"))
        self.assertEqual(args.context, pathlib.Path("/path/to/context"))
        self.assertEqual(args.wrapped_cmd, ["ninja", "target"])


class NinjaBuildWrapTest(unittest.TestCase):
    """Tests target resolution, lifecycles, and bazel builds inside wrap_ninja and _get_targets."""

    def test_resolve_targets_default(self) -> None:
        """Verifies target resolution returns :default when requested."""
        static_spec = static_pb2.Static(include_default_ninja_target=True)
        context_spec = context_pb2.Context(
            checkout_dir="fake_checkout", build_dir="fake_dir"
        )

        host = fint_build.HostProperties(os="linux", cpu="x64")
        ctx = fint_build.BuildContext(static_spec, context_spec, host)
        targets = ctx._get_targets()
        self.assertEqual(targets, [":default"])

    def test_resolve_targets_custom_ninja(self) -> None:
        """Verifies custom targets are correctly appended and resolved."""
        static_spec = static_pb2.Static(ninja_targets=["foo", "bar"])
        context_spec = context_pb2.Context(
            checkout_dir="fake_checkout", build_dir="fake_dir"
        )

        host = fint_build.HostProperties(os="linux", cpu="x64")
        ctx = fint_build.BuildContext(static_spec, context_spec, host)
        targets = ctx._get_targets()
        self.assertEqual(targets, ["bar", "foo"])

    def test_resolve_targets_host_tests(self) -> None:
        """Verifies that host test targets are correctly filtered and resolved."""
        static_spec = static_pb2.Static(include_host_tests=True)
        with tempfile.TemporaryDirectory() as tmp_dir:
            context_spec = context_pb2.Context(
                checkout_dir="fake_checkout", build_dir=tmp_dir
            )

            test_specs_path = os.path.join(tmp_dir, fint_build.TESTS_JSON)
            test_specs_data = [
                {"test": {"os": "fuchsia", "path": "fuchsia_test"}},
                {"test": {"os": "linux", "path": "host_test_1"}},
                {"test": {"os": "mac", "path": "host_test_2"}},
            ]
            with open(test_specs_path, "w") as f:
                json.dump(test_specs_data, f)

            host = fint_build.HostProperties(os="linux", cpu="x64")
            ctx = fint_build.BuildContext(static_spec, context_spec, host)
            targets = ctx._get_targets()
            self.assertEqual(targets, ["host_test_1", "host_test_2"])

    @mock.patch.object(subprocess, "run")
    def test_lifecycle_context_manager(self, mock_run: MagicMock) -> None:
        """Verifies wrap_ninja touches files and manages success stamp correctly."""
        with tempfile.TemporaryDirectory() as tmp_dir:
            static_spec = static_pb2.Static(incremental=True)
            context_spec = context_pb2.Context(
                checkout_dir="fake_checkout", build_dir=tmp_dir
            )

            stamp_path = os.path.join(
                tmp_dir, fint_build.LAST_NINJA_BUILD_SUCCESS_STAMP
            )
            with open(stamp_path, "w") as f:
                f.write("old-content")

            host = fint_build.HostProperties(os="linux", cpu="x64")
            ctx = fint_build.BuildContext(static_spec, context_spec, host)
            with ctx.wrap_ninja(["ninja"]) as run:
                # Entering should remove the success stamp
                self.assertFalse(os.path.exists(stamp_path))

                # Rebuild sentinel should be touched on incremental builds
                sentinel = os.path.join(
                    tmp_dir, fint_build.FORCE_NONHERMETIC_REBUILD_SENTINEL
                )
                self.assertTrue(os.path.exists(sentinel))

                run.exit_code = 0

            # Exiting with success should write a new success stamp
            self.assertTrue(os.path.exists(stamp_path))

    @mock.patch.object(subprocess, "run")
    def test_build_bazel_host_tests(self, mock_run: MagicMock) -> None:
        """Verifies that Bazel host tests are correctly built when present."""
        with tempfile.TemporaryDirectory() as checkout_dir:
            # Create the bazel_top_dir configuration
            config_dir = os.path.join(checkout_dir, "build", "bazel", "config")
            os.makedirs(config_dir, exist_ok=True)
            with open(os.path.join(config_dir, "bazel_top_dir"), "w") as f:
                f.write("custom/bazel/dir\n")

            with tempfile.TemporaryDirectory() as build_dir:
                # Create the mock bazel launcher binary
                bazel_launcher_dir = os.path.join(
                    build_dir, "custom", "bazel", "dir"
                )
                os.makedirs(bazel_launcher_dir, exist_ok=True)
                bazel_launcher_path = os.path.join(bazel_launcher_dir, "bazel")
                with open(bazel_launcher_path, "w") as f:
                    pass

                # Write actual tests.json
                test_specs_data = [
                    {
                        "test": {
                            "label": "@//src/foo:foo_test",
                            "path": "bazel-out/foo",
                        }
                    },
                    {
                        "test": {
                            "label": "@//src/bar:bar_test",
                            "path": "bazel-out/bar",
                        }
                    },
                    {
                        "test": {
                            "label": "//src/gn:gn_test",
                            "path": "host_x64/gn_test",
                        }
                    },
                ]
                with open(
                    os.path.join(build_dir, fint_build.TESTS_JSON), "w"
                ) as f:
                    json.dump(test_specs_data, f)

                static_spec = static_pb2.Static()
                context_spec = context_pb2.Context(
                    checkout_dir=checkout_dir, build_dir=build_dir
                )
                host = fint_build.HostProperties(os="linux", cpu="x64")
                ctx = fint_build.BuildContext(static_spec, context_spec, host)

                ctx._build_bazel_host_tests()

                # Verify that subprocess.run was called to build the @ tests
                mock_run.assert_called_once_with(
                    [
                        bazel_launcher_path,
                        "build",
                        "--config=host",
                        "--build_runfile_links=true",
                        "--enable_runfiles=true",
                        "@//src/foo:foo_test",
                        "@//src/bar:bar_test",
                    ],
                    check=True,
                )


class MainExecutionTest(unittest.TestCase):
    """Integration tests for the wrapper main() execution flow."""

    @mock.patch.object(subprocess, "run")
    @mock.patch.object(signal_utils, "SignalManagedProcess")
    def test_main_writes_build_artifacts_json(
        self, mock_managed: MagicMock, mock_run: MagicMock
    ) -> None:
        """Verifies that main writes build_artifacts.json when artifact_dir is present."""
        with tempfile.TemporaryDirectory() as artifact_dir, tempfile.TemporaryDirectory() as build_dir:
            with tempfile.NamedTemporaryFile(
                mode="w", suffix=".textproto", delete=False
            ) as static_file:
                static_file.write("incremental: true")
                static_path = static_file.name

            with tempfile.NamedTemporaryFile(
                mode="w", suffix=".textproto", delete=False
            ) as context_file:
                context_file.write(
                    f'checkout_dir: "fake_checkout"\nartifact_dir: "{artifact_dir}"\nbuild_dir: "{build_dir}"'
                )
                context_path = context_file.name

            try:
                # Mock successful build command execution
                mock_managed.return_value.run.return_value = 0
                mock_run.return_value.returncode = 0
                mock_run.return_value.stdout = "ninja: no work to do."

                real_argv = [
                    "--static",
                    static_path,
                    "--context",
                    context_path,
                    "--",
                    "ninja",
                    "target",
                ]

                # Run main() passing arguments directly
                exit_code = fint_build.main(real_argv)
                self.assertEqual(exit_code, 0)

                # Verify build_artifacts.json was written to the artifact directory
                manifest_path = os.path.join(
                    artifact_dir, fint_build.BUILD_ARTIFACTS_JSON
                )
                self.assertTrue(os.path.exists(manifest_path))

                # Load and verify content
                with open(manifest_path, "r") as f:
                    manifest_content = json.loads(f.read())
                self.assertIn("ninjaDurationSeconds", manifest_content)

            finally:
                os.unlink(static_path)
                os.unlink(context_path)

    @mock.patch.object(subprocess, "run")
    @mock.patch.object(signal_utils, "SignalManagedProcess")
    def test_main_writes_build_artifacts_json_with_ninja_error_logging_output(
        self, mock_managed: MagicMock, mock_run: MagicMock
    ) -> None:
        """Verifies that main writes build_artifacts.json with a parsed failure summary when ninja_error_logging_output is present."""
        with tempfile.TemporaryDirectory() as artifact_dir, tempfile.TemporaryDirectory() as build_dir:
            with tempfile.NamedTemporaryFile(
                mode="w", suffix=".textproto", delete=False
            ) as static_file:
                static_file.write("incremental: true")
                static_path = static_file.name

            with tempfile.NamedTemporaryFile(
                mode="w", suffix=".textproto", delete=False
            ) as context_file:
                context_file.write(
                    f'checkout_dir: "fake_checkout"\nartifact_dir: "{artifact_dir}"\nbuild_dir: "{build_dir}"'
                )
                context_path = context_file.name

            errors_json_path = os.path.join(
                build_dir, fint_build.NINJA_ERRORS_JSON
            )
            mock_errors_data = {
                "version": 1,
                "failures": [
                    {
                        "artifacts": ["out/default/foo.o"],
                        "exit_code": 1,
                        "output": "FAILED: foo.o\nsome compiler error output\nline 2",
                    }
                ],
            }
            with open(errors_json_path, "w") as f:
                json.dump(mock_errors_data, f)

            try:
                # Mock failed build command execution
                mock_managed.return_value.run.return_value = 1
                mock_run.return_value.returncode = 0
                mock_run.return_value.stdout = "ninja: build failed."

                real_argv = [
                    "--static",
                    static_path,
                    "--context",
                    context_path,
                    "--ninja-error-logging-output",
                    errors_json_path,
                    "--",
                    "ninja",
                    "target",
                ]

                # Run main() passing arguments directly
                exit_code = fint_build.main(real_argv)
                self.assertEqual(exit_code, 1)

                # Verify build_artifacts.json was written to the artifact directory
                manifest_path = os.path.join(
                    artifact_dir, fint_build.BUILD_ARTIFACTS_JSON
                )
                self.assertTrue(os.path.exists(manifest_path))

                # Load and verify content
                with open(manifest_path, "r") as f:
                    manifest_content = json.loads(f.read())
                self.assertIn("ninjaDurationSeconds", manifest_content)
                self.assertIn(
                    "FAILED: [code=1] out/default/foo.o",
                    manifest_content["failureSummary"],
                )
                self.assertIn(
                    "some compiler error output",
                    manifest_content["failureSummary"],
                )

            finally:
                os.unlink(static_path)
                os.unlink(context_path)

    @mock.patch.object(subprocess, "run")
    @mock.patch.object(signal_utils, "SignalManagedProcess")
    @mock.patch.object(fint_build, "load_json_list")
    @mock.patch.object(pathlib.Path, "cwd")
    def test_main_omitted_context(
        self,
        mock_cwd: MagicMock,
        mock_load_json_list: MagicMock,
        mock_managed: MagicMock,
        mock_run: MagicMock,
    ) -> None:
        """Verifies that omitting --context automatically generates context_spec."""
        mock_load_json_list.return_value = []
        with tempfile.TemporaryDirectory() as tmp_dir:
            mock_cwd.return_value = pathlib.Path(tmp_dir)
            with tempfile.NamedTemporaryFile(
                mode="w", suffix=".textproto", delete=False
            ) as static_file:
                static_file.write("")
                static_path = static_file.name

            try:
                # Mock successful build command execution
                mock_managed.return_value.run.return_value = 0
                mock_run.return_value.returncode = 0
                mock_run.return_value.stdout = "ninja: no work to do."

                real_argv = [
                    "--static",
                    static_path,
                    "--",
                    "ninja",
                    "-j",
                    "64",
                    "target",
                ]

                # Run main() passing arguments directly without --context
                exit_code = fint_build.main(real_argv)
                self.assertEqual(exit_code, 0)

            finally:
                os.unlink(static_path)

    @mock.patch.object(signal_utils, "SignalManagedProcess")
    @mock.patch.object(fint_build.BuildContext, "wrap_bazel")
    @mock.patch.object(fint_build.BuildContext, "wrap_ninja")
    @mock.patch.object(fint_build, "make_build_context")
    def test_main_selects_wrapper_ninja(
        self,
        mock_make_ctx: MagicMock,
        mock_wrap_ninja: MagicMock,
        mock_wrap_bazel: MagicMock,
        mock_managed: MagicMock,
    ) -> None:
        """Verifies that main() selects wrap_ninja in default or ninja mode."""
        # Setup mock context to be a real BuildContext instance
        static_spec = static_pb2.Static()
        context_spec = context_pb2.Context()
        host = fint_build.HostProperties(os="linux", cpu="x64")
        ctx = fint_build.BuildContext(static_spec, context_spec, host)
        mock_make_ctx.return_value = ctx

        # Mock successful subprocess execution
        mock_managed.return_value.run.return_value = 0

        @contextlib.contextmanager
        def fake_wrap_ninja(
            wrapped_cmd: list[str],
        ) -> Generator[fint_build.BuildExecution, None, None]:
            yield fint_build.BuildExecution(command=["ninja"])

        mock_wrap_ninja.side_effect = fake_wrap_ninja

        fint_build.main(["--static", "fake_static", "--", "ninja"])
        mock_wrap_ninja.assert_called_once()
        mock_wrap_bazel.assert_not_called()

    @mock.patch.object(signal_utils, "SignalManagedProcess")
    @mock.patch.object(fint_build.BuildContext, "wrap_bazel")
    @mock.patch.object(fint_build.BuildContext, "wrap_ninja")
    @mock.patch.object(fint_build, "make_build_context")
    def test_main_selects_wrapper_bazel(
        self,
        mock_make_ctx: MagicMock,
        mock_wrap_ninja: MagicMock,
        mock_wrap_bazel: MagicMock,
        mock_managed: MagicMock,
    ) -> None:
        """Verifies that main() selects wrap_bazel in bazel mode."""
        # Setup mock context to be a real BuildContext instance
        static_spec = static_pb2.Static()
        context_spec = context_pb2.Context()
        host = fint_build.HostProperties(os="linux", cpu="x64")
        ctx = fint_build.BuildContext(static_spec, context_spec, host)
        mock_make_ctx.return_value = ctx

        # Mock successful subprocess execution
        mock_managed.return_value.run.return_value = 0

        @contextlib.contextmanager
        def fake_wrap_bazel(
            wrapped_cmd: list[str],
        ) -> Generator[fint_build.BuildExecution, None, None]:
            yield fint_build.BuildExecution(command=["bazel"])

        mock_wrap_bazel.side_effect = fake_wrap_bazel

        fint_build.main(
            ["--static", "fake_static", "--mode", "bazel", "--", "bazel"]
        )
        mock_wrap_bazel.assert_called_once()
        mock_wrap_ninja.assert_not_called()


class FintBuildMainTest(unittest.TestCase):
    """Tests for the main() function entrypoint of fint_build.py."""

    def test_main_print_artifact_dir_success(self) -> None:
        """Verifies that --print-artifact-dir outputs the correct directory."""
        with tempfile.TemporaryDirectory() as tmp_dir:
            tmp_path = pathlib.Path(tmp_dir)
            context_file = tmp_path / "context.textproto"
            context_file.write_text('artifact_dir: "/mock/artifact/dir"\n')

            # Redirect stdout to capture the printed path
            f_stdout = io.StringIO()
            with contextlib.redirect_stdout(f_stdout):
                exit_code = fint_build.main(
                    ["--context", str(context_file), "--print-artifact-dir"]
                )

            self.assertEqual(exit_code, 0)
            self.assertEqual(f_stdout.getvalue().strip(), "/mock/artifact/dir")

    def test_main_print_artifact_dir_missing_context(self) -> None:
        """Verifies that using --print-artifact-dir without --context returns an error."""
        f_stderr = io.StringIO()
        with contextlib.redirect_stderr(f_stderr):
            exit_code = fint_build.main(["--print-artifact-dir"])

        self.assertEqual(exit_code, 1)
        self.assertIn(
            "Error: --context is required with --print-artifact-dir",
            f_stderr.getvalue(),
        )

    def test_main_missing_required_args(self) -> None:
        """Verifies that missing static or wrapped_cmd returns 2."""
        # 1. Missing static
        f_stderr = io.StringIO()
        with contextlib.redirect_stderr(f_stderr):
            exit_code = fint_build.main(["--static", "", "--", "ninja"])
        self.assertEqual(exit_code, 2)
        self.assertIn(
            "Error: the following arguments are required: --static",
            f_stderr.getvalue(),
        )

        # 2. Missing wrapped_cmd
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".textproto", delete=False
        ) as f:
            f.write("")
            static_path = f.name
        try:
            f_stderr = io.StringIO()
            with contextlib.redirect_stderr(f_stderr):
                exit_code = fint_build.main(["--static", static_path])
            self.assertEqual(exit_code, 2)
            self.assertIn(
                "Error: wrapped_cmd is required for build execution",
                f_stderr.getvalue(),
            )
        finally:
            os.unlink(static_path)


class TimerTest(unittest.TestCase):
    """Tests the Timer context manager."""

    def test_timer_measures_duration(self) -> None:
        """Verifies that the Timer correctly measures elapsed time."""
        with mock.patch.object(time, "time") as mock_time:
            # Simulate passage of 5.5 seconds
            mock_time.side_effect = [100.0, 105.5]
            with fint_build.Timer() as t:
                pass
            self.assertEqual(t.duration, 5.5)


class MakeBuildContextTest(unittest.TestCase):
    """Tests the make_build_context helper function."""

    @mock.patch.object(fint_build, "load_static_spec")
    @mock.patch.object(fint_build, "load_context_spec")
    @mock.patch.object(fint_build.HostProperties, "detect")
    def test_make_build_context_with_context_path(
        self,
        mock_detect: MagicMock,
        mock_load_context: MagicMock,
        mock_load_static: MagicMock,
    ) -> None:
        """Verifies make_build_context loads both specs when context path is provided."""
        mock_detect.return_value = fint_build.HostProperties(
            os="linux", cpu="x64"
        )
        static_path = pathlib.Path("fake_static.textproto")
        context_path = pathlib.Path("fake_context.textproto")

        ctx = fint_build.make_build_context(static_path, context_path)

        mock_load_static.assert_called_once_with(static_path)
        mock_load_context.assert_called_once_with(context_path)
        self.assertEqual(ctx.host.os, "linux")

    @mock.patch.object(fint_build, "load_static_spec")
    @mock.patch.object(fint_build.HostProperties, "detect")
    @mock.patch.object(pathlib.Path, "cwd")
    def test_make_build_context_omitted_context_path(
        self,
        mock_cwd: MagicMock,
        mock_detect: MagicMock,
        mock_load_static: MagicMock,
    ) -> None:
        """Verifies make_build_context generates a fallback context spec when context path is omitted."""
        mock_detect.return_value = fint_build.HostProperties(
            os="linux", cpu="x64"
        )
        mock_cwd.return_value = pathlib.Path("/mock/cwd")
        static_path = pathlib.Path("fake_static.textproto")

        ctx = fint_build.make_build_context(static_path, None)

        mock_load_static.assert_called_once_with(static_path)
        self.assertEqual(
            ctx.context_spec.checkout_dir,
            str(fint_build.fuchsia_root.resolve()),
        )
        self.assertEqual(ctx.context_spec.build_dir, "/mock/cwd")


if __name__ == "__main__":
    unittest.main()
