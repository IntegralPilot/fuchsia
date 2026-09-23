# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from unittest import mock

import search_tests


class PreserveEnvAndCaptureOutputTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self._old_fuchsia_dir = os.getenv("FUCHSIA_DIR")
        self.stdout = io.StringIO()
        self._context = contextlib.redirect_stdout(self.stdout)
        self._context.__enter__()

    def tearDown(self) -> None:
        if self._old_fuchsia_dir:
            os.environ["FUCHSIA_DIR"] = self._old_fuchsia_dir
        self._context.__exit__(None, None, None)
        return super().tearDown()


class TestSearchLocations(PreserveEnvAndCaptureOutputTestCase):
    def test_environment_variable_unset(self) -> None:
        del os.environ["FUCHSIA_DIR"]

        with self.assertRaises(Exception) as ex:
            search_tests.create_search_locations(False, [])

        self.assertEqual(
            str(ex.exception), "Environment variable FUCHSIA_DIR must be set"
        )

    def test_not_a_file(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            path = os.path.join(dir, "tmpfile")
            with open(path, "w"):
                pass
            os.environ["FUCHSIA_DIR"] = str(path)

            with self.assertRaises(Exception) as ex:
                search_tests.create_search_locations(False, [])

            self.assertEqual(
                str(ex.exception), f"Path {path} should be a directory"
            )

    def test_missing_tests_json(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            with open(os.path.join(dir, ".fx-build-dir"), "w") as f:
                f.write("out/other")

            os.makedirs(os.path.join(dir, "out", "other"))

            os.environ["FUCHSIA_DIR"] = str(dir)

            with self.assertRaises(Exception) as ex:
                search_tests.create_search_locations(False, [])

            expected = os.path.join(dir, "out", "other", "tests.json")

            self.assertEqual(
                str(ex.exception),
                f"Expected to find a test list file at {expected}",
            )

    def test_success(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            with open(os.path.join(dir, ".fx-build-dir"), "w") as f:
                f.write("out/other")
            os.makedirs(os.path.join(dir, "out", "other"))
            with open(
                os.path.join(dir, "out", "other", "tests.json"), "w"
            ) as f:
                pass

            os.environ["FUCHSIA_DIR"] = str(dir)

            locations = search_tests.create_search_locations(False, [])
            self.assertEqual(locations.fuchsia_directory, dir)
            self.assertEqual(
                locations.tests_json_file,
                os.path.join(dir, "out", "other", "tests.json"),
            )
            self.assertNotEqual("", str(locations))

    @mock.patch("search_tests.Pool")
    @mock.patch("search_tests.subprocess.run")
    @mock.patch("search_tests.subprocess.check_output")
    def test_success_with_remote(
        self,
        mock_check_output: mock.MagicMock,
        mock_run: mock.MagicMock,
        mock_pool: mock.MagicMock,
    ) -> None:
        mock_check_output.return_value = b"fake-build-id-123\n"
        mock_run.side_effect = lambda *args, **kwargs: mock.MagicMock(
            spec=subprocess.CompletedProcess, returncode=0
        )
        mock_pool.return_value.__enter__.return_value.map.return_value = [
            "/fake/path/1",
            "/fake/path/2",
            "/fake/path/3",
            "/fake/path/4",
            "/fake/path/5",
        ]

        with tempfile.TemporaryDirectory() as dir:
            with open(os.path.join(dir, ".fx-build-dir"), "w") as f:
                f.write("out/other")
            os.makedirs(os.path.join(dir, "out", "other"))
            with open(
                os.path.join(dir, "out", "other", "tests.json"), "w"
            ) as f:
                pass

            os.environ["FUCHSIA_DIR"] = str(dir)

            locations = search_tests.create_search_locations(
                True, ["test_builder"]
            )
            self.assertEqual(5, len(locations.remote_tests_jsons))


class TestTestsFileMatcher(unittest.TestCase):
    def _write_names(self, dir: str, names: list[str | tuple[str, str]]) -> str:
        path = os.path.join(dir, "tests.json")
        with open(path, "w") as f:
            l = []
            if names and isinstance(names[0], str):
                l = [{"test": {"name": name}} for name in names]
            elif names and isinstance(names[0], tuple):
                l = [
                    {"test": {"name": value[0], "label": value[1]}}
                    for value in names
                ]
            json.dump(l, f)
        return path

    def test_empty_file(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            path = self._write_names(dir, [])
            tests_matcher = search_tests.TestsFileMatcher(path, False)
            matcher = search_tests.Matcher(threshold=0.75)
            self.assertEqual(tests_matcher.find_matches("foo", matcher), [])

    def test_exact_matches(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            path = self._write_names(
                dir,
                [
                    "fuchsia-pkg://fuchsia.com/my-package#meta/my-component.cm",
                    "host_test/my-host-test",
                ],
            )
            tests_matcher = search_tests.TestsFileMatcher(path, False)
            matcher = search_tests.Matcher(threshold=1)
            self.assertEqual(
                [
                    val.matched_name
                    for val in tests_matcher.find_matches(
                        "my_component", matcher
                    )
                ],
                ["my-component"],
            )
            self.assertEqual(
                [
                    val.matched_name
                    for val in tests_matcher.find_matches("my_package", matcher)
                ],
                ["my-package"],
            )
            self.assertEqual(
                [
                    val.matched_name
                    for val in tests_matcher.find_matches(
                        "my_host_test", matcher
                    )
                ],
                ["my-host-test"],
            )

    def test_labels(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            path = self._write_names(
                dir,
                [
                    (
                        "fuchsia-pkg://fuchsia.com/my-package#meta/my-component.cm",
                        "//src/sys:my_component",
                    ),
                ],
            )
            tests_matcher = search_tests.TestsFileMatcher(path, False)
            matcher = search_tests.Matcher(threshold=0.7)
            self.assertEqual(
                [
                    val.matched_name
                    for val in tests_matcher.find_matches("//src/sys", matcher)
                ],
                ["//src/sys:my_component"],
            )

    def test_remote_empty_labels(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            path = self._write_names(
                dir,
                [
                    "fuchsia-pkg://fuchsia.com/my-package#meta/my-component.cm",
                ],
            )
            # is_remote=True with tests.json having no labels field
            tests_matcher = search_tests.TestsFileMatcher(path, True)
            matcher = search_tests.Matcher(threshold=1)
            matches = tests_matcher.find_matches("my_component", matcher)
            self.assertEqual(len(matches), 1)
            self.assertEqual(matches[0].matched_name, "my-component")


TEST_PACKAGE = (
    lambda x: f"""
fuchsia_test_package("{x}") {{

}}
"""
)

TEST_PACKAGE_CUSTOM_NAME = (
    lambda name, x: f"""
{name}("{x}") {{

}}
"""
)

TEST_COMPONENT = (
    lambda x: f"""
fuchsia_test_component("{x}") {{

}}
"""
)

TEST_COMPONENT_WITH_COMPONENT_NAME = (
    lambda x, name: f"""
fuchsia_test_component("{x}") {{
    component_name = "{name}"
}}
"""
)

TEST_PACKAGE_WITH_PACKAGE_NAME = (
    lambda x, name: f"""
fuchsia_test_package("{x}") {{
    package_name = "{name}"
}}
"""
)

TEST_PACKAGE_WITH_COMPONENT_NAME = (
    lambda x, name: f"""
fuchsia_test_package("{x}") {{
    component_name = "{name}"
}}
"""
)

TEST_PACKAGE_WITH_TEST_COMPONENTS = (
    lambda x, components: f"""
fuchsia_test_package("{x}") {{
    test_components = [
        {",".join([f'"{name}"' for name in components])}
    ]
}}
"""
)

PYTHON_HOST_TEST = (
    lambda name: f"""
python_host_test("{name}") {{

}}
"""
)

HOST_TEST = (
    lambda name: f"""
host_test("{name}") {{

}}
"""
)

PYTHON_MOBLY_TEST = (
    lambda name: f"""
python_mobly_test("{name}") {{

}}
"""
)

PYTHON_PERF_TEST = (
    lambda name: f"""
python_perf_test("{name}") {{

}}
"""
)


class TestBuildFileMatcher(unittest.TestCase):
    def test_simple_packages(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            os.makedirs(os.path.join(dir, "src"))
            with open(os.path.join(dir, "src", "BUILD.gn"), "w") as f:
                f.write(TEST_PACKAGE("my-test-package"))
                f.write(
                    TEST_PACKAGE_WITH_PACKAGE_NAME(
                        "other-package", "other-real-name"
                    )
                )
                f.write(
                    TEST_PACKAGE_WITH_COMPONENT_NAME(
                        "yet-another-package", "yet-another-real-name"
                    )
                )

            build_matcher = search_tests.BuildFileMatcher(dir)
            matcher = search_tests.Matcher(threshold=1)

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches(
                        "my-test-package", matcher
                    )
                ],
                [("my-test-package", "fx add-test //src:my-test-package")],
            )

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches(
                        "other-real-name", matcher
                    )
                ],
                [("other-real-name", "fx add-test //src:other-package")],
            )

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches(
                        "yet-another-real-name", matcher
                    )
                ],
                [
                    (
                        "yet-another-real-name",
                        "fx add-test //src:yet-another-package",
                    )
                ],
            )

    def test_simple_labels(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            os.makedirs(os.path.join(dir, "src"))
            with open(os.path.join(dir, "src", "BUILD.gn"), "w") as f:
                f.write(TEST_PACKAGE("my-test-package"))
                f.write(
                    TEST_PACKAGE_WITH_PACKAGE_NAME(
                        "other-package", "other-real-name"
                    )
                )
                f.write(
                    TEST_PACKAGE_WITH_COMPONENT_NAME(
                        "yet-another-package", "yet-another-real-name"
                    )
                )

            build_matcher = search_tests.BuildFileMatcher(dir)
            matcher = search_tests.Matcher(threshold=0.4)

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches("//src", matcher)
                ],
                [
                    ("my-test-package", "fx add-test //src:my-test-package"),
                    ("other-real-name", "fx add-test //src:other-package"),
                    (
                        "yet-another-real-name",
                        "fx add-test //src:yet-another-package",
                    ),
                ],
            )

    def test_packages_with_components(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            os.makedirs(os.path.join(dir, "src", "nested"))
            with open(os.path.join(dir, "src", "nested", "BUILD.gn"), "w") as f:
                f.write(TEST_COMPONENT("my-test-component"))
                f.write(
                    TEST_COMPONENT_WITH_COMPONENT_NAME(
                        "another-component", "component-real-name"
                    )
                )
                f.write(
                    TEST_PACKAGE_WITH_TEST_COMPONENTS(
                        "test-package",
                        [":my-test-component", ":another-component"],
                    )
                )

            build_matcher = search_tests.BuildFileMatcher(dir)
            matcher = search_tests.Matcher(threshold=1)

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches(
                        "my_test_component", matcher
                    )
                ],
                [
                    (
                        "my-test-component",
                        "fx add-test //src/nested:test-package",
                    )
                ],
            )

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches(
                        "component_real_name", matcher
                    )
                ],
                [
                    (
                        "component-real-name",
                        "fx add-test //src/nested:test-package",
                    )
                ],
            )

    def test_custom_package_name(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            os.makedirs(os.path.join(dir, "src"))
            with open(os.path.join(dir, "src", "BUILD.gn"), "w") as f:
                f.write(
                    TEST_PACKAGE_CUSTOM_NAME(
                        "fuchsia_test_with_expectations_package",
                        "my-test-package",
                    )
                )
                f.write(
                    TEST_PACKAGE_CUSTOM_NAME(
                        "test_but_does_not_match", "other-package-name"
                    )
                )

            build_matcher = search_tests.BuildFileMatcher(dir)
            matcher = search_tests.Matcher(threshold=1)

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches(
                        "my-test-package", matcher
                    )
                ],
                [("my-test-package", "fx add-test //src:my-test-package")],
            )

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches(
                        "other-package-name", matcher
                    )
                ],
                [],
            )

    def test_host_tests(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            os.makedirs(os.path.join(dir, "src"))
            with open(os.path.join(dir, "src", "BUILD.gn"), "w") as f:
                f.write(HOST_TEST("my_host_test"))
                f.write(PYTHON_HOST_TEST("my_python_test"))
                f.write(PYTHON_MOBLY_TEST("my_mobly_test"))

            build_matcher = search_tests.BuildFileMatcher(dir)
            matcher = search_tests.Matcher(threshold=1)

            for expected_name in [
                "my_host_test",
                "my_python_test",
                "my_mobly_test",
            ]:
                self.assertEqual(
                    [
                        (val.matched_name, val.full_suggestion)
                        for val in build_matcher.find_matches(
                            expected_name, matcher
                        )
                    ],
                    [
                        (
                            expected_name,
                            f"fx add-host-test //src:{expected_name}",
                        )
                    ],
                )

    def test_developer_tests(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            os.makedirs(os.path.join(dir, "src"))
            with open(os.path.join(dir, "src", "BUILD.gn"), "w") as f:
                f.write(PYTHON_PERF_TEST("my_perf_test"))

            build_matcher = search_tests.BuildFileMatcher(dir)
            matcher = search_tests.Matcher(threshold=1)

            for expected_name in [
                "my_perf_test",
            ]:
                self.assertEqual(
                    [
                        (val.matched_name, val.full_suggestion)
                        for val in build_matcher.find_matches(
                            expected_name, matcher
                        )
                    ],
                    [
                        (
                            expected_name,
                            f"fx add-test //src:{expected_name}",
                        )
                    ],
                )

    def test_standard_test_with_host_toolchain(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            os.makedirs(os.path.join(dir, "src"))
            with open(os.path.join(dir, "src", "BUILD.gn"), "w") as f:
                f.write(
                    """
test("my_std_test") {
}
group("tests") {
  deps = [ ":my_std_test($host_toolchain)" ]
}
"""
                )

            build_matcher = search_tests.BuildFileMatcher(dir)
            matcher = search_tests.Matcher(threshold=1)

            self.assertEqual(
                [
                    (val.matched_name, val.full_suggestion)
                    for val in build_matcher.find_matches(
                        "my_std_test", matcher
                    )
                ],
                [("my_std_test", "fx add-host-test //src:my_std_test")],
            )


class TestTimingTracker(PreserveEnvAndCaptureOutputTestCase):
    def test_timing(self) -> None:
        search_tests.TimingTracker.reset()

        with search_tests.TimingTracker("Test timings"):
            pass
        with search_tests.TimingTracker("Test again"):
            pass

        search_tests.TimingTracker.print_timings()

        lines = self.stdout.getvalue().strip().split("\n")
        self.assertEqual(lines[0], "Debug timings:")
        self.assertRegex(lines[1], r"\s+Test timings\s+\d+\.\d\d\dms$")
        self.assertRegex(lines[2], r"\s+Test again\s+\d+\.\d\d\dms$")

        with search_tests.TimingTracker("In progress"):
            # Ensure we omit in progress readings
            search_tests.TimingTracker.print_timings()
            lines2 = self.stdout.getvalue().strip().split("\n")
            self.assertListEqual(lines, lines2[len(lines) :])


class TestCommand(PreserveEnvAndCaptureOutputTestCase):
    def setUp(self) -> None:
        super().setUp()
        self.dir = tempfile.TemporaryDirectory()
        with open(os.path.join(self.dir.name, ".fx-build-dir"), "w") as f:
            f.write("out/default")
        os.makedirs(os.path.join(self.dir.name, "out", "default"))
        os.makedirs(os.path.join(self.dir.name, "src", "nested"))
        with open(
            os.path.join(self.dir.name, "out", "default", "tests.json"), "w"
        ) as f:
            json.dump(
                [
                    {
                        "test": {
                            "name": "fuchsia-pkg://fuchsia.com/foo-tests#meta/foo-test-component.cm"
                        }
                    },
                    {"test": {"name": "host_x64/local_script_test"}},
                ],
                f,
            )
        with open(
            os.path.join(self.dir.name, "src", "nested", "BUILD.gn"), "w"
        ) as f:
            f.write(TEST_COMPONENT("foo-test-component"))
            f.write(
                TEST_PACKAGE_WITH_TEST_COMPONENTS(
                    "foo-tests", [":foo-test-component"]
                )
            )
        with open(os.path.join(self.dir.name, "src", "BUILD.gn"), "w") as f:
            f.write(TEST_PACKAGE_WITH_COMPONENT_NAME("tests", "kernel-tests"))
            f.write(
                TEST_PACKAGE_WITH_PACKAGE_NAME(
                    "component-tests", "my-component-tests"
                )
            )
            f.write(TEST_PACKAGE("integration-tests"))
            f.write(HOST_TEST("my-host-test"))

        os.environ["FUCHSIA_DIR"] = str(self.dir.name)
        os.environ["FUCHSIA_TEST_FETCH_REMOTE"] = "FALSE"

    def tearDown(self) -> None:
        self.dir.cleanup()
        return super().tearDown()

    def test_bad_arguments(self) -> None:
        with self.assertRaises(Exception) as ex:
            search_tests.main(["foo", "--threshold", "3"])
        self.assertEqual(
            str(ex.exception), "--threshold must be between 0 and 1"
        )

    def test_without_matches(self) -> None:
        search_tests.main(["afkdjsflkejkgh"])
        self.assertTrue(
            "No matching tests" in self.stdout.getvalue(),
            "Could not find expected string in " + self.stdout.getvalue(),
        )

    def test_component_match(self) -> None:
        search_tests.main(
            ["foo-test-component", "--threshold", "1", "--no-color"]
        )

        self.assertEqual(
            self.stdout.getvalue().strip(),
            """
foo-test-component (100.00% similar)
    Build includes: fuchsia-pkg://fuchsia.com/foo-tests#meta/foo-test-component.cm
""".strip(),
        )

    def test_package_match(self) -> None:
        search_tests.main(["kernel", "--threshold", ".75", "--no-color"])

        self.assertEqual(
            self.stdout.getvalue().strip(),
            """
kernel-tests (90.00% similar)
    fx add-test //src:tests
""".strip(),
        )

    def test_multi_match(self) -> None:
        search_tests.main(
            ["tests", "--threshold", ".2", "--no-color", "--max-results=3"]
        )

        self.assertEqual(
            self.stdout.getvalue().strip(),
            """
foo-test-component (67.41% similar)
    Build includes: fuchsia-pkg://fuchsia.com/foo-tests#meta/foo-test-component.cm
kernel-tests (62.42% similar)
    fx add-test //src:tests
integration-tests (59.22% similar)
    fx add-test //src:integration-tests
(4 more matches not shown)
""".strip(),
        )

    def test_with_without_tests_json_match(self) -> None:
        search_tests.main(
            ["foo-test-component", "--threshold", "1", "--no-color"]
        )
        search_tests.main(
            [
                "foo-test-component",
                "--threshold",
                "1",
                "--no-color",
                "--omit-test-file",
            ]
        )

        self.assertEqual(
            self.stdout.getvalue().strip(),
            """
foo-test-component (100.00% similar)
    Build includes: fuchsia-pkg://fuchsia.com/foo-tests#meta/foo-test-component.cm
No matching tests could be found in your Fuchsia checkout.
""".strip(),
        )

    def test_filtering_host(self) -> None:
        # Search for "my-host-test" with --host. It should find it.
        search_tests.main(
            ["my-host-test", "--host", "--no-color", "--threshold", "1"]
        )
        output = self.stdout.getvalue().strip()
        self.assertTrue("my-host-test" in output)
        self.assertTrue("fx add-host-test" in output)

        # Search for a device test "foo-test-component" with --host and high threshold. It should NOT find it.
        self.stdout.truncate(0)
        self.stdout.seek(0)
        search_tests.main(
            ["foo-test-component", "--host", "--no-color", "--threshold", "1"]
        )
        output = self.stdout.getvalue().strip()
        self.assertTrue("No matching tests" in output)

    def test_filtering_device(self) -> None:
        # Search for "foo-test-component" with --device. It should find it.
        search_tests.main(
            ["foo-test-component", "--device", "--no-color", "--threshold", "1"]
        )
        output = self.stdout.getvalue().strip()
        self.assertTrue("foo-test-component" in output)

        # Search for "my-host-test" with --device. It should NOT find it.
        self.stdout.truncate(0)
        self.stdout.seek(0)
        search_tests.main(
            ["my-host-test", "--device", "--no-color", "--threshold", "1"]
        )
        output = self.stdout.getvalue().strip()
        self.assertTrue("No matching tests" in output)


if __name__ == "__main__":
    unittest.main()
