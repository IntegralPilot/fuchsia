# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Types needed for selecting tests.

This functionality is refactored to a separate module to avoid
circular dependencies between events and selection logic.
"""

from dataclasses import dataclass
from dataclasses import field
import random
import typing

import args
from test_list_file import Test


@dataclass
class MatchGroup:
    """Description of one set of properties that must match a test for selection.

    All properties are logically ANDed together. Logical OR is represented by
    multiple MatchGroups applied in sequence.

    Examples:
      `--package foo --component bar`
        Matches tests with package foo OR component bar
      `--package foo --and --component bar`
        Matches only tests with package foo and component bar.
      `my-test --package foo --and unittest`
        Matches tests named "my-test" OR tests named "unittest" in package foo.
    """

    # Set of names to match. "Name" matches are the most permissive.
    names: typing.Set[str] = field(default_factory=set)

    # Set of package names to match. The package field must be a match.
    packages: typing.Set[str] = field(default_factory=set)

    # Set of component names to match. The component field must be a match.
    components: typing.Set[str] = field(default_factory=set)

    def __str__(self) -> str:
        """Create a human-readable representation of this match group.

        This aligns with the input flags to `fx test` such that a
        user can copy and paste the output of this function and
        produce an equivalent MatchGroup.

        Returns:
            str: The copy/pasteable command line representing this group.
        """
        elements = (
            list(self.names)
            + [f"--package {p}" for p in self.packages]
            + [f"--component {c}" for c in self.components]
        )
        return " --and ".join(elements)


@dataclass
class TestSelections:
    """Return value for test selection.

    This class contains the list of selected tests as well as
    information related to the selection process for debugging.
    """

    # The list of tests selected, ordered by presence in tests.json.
    selected: list[Test]

    # Tests that were selected but will not be run due to flags.
    # (e.g. --count)
    selected_but_not_run: list[Test]

    # The best score calculated for each test in tests.json, including non-selected tests.
    best_score: dict[str, int]

    # List of match groups with the set of tests selected by that match group.
    group_matches: list[tuple[MatchGroup, list[str]]]

    # The threshold used to match these tests.
    fuzzy_distance_threshold: int

    def has_device_test(self) -> bool:
        """Determine if this set of test selections has any tests
        that depend on a running Fuchsia device.

        Returns:
            bool: True if a test that requires a device is selected, False otherwise.
        """
        return any([entry.needs_device() for entry in self.selected])

    def has_e2e_test(self) -> bool:
        """Determine if this set of test selections has any host-driven device tests.

        Returns:
            bool: True if a host-driven device test is selected, False otherwise.
        """
        return any([entry.is_e2e_test() for entry in self.selected])

    def apply_flags(self, flags: args.Flags) -> None:
        """Mutate the set of selected tests based on flags.

        Args:
            flags (args.Flags): The flags to apply to these selections.
        """
        if flags.random:
            random.shuffle(self.selected)
        self.selected_but_not_run.extend(self.selected[: flags.offset])
        self.selected = self.selected[flags.offset :]
        if flags.limit is not None:
            self.selected_but_not_run.extend(self.selected[flags.limit :])
            self.selected = self.selected[: flags.limit]
