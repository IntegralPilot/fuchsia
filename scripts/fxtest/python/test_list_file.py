# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Data model and associated methods for test-list.json files.

test-list.json is a post-processed version of tests.json that is
produced by //tools/test_list_tool. It provides basic execution
instructions and categorization for tests that are part of the
Fuchsia build.

test-list.json is typically stored at the root of the output directory
for the Fuchsia build.
"""

from dataclasses import dataclass
import json
import re
import typing

from dataparse import dataparse
import tests_json_file


@dataparse
@dataclass
class TestListTagKV:
    """Key/Value pair for test tags."""

    key: str
    value: str


@dataparse
@dataclass
class TestListExecutionEntry:
    """The entry describing how to execute a test in test-list.json."""

    # If set, this is the component URL to execute using ffx test.
    component_url: str

    # If set, this is the --realm argument for ffx test.
    realm: str | None = None

    # If set, this is the --max-severity-logs argument for ffx test.
    max_severity_logs: str | None = None

    # If set, this is the --min-severity-logs argument for ffx test.
    min_severity_logs: str | None = None

    # If set, this is test filters to pass to ffx test.
    test_filters: list[str] | None = None

    # If set, the test_filter selecting an empty set of cases to run is
    # treated as success rather than failure.
    no_cases_equals_success: bool = False


@dataparse
@dataclass
class TestListEntry:
    """A single test entry stored in test-list.json."""

    # The name of the test. Must be unique in the file for device tests, may
    # overlap for host.
    name: str

    # A list of tags for the test, stored as KV pairs.
    tags: list[TestListTagKV]

    # Execution details for the test.
    execution: TestListExecutionEntry | None = None

    def is_hermetic(self) -> bool:
        """Determine if a test is hermetic.

        A test is hermetic only if it has a tag "hermetic" set to
        "true". Otherwise we assume the test may be non-hermetic.

        Hermetic tests run in isolation, so knowing that a test is hermetic means
        we can run that test in parallel with other tests safely.

        Returns:
            bool: True if the test is definitely hermetic, False otherwise.
        """
        for tag in self.tags:
            if tag.key == "hermetic" and tag.value == "true":
                return True
        return False


@dataparse
@dataclass
class TestListFile:
    """Top-level data model for test-list.json files."""

    # The list of test list entries stored in the file.
    data: list[TestListEntry]

    @staticmethod
    def entries_from_file(file: str) -> dict[str, TestListEntry]:
        """Parse the file at the given path as a test-list.json file.

        This method converts the flat list in the file into a dictionary
        keyed by test name.

        Args:
            file (os.PathLike): The file path to parse.

        Returns:
            dict[str, TestListEntry]: Map from test name to entry for that test in the file.

        Raises:
            IOError: If the file could not be read.
            JSONDecodeError: If the file is not valid JSON.
            DataParseError: If the data in the file does not match the data model.
        """
        with open(file) as f:
            data = json.load(f)
            parsed: TestListFile = TestListFile.from_dict(data)  # type:ignore
            ret = dict()
            for value in parsed.data:
                ret[value.name] = value
        return ret


_PACKAGE_REGEX = re.compile(r"/([\w\-_]+)#meta")


@dataclass
class Test:
    """Wrapper containing data from both tests.json and test-list.json.

    tests.json contains build-specific information about tests,
    while test-list.json provides the necessary information to
    execute and categorize tests. Since both pieces of information
    are needed to make sense of tests, this dataclass combines both
    pieces for a test.

    Note that info is added lazily because we need to complete a build before
    loading a test-list.json. Regenerating the top-level file can be very
    expensive, so we opt to generate a test-list.json once tests.json and
    selected tests are built only.
    """

    # The test as described by tests.json.
    build: tests_json_file.TestEntry

    # The info for this test loaded from test-list.json
    # This is lazily loaded since we need to build test packages before
    # test-list.json is generated.
    _maybe_info: TestListEntry | None = None

    def __hash__(self) -> int:
        return self.build.test.name.__hash__()

    def __eq__(self, other: object) -> bool:
        if isinstance(other, Test):
            return self.build == other.build
        return False

    def needs_device(self) -> bool:
        """Determine if this test requires a device.

        Device tests require one or more target devices to execute.

        Returns:
            bool: True only if the test requires a device to run.
        """
        return self.is_pure_device_test() or self.is_e2e_test()

    def is_host_test(self) -> bool:
        # These are "pure" host tests, which excludes E2E tests
        # that require a device.
        return self.build.test.path is not None and not self.is_e2e_test()

    def join_info(self, entries: dict[str, TestListEntry]) -> None:
        """Add info from test-list.json to this test.

        This is done is two stages to support lazily creating test-list.json
        following the build.

        Args:
            entries (dict[str, TestListEntry]): Result of parsing test-list.json.
        """
        self._maybe_info = entries[self.name()]

    def name(self) -> str:
        """Return the unique name of this test.

        Returns:
            str: This tests name, which is unique among Tests.
        """
        return self.build.test.name

    @property
    def info(self) -> TestListEntry:
        if self._maybe_info is None:
            raise ValueError(
                f"Test {self.name()} has not been joined with test-list.json"
            )
        return self._maybe_info

    def is_e2e_test(self) -> bool:
        """Determine if this test is an E2E test.

        E2E tests run on the host system (Linux) and also have a device_type
        set in their environments, or explicitly expect SSH.

        Returns:
            bool: True only if the test is an E2E test.
        """
        is_linux = self.build.test.os.lower() == "linux"
        has_device = any(
            env.dimensions.device_type is not None
            for env in self.build.environments or []
        )
        expects_ssh = (
            self.build.expects_ssh is not None and self.build.expects_ssh
        )
        # End-to-end tests are treated as linux host tests, because they are
        # largely self-contained linux executables, hence `is_linux` below.
        # A device is required to run them, so we look for a device type or
        # the expectation that ssh will be used. Vanilla device tests and boot
        # tests, which are not e2e tests, may pass the first two checks, so we
        # explicitly check for the packet URL indicative of a vanilla device test
        # and for qualification as a boot test to rule those out.
        return (
            is_linux
            and (has_device or expects_ssh)
            and self.build.test.package_url is None
            and not self.is_boot_test()
        )

    def is_pure_device_test(self) -> bool:
        """Determine if this test is a pure device test.

        Pure device tests run directly on target devices.

        Returns:
            bool: True only if the test is a pure device test.
        """
        return self.build.test.package_url is not None

    def is_boot_test(self) -> bool:
        """Determine if this test is a boot test.

        Boot tests specify is_boot_test and reboot the device.

        Returns:
            bool: True only if this test is a boot test.
        """
        return self.build.is_boot_test is not None and self.build.is_boot_test

    def package_name(self) -> str | None:
        """Get the package name for this test if applicable.

        If the test in question is a host test, returns None.

        Returns:
            str | None: Package name if this is a device test, None otherwise.
        """
        if self.build.test.package_url is None:
            return None
        m = _PACKAGE_REGEX.findall(self.build.test.package_url)
        return m[0] if m else None

    @classmethod
    def augment_tests_with_info(
        _cls,
        test_entries: list[typing.Self],
        test_list_entries: dict[str, TestListEntry],
    ) -> None:
        """Augment a list of Tests with info fields.

        Args:
            test_entries (list[Test]): A list of Tests to augment.
            test_list_entries (Dict[str, TestListEntry]): The contents parsed from test-list.json

        Raises:
            ValueError: If a test from tests.json does not have a corresponding entry in test-list.json.

        Returns:
            List[Test]: List of joined contents for all tests in tests.json.
        """
        try:
            for entry in test_entries:
                entry.join_info(test_list_entries)
        except KeyError as e:
            raise ValueError(
                f"Test '{e.args[0]}' was found in "
                + "tests.json, but not test-list.json.\nYou may need to run "
                + "`fx build //:test-list` or a full `fx build`."
            )
