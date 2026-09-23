# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import tempfile
import unittest
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(_SCRIPT_DIR))
import ninja_artifacts

sys.path.insert(1, str(_SCRIPT_DIR / "../bazel/scripts"))
from build_utils import MockNinjaRunner


class NinjaArtifactsTest(unittest.TestCase):
    def test_get_last_build_targets(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            build_dir = Path(temp_dir)

            # If the file doesn't exist, default to [":default"].
            self.assertListEqual(
                ninja_artifacts.get_last_build_targets(build_dir), [":default"]
            )

            # If the file is empty, default to [":default"] too.
            (build_dir / ninja_artifacts.LAST_NINJA_TARGETS_FILE).write_text("")
            self.assertListEqual(
                ninja_artifacts.get_last_build_targets(build_dir), [":default"]
            )

            (build_dir / ninja_artifacts.LAST_NINJA_TARGETS_FILE).write_text(
                " foo"
            )
            self.assertListEqual(
                ninja_artifacts.get_last_build_targets(build_dir), ["foo"]
            )

            (build_dir / ninja_artifacts.LAST_NINJA_TARGETS_FILE).write_text(
                "foo bar"
            )
            self.assertListEqual(
                ninja_artifacts.get_last_build_targets(build_dir),
                ["foo", "bar"],
            )

    def test_get_build_plan_deps(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            build_dir = Path(temp_dir)

            (build_dir / ninja_artifacts.NINJA_BUILD_PLAN_DEPS_FILE).write_text(
                "build.ninja.stamp: dep1 dep2 dep3 dep4\n"
            )

            self.assertListEqual(
                ninja_artifacts.get_build_plan_deps(build_dir),
                ["dep1", "dep2", "dep3", "dep4"],
            )

    def test_check_output_needs_update(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            build_dir = Path(temp_dir)

            input_files = [build_dir / "input1", build_dir / "input2"]
            output_file = build_dir / "output"

            # output file does not exist, nor any input file.
            self.assertTrue(
                ninja_artifacts.check_output_needs_update(
                    output_file, input_files
                )
            )

            # output file does not exist, but input files do.
            input_files[0].write_text("one")
            input_files[1].write_text("two")
            self.assertTrue(
                ninja_artifacts.check_output_needs_update(
                    output_file, input_files
                )
            )

            # output file does exist, and is newer than inputs.
            output_file.write_text("out")
            self.assertFalse(
                ninja_artifacts.check_output_needs_update(
                    output_file, input_files
                )
            )

            # output file does exist, but is older than one input.
            output_stat = output_file.stat()
            os.utime(
                input_files[1],
                times=(output_stat.st_atime, output_stat.st_mtime + 1),
            )
            self.assertTrue(
                ninja_artifacts.check_output_needs_update(
                    output_file, input_files
                )
            )

    def test_get_last_build_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            # Setup fake source and build directory.
            build_gn_path = Path(temp_dir) / "BUILD.gn"
            build_gn_path.write_text("# Fake BUILD.gn\n")

            build_dir = Path(temp_dir) / "out"
            build_dir.mkdir(parents=True)

            build_ninja_d_path = (
                build_dir / ninja_artifacts.NINJA_BUILD_PLAN_DEPS_FILE
            )
            build_ninja_d_path.write_text("build.ninja.stamp: ../BUILD.gn")

            last_targets_path = (
                build_dir / ninja_artifacts.LAST_NINJA_TARGETS_FILE
            )
            last_targets_path.write_text("foo")

            # Create mock NinjaRunner instance to avoid calling Ninja binary.
            ninja_runner = MockNinjaRunner(
                build_dir, "bar\nfoo\nzoo\n'quoted'\n"
            )
            self.assertListEqual(
                ninja_artifacts.get_last_build_artifacts(ninja_runner),
                ["bar", "foo", "zoo", "'quoted'"],
            )
            self.assertListEqual(
                ninja_runner.last_ninja_args(), ["-t", "outputs", "foo"]
            )

            last_ninja_artifacts_path = (
                build_dir / ninja_artifacts.LAST_NINJA_ARTIFACTS_FILE
            )
            self.assertTrue(last_ninja_artifacts_path.exists())
            self.assertEqual(
                last_ninja_artifacts_path.read_text(), "bar\nfoo\nzoo\n'quoted'"
            )

            # Modify last_ninja_build_targets.txt and verify the cache was regenerated.

            last_targets_path.write_text("bar zoo")
            last_targets_stat = last_targets_path.stat()
            os.utime(
                last_targets_path,
                times=(
                    last_targets_stat.st_atime,
                    last_targets_stat.st_mtime + 1,
                ),
            )

            ninja_runner = MockNinjaRunner(build_dir, "second\ncall\n")
            self.assertListEqual(
                ninja_artifacts.get_last_build_artifacts(ninja_runner),
                ["second", "call"],
            )
            self.assertListEqual(
                ninja_runner.last_ninja_args(), ["-t", "outputs", "bar", "zoo"]
            )
            self.assertEqual(
                last_ninja_artifacts_path.read_text(), "second\ncall"
            )

    def test_get_last_build_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            # Setup fake source and build directory.
            build_gn_path = Path(temp_dir) / "BUILD.gn"
            build_gn_path.write_text("# Fake BUILD.gn\n")

            build_dir = Path(temp_dir) / "out"
            build_dir.mkdir(parents=True)

            build_ninja_d_path = (
                build_dir / ninja_artifacts.NINJA_BUILD_PLAN_DEPS_FILE
            )
            build_ninja_d_path.write_text("build.ninja.stamp: ../BUILD.gn")

            last_targets_path = (
                build_dir / ninja_artifacts.LAST_NINJA_TARGETS_FILE
            )
            last_targets_path.write_text("foo")

            # Create mock NinjaRunner instance to avoid calling Ninja binary.
            ninja_runner = MockNinjaRunner(
                build_dir,
                "../src/foo\n../src/bar\noutput_file\nout_dir/out_file\n../src/zoo\n",
            )
            self.assertListEqual(
                ninja_artifacts.get_last_build_sources(ninja_runner),
                ["../src/foo", "../src/bar", "../src/zoo"],
            )
            self.assertListEqual(
                ninja_runner.last_ninja_args(),
                [
                    "-t",
                    "inputs",
                    "--no-shell-escape",
                    "--dependency-order",
                    "foo",
                ],
            )

            last_ninja_sources_path = (
                build_dir / ninja_artifacts.LAST_NINJA_SOURCES_FILE
            )
            self.assertTrue(last_ninja_sources_path.exists())
            self.assertEqual(
                last_ninja_sources_path.read_text(),
                "../src/foo\n../src/bar\n../src/zoo",
            )

            # Modify last_ninja_build_targets.txt and verify the cache was regenerated.

            last_targets_path.write_text("bar zoo")
            last_targets_stat = last_targets_path.stat()
            os.utime(
                last_targets_path,
                times=(
                    last_targets_stat.st_atime,
                    last_targets_stat.st_mtime + 1,
                ),
            )

            ninja_runner = MockNinjaRunner(build_dir, "../second\n../call\n")
            self.assertListEqual(
                ninja_artifacts.get_last_build_sources(ninja_runner),
                ["../second", "../call"],
            )
            self.assertListEqual(
                ninja_runner.last_ninja_args(),
                [
                    "-t",
                    "inputs",
                    "--no-shell-escape",
                    "--dependency-order",
                    "bar",
                    "zoo",
                ],
            )
            self.assertEqual(
                last_ninja_sources_path.read_text(), "../second\n../call"
            )


class ShouldChangedFilesTriggerBuildTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.root = Path(self._td.name)
        self.build_dir = self.root / "out/build"
        self.build_dir.mkdir(parents=True)

        (
            self.build_dir / ninja_artifacts.NINJA_BUILD_PLAN_DEPS_FILE
        ).write_text(
            "build.ninja.stamp: ../../BUILD.gn ../../src/foo.gni dep1 dep2 dep3 dep4"
        )

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_no_change(self) -> None:
        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["some/file.txt"], self.root, MockNinjaRunner(self.build_dir, "")
        )
        self.assertFalse(result)
        self.assertEqual(reason, "")

    def test_build_file_changes(self) -> None:
        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["BUILD.gn"],
            self.root,
            MockNinjaRunner(self.build_dir, ""),
        )
        self.assertEqual(reason, "GN build graph changed.")
        self.assertTrue(result)

        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["src/foo.gni"],
            self.root,
            MockNinjaRunner(self.build_dir, ""),
        )
        self.assertEqual(reason, "GN build graph changed.")
        self.assertTrue(result)

        MockNinjaRunner(self.build_dir, "")
        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["other/BUILD.gn", "src/bar.gni"],
            self.root,
            MockNinjaRunner(self.build_dir, ""),
        )
        self.assertEqual(reason, "")
        self.assertFalse(result)

    def test_source_file_changes(self) -> None:
        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["some/file.txt"],
            self.root,
            MockNinjaRunner(self.build_dir, ":default\t../../some/file.txt"),
        )
        self.assertEqual(reason, "Sources updated for target: :default")
        self.assertTrue(result)

        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["some/file.txt"],
            self.root,
            MockNinjaRunner(
                self.build_dir, ":default\t../../some/other_file.txt"
            ),
        )
        self.assertEqual(reason, "")
        self.assertFalse(result)

        # Simulate a previous build of 'foo' instead of ':default' and verify that
        # only when related source have change does
        (self.build_dir / ninja_artifacts.LAST_NINJA_TARGETS_FILE).write_text(
            "foo bar"
        )

        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["some/file.txt"],
            self.root,
            MockNinjaRunner(
                self.build_dir,
                "foo\t../../src/foo.cc\nfoo\t../../src/foo.h\nbar\t../../src/bar.cc\n",
            ),
        )
        self.assertEqual(reason, "")
        self.assertFalse(result)

        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["src/foo.h", "src/qux.cc"],
            self.root,
            MockNinjaRunner(
                self.build_dir,
                "foo\t../../src/foo.cc\nfoo\t../../src/foo.h\nbar\t../../src/bar.cc\n",
            ),
        )
        self.assertEqual(reason, "Sources updated for target: foo")
        self.assertTrue(result)

        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["src/foo.cc", "src/bar.cc"],
            self.root,
            MockNinjaRunner(
                self.build_dir,
                "foo\t../../src/foo.cc\nfoo\t../../src/foo.h\nbar\t../../src/bar.cc\n",
            ),
        )
        self.assertEqual(reason, "Sources updated for 2 targets.")
        self.assertTrue(result)

    def test_root_targets(self) -> None:
        # If root_targets is not specific, verify that the fallback ":default" is used
        # when invoking the Ninja tool.
        mock_runner = MockNinjaRunner(
            self.build_dir, ":default\t../../some/file.txt"
        )
        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["some/file.txt"], self.root, mock_runner
        )
        self.assertListEqual(
            mock_runner.last_ninja_args(),
            ["-t", "multi-inputs", "--depfile", ":default"],
        )

        mock_runner = MockNinjaRunner(
            self.build_dir,
            ":first\t../../some/file.txt\n:second\t../../src.foo.cc",
        )
        result, reason = ninja_artifacts.should_file_changes_trigger_build(
            ["some/file.txt"],
            self.root,
            mock_runner,
            root_targets=[":first", ":second"],
        )
        self.assertListEqual(
            mock_runner.last_ninja_args(),
            ["-t", "multi-inputs", "--depfile", ":first", ":second"],
        )


if __name__ == "__main__":
    unittest.main()
