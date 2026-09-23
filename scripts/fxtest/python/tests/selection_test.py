# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import unittest
from unittest import mock

import args
import environment
import event
import selection
import selection_types
import test_list_file
import tests_json_file


class MatchGroupTest(unittest.TestCase):
    """Test creating and printing MatchGroups."""

    def test_parse_empty(self) -> None:
        """Ensure that an empty selection produces no groups."""
        groups = selection._parse_selection_command_line([])
        self.assertEqual(len(groups), 0)

    def assertMatchContents(
        self,
        group: selection_types.MatchGroup,
        names: set[str],
        packages: set[str],
        components: set[str],
    ) -> None:
        """Helper to assert string sets against MatchGroup fields.

        Args:
            group (MatchGroup): Group to check.
            names (set[str]): Expected names.
            packages (set[str]): Expected packages.
            components (set[str]): Expected components.
        """
        self.assertSetEqual(group.names, names)
        self.assertSetEqual(group.packages, packages)
        self.assertSetEqual(group.components, components)

    def test_parse_single(self) -> None:
        """Test parsing and formatting single arguments."""
        name_group = selection._parse_selection_command_line(["name"])
        package_group = selection._parse_selection_command_line(
            ["--package", "name"]
        )
        component_group = selection._parse_selection_command_line(
            ["--component", "name"]
        )

        self.assertMatchContents(name_group[0], {"name"}, set(), set())
        self.assertMatchContents(package_group[0], set(), {"name"}, set())
        self.assertMatchContents(component_group[0], set(), set(), {"name"})

        self.assertEqual(str(name_group[0]), "name")
        self.assertEqual(str(package_group[0]), "--package name")
        self.assertEqual(str(component_group[0]), "--component name")

    def test_parse_and(self) -> None:
        """Test logical AND parsing and formatting."""
        groups = selection._parse_selection_command_line(
            [
                "name",
                "--and",
                "--component",
                "component",
                "--and",
                "--package",
                "package",
            ]
        )
        self.assertEqual(len(groups), 1)

        self.assertMatchContents(
            groups[0], {"name"}, {"package"}, {"component"}
        )
        self.assertEqual(
            str(groups[0]),
            "name --and --package package --and --component component",
        )

    def test_parse_or(self) -> None:
        """Test spreading arguments across multiple MatchGroups."""
        groups = selection._parse_selection_command_line(
            ["name", "--component", "component", "--package", "package"]
        )
        self.assertEqual(len(groups), 3)

        self.assertMatchContents(groups[0], {"name"}, set(), set())
        self.assertMatchContents(groups[1], set(), set(), {"component"})
        self.assertMatchContents(groups[2], set(), {"package"}, set())

    def test_all_together(self) -> None:
        """Test combination of values, ANDs, and ORs."""
        groups = selection._parse_selection_command_line(
            [
                "name",
                "--and",
                "name2",
                "--component",
                "component",
                "--and",
                "name3",
                "--package",
                "package",
                "--and",
                "name4",
            ]
        )
        self.assertEqual(len(groups), 3)

        self.assertMatchContents(groups[0], {"name", "name2"}, set(), set())
        self.assertMatchContents(groups[1], {"name3"}, set(), {"component"})
        self.assertMatchContents(groups[2], {"name4"}, {"package"}, set())

    def test_parse_errors(self) -> None:
        """Test various invalid scenarios."""

        # Invalid to start an expression with AND
        self.assertRaises(
            selection.SelectionError,
            lambda: selection._parse_selection_command_line(["--and", "value"]),
        )

        # Invalid to use --and after --and
        self.assertRaises(
            selection.SelectionError,
            lambda: selection._parse_selection_command_line(
                ["value", "--and", "--and"]
            ),
        )

        # Invalid to use --and at the end of a selection
        self.assertRaises(
            selection.SelectionError,
            lambda: selection._parse_selection_command_line(["value", "--and"]),
        )

        # --package and --component require an argument
        self.assertRaises(
            selection.SelectionError,
            lambda: selection._parse_selection_command_line(
                ["value", "--component"]
            ),
        )
        self.assertRaises(
            selection.SelectionError,
            lambda: selection._parse_selection_command_line(
                ["value", "--package"]
            ),
        )


class SelectTestsTest(unittest.IsolatedAsyncioTestCase):
    """Tests related to the selection of test entries to execute."""

    @staticmethod
    def _make_package_test(
        prefix: str, package: str, component: str
    ) -> test_list_file.Test:
        """Utility method to simulate a device Test

        Args:
            prefix (str): Prefix path for the label.
            package (str): Package name.
            component (str): Component name (without .cm)

        Returns:
            Test: Fake Test entry.
        """
        return test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name=f"fuchsia-pkg://fuchsia.com/{package}#meta/{component}.cm",
                    label=f"//{prefix}:{package}($toolchain)",
                    os="fuchsia",
                    package_url=f"fuchsia-pkg://fuchsia.com/{package}#meta/{component}.cm",
                )
            ),
        )

    @staticmethod
    def _make_host_test(prefix: str, name: str) -> test_list_file.Test:
        """Utility method to simulate a host Test

        Args:
            prefix (str): Prefix path for the label.
            name (str): Binary name, without host_x64.

        Returns:
            Test: Fake Test entry.
        """
        return test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name=f"host_x64/{name}",
                    label=f"//{prefix}:{name}($toolchain)",
                    os="linux",
                    path=f"host_x64/{name}",
                )
            ),
        )

    def _make_exec_env(self) -> environment.ExecutionEnvironment:
        """Create an execution environment for test."""

        exec_env = environment.ExecutionEnvironment(
            fuchsia_dir="",
            out_dir="",
            test_json_file="",
            disabled_ctf_tests_file="",
            log_file=None,
            test_list_file="",
        )

        # Override the return value when retrieving the command line
        # for the matching script.  This will be relative to the test
        # directory which includes dldist as test data under "bin".
        #
        # Note we need to go up two levels because the parent of __file__
        # is the .pyz file path itself.

        cur_path = os.path.dirname(os.path.dirname(__file__))
        dldist_path = os.path.join(cur_path, "bin", "dldist")

        env_patch = mock.patch.object(
            exec_env,
            "fx_cmd_line",
            mock.Mock(return_value=[dldist_path]),
        )

        env_patch.start()
        self.addCleanup(env_patch.stop)

        return exec_env

    async def test_select_all(self) -> None:
        """Test that empty selection selects all mode-matching tests with perfect scores"""
        tests = [
            self._make_package_test("src/tests", "foo", "bar"),
            self._make_host_test("src/tests2", "baz"),
        ]

        exec_env = self._make_exec_env()

        selected = await selection.select_tests(tests, [], exec_env)

        self.assertEqual(len(selected.selected), 2)
        for score in selected.best_score.values():
            self.assertEqual(score, selection.PERFECT_MATCH_DISTANCE)
        self.assertTrue(selected.has_device_test())

        host_selected = await selection.select_tests(
            tests, [], exec_env, selection.SelectionMode.HOST
        )
        self.assertEqual(len(host_selected.selected), 1)
        self.assertEqual(
            host_selected.best_score["host_x64/baz"],
            selection.PERFECT_MATCH_DISTANCE,
        )
        self.assertEqual(
            host_selected.best_score[
                "fuchsia-pkg://fuchsia.com/foo#meta/bar.cm"
            ],
            selection.NO_MATCH_DISTANCE,
        )
        self.assertFalse(host_selected.has_device_test())

        device_selected = await selection.select_tests(
            tests, [], exec_env, selection.SelectionMode.DEVICE
        )
        self.assertEqual(len(device_selected.selected), 1)
        self.assertEqual(
            device_selected.best_score[
                "fuchsia-pkg://fuchsia.com/foo#meta/bar.cm"
            ],
            selection.PERFECT_MATCH_DISTANCE,
        )
        self.assertAlmostEqual(
            device_selected.best_score["host_x64/baz"],
            selection.NO_MATCH_DISTANCE,
        )
        self.assertTrue(device_selected.has_device_test())

    async def test_select_all_fails_on_exact(self) -> None:
        """Test that empty selection fails if exact_match is used"""
        tests = [
            self._make_package_test("src/tests", "foo", "bar"),
            self._make_host_test("src/tests2", "baz"),
        ]

        exec_env = self._make_exec_env()

        try:
            selections = await selection.select_tests(
                tests,
                [],
                exec_env,
                exact_match=True,
            )
            self.assertTrue(False, f"Expected a SelectionError: {selections}")
        except selection.SelectionError:
            pass

    async def test_prefix_matches(self) -> None:
        """Test that selecting prefixes of tests results in a perfect match."""

        tests = [
            self._make_package_test("src/tests", "foo-pkg", "bar-test"),
            self._make_host_test("src/other-tests", "binary_test"),
        ]

        exec_env = self._make_exec_env()

        select_path = await selection.select_tests(
            tests, ["//src/tests"], exec_env
        )
        select_name1 = await selection.select_tests(
            tests, ["foo-pkg"], exec_env
        )
        select_name2 = await selection.select_tests(
            tests, ["bar-test"], exec_env
        )
        select_pkg = await selection.select_tests(
            tests, ["--package", "foo-pkg"], exec_env
        )
        select_cm = await selection.select_tests(
            tests, ["--component", "bar-test"], exec_env
        )
        url_prefix = await selection.select_tests(
            tests, ["fuchsia-pkg://fuchsia.com/foo-pkg"], exec_env
        )

        self.assertEqual(select_path.selected, select_name1.selected)
        self.assertEqual(select_path.selected, select_name2.selected)
        self.assertEqual(select_path.selected, select_pkg.selected)
        self.assertEqual(select_path.selected, select_cm.selected)
        self.assertEqual(select_path.selected, url_prefix.selected)

        for g, matches in select_path.group_matches:
            self.assertEqual(len(matches), 1, f"Failed in {g}")

        host_path = await selection.select_tests(
            tests, ["binary_test"], exec_env
        )
        self.assertEqual(
            [s.name() for s in host_path.selected], ["host_x64/binary_test"]
        )

        full_path = await selection.select_tests(tests, ["//src"], exec_env)
        self.assertEqual(
            [s.name() for s in full_path.selected], [t.name() for t in tests]
        )

    async def test_contains_matches(self) -> None:
        """Test that selecting substrings of tests results in a perfect match."""

        tests = [
            self._make_package_test("src/tests", "foo-pkg", "bar-test"),
            self._make_host_test("src/other-tests", "binary_test"),
        ]

        exec_env = self._make_exec_env()

        select_path = await selection.select_tests(tests, ["rc/te"], exec_env)
        select_name1 = await selection.select_tests(tests, ["o-pk"], exec_env)
        select_name2 = await selection.select_tests(
            tests, ["ar-test"], exec_env
        )
        select_pkg = await selection.select_tests(
            tests, ["--package", "o-pk"], exec_env
        )
        select_cm = await selection.select_tests(
            tests, ["--component", "ar-test"], exec_env
        )
        url_contains = await selection.select_tests(
            tests, ["fuchsia.com/foo"], exec_env
        )

        self.assertEqual(select_path.selected, select_name1.selected)
        self.assertEqual(select_path.selected, select_name2.selected)
        self.assertEqual(select_path.selected, select_pkg.selected)
        self.assertEqual(select_path.selected, select_cm.selected)
        self.assertEqual(select_path.selected, url_contains.selected)

        for _, matches in select_path.group_matches:
            self.assertEqual(len(matches), 1)

        host_path = await selection.select_tests(tests, ["ary_test"], exec_env)
        self.assertEqual(
            [s.name() for s in host_path.selected], ["host_x64/binary_test"]
        )

        full_path = await selection.select_tests(tests, ["src"], exec_env)
        self.assertEqual(
            [s.name() for s in full_path.selected], [t.name() for t in tests]
        )

    async def test_approximate_matches(self) -> None:
        """Test that fuzzy matching catches common issues."""

        tests = [
            self._make_package_test("src/tests", "foo-pkg", "bar-test"),
            self._make_host_test("src/other-tests", "binary_test"),
        ]

        exec_env = self._make_exec_env()

        host_fuzzy = await selection.select_tests(
            tests, ["binaryytest"], exec_env
        )
        self.assertEqual(
            [s.name() for s in host_fuzzy.selected], ["host_x64/binary_test"]
        )
        self.assertEqual(host_fuzzy.best_score["host_x64/binary_test"], 1)
        self.assertEqual(host_fuzzy.fuzzy_distance_threshold, 3)

    async def test_exact_does_not_match(self) -> None:
        """Test that fuzzy matching catches common issues."""

        tests = [
            self._make_package_test("src/tests", "foo-pkg", "bar-test"),
            self._make_host_test("src/other-tests", "binary_test"),
        ]

        exec_env = self._make_exec_env()

        host_exact = await selection.select_tests(
            tests,
            ["binaryytest"],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(0, len(host_exact.selected))
        self.assertEqual(
            host_exact.best_score["host_x64/binary_test"],
            selection.NO_MATCH_DISTANCE,
        )

    async def test_group_matches_isolation(self) -> None:
        """Test that multiple match groups have isolated match lists."""
        tests = [
            self._make_package_test("src/tests", "foo-pkg", "bar-test"),
            self._make_host_test("src/other-tests", "binary_test"),
        ]
        exec_env = self._make_exec_env()
        result = await selection.select_tests(
            tests,
            ["bar-test", "non_existent_test"],
            exec_env,
        )
        self.assertEqual(len(result.group_matches), 2)
        _, matches1 = result.group_matches[0]
        _, matches2 = result.group_matches[1]
        self.assertEqual(len(matches1), 1)
        self.assertEqual(len(matches2), 0)

    async def test_package_and_component(self) -> None:
        """Test that --package and/or --component works"""
        tests = [
            self._make_package_test("src/tests", "foo-pkg", "bar-test"),
            self._make_package_test("src/tests", "foo-pkg", "other-baz-test"),
            self._make_package_test(
                "src/tests", "other-bar-pkg", "other-baz-test"
            ),
            self._make_host_test("src/other-tests", "binary_test"),
        ]

        exec_env = self._make_exec_env()

        package_selection = await selection.select_tests(
            tests,
            ["--package", "foo-pkg"],
            exec_env,
        )
        self.assertEqual(
            list(map(lambda x: x.name(), package_selection.selected)),
            [
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm",
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/other-baz-test.cm",
            ],
        )

        component_selection = await selection.select_tests(
            tests,
            ["--component", "other-baz-test"],
            exec_env,
        )
        self.assertEqual(
            list(map(lambda x: x.name(), component_selection.selected)),
            [
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/other-baz-test.cm",
                "fuchsia-pkg://fuchsia.com/other-bar-pkg#meta/other-baz-test.cm",
            ],
        )

        # Try prefix of package name.
        both_selection = await selection.select_tests(
            tests,
            ["--package", "foo", "--and", "--component", "other-baz-test"],
            exec_env,
        )
        self.assertEqual(
            list(map(lambda x: x.name(), both_selection.selected)),
            [
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/other-baz-test.cm",
            ],
        )

        # Prefix should not match in exact mode
        exact_selection = await selection.select_tests(
            tests,
            [
                "--package",
                "foo",
                "--and",
                "--component",
                "other-baz-test",
            ],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(
            list(map(lambda x: x.name(), exact_selection.selected)),
            [],
        )

    async def test_exact_matches(self) -> None:
        """Test that exact_mode matching mode works correctly"""

        tests = [
            self._make_package_test("src/tests", "foo-pkg", "bar-test"),
            self._make_host_test("src/other-tests", "binary_test"),
        ]

        exec_env = self._make_exec_env()

        # Don't match package or component in exact mode
        device_exact = await selection.select_tests(
            tests,
            ["foo-pkg"],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(0, len(device_exact.selected))
        self.assertEqual(
            device_exact.best_score[
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm"
            ],
            selection.NO_MATCH_DISTANCE,
        )
        device_exact = await selection.select_tests(
            tests,
            ["bar-test"],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(0, len(device_exact.selected))
        self.assertEqual(
            device_exact.best_score[
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm"
            ],
            selection.NO_MATCH_DISTANCE,
        )

        # Package matches in fuzzy mode
        device_fuzzy = await selection.select_tests(
            tests, ["foo-pkg"], exec_env, mode=selection.SelectionMode.ANY
        )
        self.assertEqual(1, len(device_fuzzy.selected))
        self.assertEqual(
            device_fuzzy.best_score[
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm"
            ],
            selection.PERFECT_MATCH_DISTANCE,
        )
        device_fuzzy = await selection.select_tests(
            tests, ["bar-test"], exec_env, mode=selection.SelectionMode.ANY
        )
        self.assertEqual(1, len(device_fuzzy.selected))
        self.assertEqual(
            device_fuzzy.best_score[
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm"
            ],
            selection.PERFECT_MATCH_DISTANCE,
        )

        # Only match full name in exact mode
        device_exact = await selection.select_tests(
            tests,
            ["fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm"],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(1, len(device_exact.selected))
        self.assertEqual(
            device_exact.best_score[
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm"
            ],
            selection.PERFECT_MATCH_DISTANCE,
        )

        # Support exact package and component matching
        package_exact = await selection.select_tests(
            tests,
            ["--package", "foo-pkg"],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(1, len(package_exact.selected))
        self.assertEqual(
            package_exact.best_score[
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm"
            ],
            selection.PERFECT_MATCH_DISTANCE,
        )

        component_exact = await selection.select_tests(
            tests, ["--component", "bar-test"], exec_env, exact_match=True
        )
        self.assertEqual(1, len(component_exact.selected))
        self.assertEqual(
            component_exact.best_score[
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm"
            ],
            selection.PERFECT_MATCH_DISTANCE,
        )

        # Test that selecting for the full path of a host test matches exactly.
        host_exact = await selection.select_tests(
            tests,
            ["host_x64/binary_test"],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(1, len(host_exact.selected))
        self.assertEqual(
            host_exact.best_score["host_x64/binary_test"],
            selection.PERFECT_MATCH_DISTANCE,
        )

        # Test that selecting for the final path segment of a host
        # test matches exactly.
        host_exact = await selection.select_tests(
            tests,
            ["binary_test"],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(1, len(host_exact.selected))
        self.assertEqual(
            host_exact.best_score["host_x64/binary_test"],
            selection.PERFECT_MATCH_DISTANCE,
        )

        # Test that selecting by label works in exact mode.
        label_exact = await selection.select_tests(
            tests,
            ["//src/other-tests:binary_test"],
            exec_env,
            exact_match=True,
        )
        self.assertEqual(1, len(label_exact.selected))
        self.assertEqual(
            label_exact.best_score["host_x64/binary_test"],
            selection.PERFECT_MATCH_DISTANCE,
        )

    async def test_perfect_match_omits_approximate_match(self) -> None:
        """Test that fuzzy matching is not used if there is a perfect match."""

        tests = [
            self._make_host_test("src/other-tests", "binary_test"),
            self._make_host_test("src/more-tests", "binaryytest"),
        ]

        exec_env = self._make_exec_env()

        host_fuzzy = await selection.select_tests(
            tests, ["binaryytest"], exec_env
        )
        self.assertEqual(
            [s.name() for s in host_fuzzy.selected], ["host_x64/binaryytest"]
        )
        self.assertEqual(host_fuzzy.best_score["host_x64/binaryytest"], 0)
        self.assertEqual(host_fuzzy.best_score["host_x64/binary_test"], 1)
        self.assertEqual(
            [s.name() for s in host_fuzzy.selected_but_not_run],
            ["host_x64/binary_test"],
        )
        self.assertEqual(host_fuzzy.fuzzy_distance_threshold, 3)

    async def test_approximate_match_multiple(self) -> None:
        """Test that fuzzy matching matches multiple imperfect
        matches if there is no perfect match."""

        tests = [
            self._make_host_test("src/other-tests", "binary_test"),
            self._make_host_test("src/more-tests", "binaryytest"),
        ]

        exec_env = self._make_exec_env()

        host_fuzzy = await selection.select_tests(
            tests, ["binary0test"], exec_env
        )
        self.assertEqual(
            {s.name() for s in host_fuzzy.selected},
            {"host_x64/binary_test", "host_x64/binaryytest"},
        )
        self.assertEqual(host_fuzzy.best_score["host_x64/binaryytest"], 1)
        self.assertEqual(host_fuzzy.best_score["host_x64/binary_test"], 1)

    async def test_flag_mutation(self) -> None:
        """Test that we can apply command line flag behavior to selections"""

        tests = [
            self._make_package_test("src/tests", "foo-pkg", "bar-test"),
            self._make_package_test("src/tests", "bar-pkg", "baz-test"),
            self._make_host_test("src/other-tests", "binary_test"),
            self._make_host_test("src/other-tests", "script_test"),
        ]

        exec_env = self._make_exec_env()

        # Limit only
        select_all = await selection.select_tests(tests, [], exec_env)
        select_all.apply_flags(args.parse_args(["--limit=3"]))
        self.assertEqual(len(select_all.selected), 3)
        self.assertEqual(len(select_all.selected_but_not_run), 1)
        self.assertEqual(
            [x.name() for x in select_all.selected],
            [
                "fuchsia-pkg://fuchsia.com/foo-pkg#meta/bar-test.cm",
                "fuchsia-pkg://fuchsia.com/bar-pkg#meta/baz-test.cm",
                "host_x64/binary_test",
            ],
        )

        # Offset only
        select_all = await selection.select_tests(tests, [], exec_env)
        select_all.apply_flags(args.parse_args(["--offset=3"]))
        self.assertEqual(len(select_all.selected), 1)
        self.assertEqual(len(select_all.selected_but_not_run), 3)
        self.assertEqual(
            [x.name() for x in select_all.selected],
            [
                "host_x64/script_test",
            ],
        )

        # Both
        select_all = await selection.select_tests(tests, [], exec_env)
        select_all.apply_flags(args.parse_args(["--offset=1", "--limit=2"]))
        self.assertEqual(len(select_all.selected), 2)
        self.assertEqual(len(select_all.selected_but_not_run), 2)
        self.assertEqual(
            [x.name() for x in select_all.selected],
            [
                "fuchsia-pkg://fuchsia.com/bar-pkg#meta/baz-test.cm",
                "host_x64/binary_test",
            ],
        )

    async def test_logs_will_not_contain_dldist(self) -> None:
        """Test that we omit process output from dldist in the logs"""

        tests = [
            self._make_host_test("src/other-tests", "binary_test"),
        ]

        exec_env = self._make_exec_env()

        recorder = event.EventRecorder()

        recorder.emit_init()

        host_fuzzy = await selection.select_tests(
            tests, ["binary"], exec_env, recorder=recorder
        )

        recorder.emit_end()

        output_events = [
            e
            async for e in recorder.iter()
            if e.payload is not None and e.payload.program_output is not None
        ]
        self.assertEqual(len(output_events), 0)
