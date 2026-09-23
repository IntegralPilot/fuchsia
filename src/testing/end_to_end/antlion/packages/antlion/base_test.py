#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import inspect
import re
from typing import Callable

from mobly.base_test import BaseTestClass
from mobly.base_test import Error as MoblyError


class AntlionBaseTest(BaseTestClass):
    # TODO(b/415313773): Remove this once wlanix tests are updated to use mobly's base_test.py
    # instead of AntlionBaseTest class, as the missing functionality is now merged into Mobly.
    def _get_test_methods(
        self, test_names: list[str]
    ) -> list[tuple[str, Callable[[], None]]]:
        """Resolves test method names to bound test methods.

        Args:
            test_names: Test method names.

        Returns:
            List of tuples containing the test method name and the function implementing
            its logic.

        Raises:
            MoblyError: test_names does not match any tests.
        """

        test_table: dict[str, Callable[[], None]] = {
            **self._generated_test_table
        }
        for name, _ in inspect.getmembers(type(self), callable):
            if name.startswith("test_"):
                test_table[name] = getattr(self, name)

        test_methods: list[tuple[str, Callable[[], None]]] = []
        for test_name in test_names:
            if test_name in test_table:
                test_methods.append((test_name, test_table[test_name]))
            else:
                try:
                    pattern = re.compile(test_name)
                except Exception as e:
                    raise MoblyError(
                        f'"{test_name}" is not a valid regular expression'
                    ) from e
                for name in test_table:
                    if pattern.fullmatch(name.strip()):
                        test_methods.append((name, test_table[name]))

        if len(test_methods) == 0:
            all_patterns = '" or "'.join(test_names)
            all_tests = "\n - ".join(test_table.keys())
            raise MoblyError(
                f"{self.TAG} does not declare any tests matching "
                f'"{all_patterns}". Please verify the correctness of '
                f"{self.TAG} test names: \n - {all_tests}"
            )

        return test_methods
