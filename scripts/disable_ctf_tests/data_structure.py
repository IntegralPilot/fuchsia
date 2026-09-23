# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

""" Data structures holding test suites and test cases.

TestCase: A single named test case name with disabled flag and bug annotation.

TestSuite: A single named test suite with a list of `TestCase`s.

Tests: Holds a list of TestSuites. This is the top data structure for the
    program. Can be initialized with a list of TestSuites or a
    `_DictStructuredTests`.

load_and_merge(enumerated_tests: Tests) -> Tests: Starts with a set of tests
    produced by the enumeration mechanism (in command_runner.TestEnumerator).
    Loads the saved test info, and uses that to update/overwrite the enumerated
    list. Returns the combined list. Prints the name of any tests found in the
    saved file but not the enumerated tests - this is possible if a test has
    been dropped from the current test set but is still in the file.
"""

import re
from dataclasses import dataclass
from typing import Any

import util


@dataclass
class TestCase:
    """A named test case, with bug annotation and disabled status."""

    name: str
    disabled: bool = False
    bug: str = ""


@dataclass
class TestSuite:
    """Contains a named test suite and its `TestCase`s.

    Can be initialized from a list of test cases or an internal
    _DictTestSuite."""

    name: str
    cases: list[TestCase]
    compact_name: str = ""

    def __init__(
        self,
        name: str = "",
        cases: list[TestCase] | None = None,
        dict_suite: "_DictTestSuite | None" = None,
    ) -> None:
        assert cases is None or dict_suite is None
        if dict_suite:
            self.name = dict_suite.name
            self.cases = [c for c in dict_suite.cases.values()]
        else:
            self.name = name
            self.cases = cases or []
        self.compact_name = re.sub(
            "^fuchsia-pkg://fuchsia.com/", "fx/", self.name
        )

    def _to_dict_suite(self) -> "_DictTestSuite":
        return _DictTestSuite(
            name=self.name, cases={case.name: case for case in self.cases}
        )


# For merging two sets of tests, dicts are easier than lists.
@dataclass
class _DictTestSuite:
    """TestSuite, but with a dict to find `TestCase`s by their name.

    Used for merging two sets of tests."""

    name: str
    cases: dict[str, TestCase]


@dataclass
class _DictStructuredTests:
    """A dict to find `TestSuite`s by their name.

    Used for merging two sets of tests."""

    suites: dict[str, _DictTestSuite]


class Tests:
    """Public-facing top-level data structure of tests.

    Field `tests` is public, a list of `TestSuite`.

    Method: save() writes to a fixed-location file.
        save() can ignore a parameter for interop with GUI events."""

    def __init__(
        self,
        test_dict: _DictStructuredTests | None = None,
        test_list: list[TestSuite] | None = None,
    ) -> None:
        self.tests: list[TestSuite] = []
        if test_dict:
            self.tests.extend(
                [
                    TestSuite(dict_suite=test)
                    for test in test_dict.suites.values()
                ]
            )
        if test_list:
            self.tests.extend(test_list)

    def save(self, _: Any = None) -> None:
        """Save all test cases that are disabled and/or have bug info."""
        save_list: list[TestSuite] = []
        for suite in self.tests:
            save_suite = TestSuite(suite.name)
            for case in suite.cases:
                if case.bug or case.disabled:
                    save_suite.cases.append(case)
            if save_suite.cases:
                save_list.append(save_suite)
        util.save_program_state(save_list)

    def __str__(self) -> str:
        return f"Tests {self.tests}"

    def _to_dict_tests(self) -> _DictStructuredTests:
        suites = {}
        for t in self.tests:
            cases_dict = {c.name: c for c in t.cases}
            suites[t.name] = _DictTestSuite(name=t.name, cases=cases_dict)
        return _DictStructuredTests(suites)


def _saved_tests() -> "Tests":
    """Loads the program state file and parses the JSON."""

    raw_tests = util.load_program_state()
    suites = []
    for test in raw_tests:
        suite_name = test["name"]
        cases = []
        for case in test["cases"]:
            name: str = case["name"]
            disabled: bool = case["disabled"]
            bug: str = case["bug"]
            cases.append(TestCase(name=name, disabled=disabled, bug=bug))
        suites.append(TestSuite(name=suite_name, cases=cases))
    return Tests(test_list=suites)


def load_and_merge(enumerated_tests: "Tests") -> "Tests":
    """Combines the saved test state with the enumerated tests."""

    test_dict = enumerated_tests._to_dict_tests()
    test_accumulator = test_dict.suites
    tests_from_file = _saved_tests()
    for suite in tests_from_file.tests:
        if suite.name not in test_accumulator:
            print(f"Test {suite.name} not found in enumerated tests.")
            test_accumulator[suite.name] = suite._to_dict_suite()
        else:
            t = test_accumulator[suite.name]
            for case in suite.cases:
                if case.name not in t.cases:
                    print(
                        f"Test case {suite.name} : {case.name} not found in enumerated tests."
                    )
                test_accumulator[suite.name].cases[case.name] = case
    return Tests(test_dict=test_dict)
