# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

from fidl._fidl_common import FidlMeta, camel_case_to_snake_case


class NoAdditionalRequired(metaclass=FidlMeta):
    ...


class OneAdditionalRequired(
    metaclass=FidlMeta, required_class_variables=[("foo", int)]
):
    ...


class FidlCommon(unittest.TestCase):
    """Tests for FIDL common utils"""

    def test_camel_case_to_snake_case_standard(self) -> None:
        expected = "test_string"
        got = camel_case_to_snake_case("TestString")
        self.assertEqual(expected, got)

    def test_camel_case_to_snake_case_empty_string(self) -> None:
        expected = ""
        got = camel_case_to_snake_case("")
        self.assertEqual(expected, got)

    def test_camel_case_to_snake_case_all_caps(self) -> None:
        expected = "f_f_f_f"
        got = camel_case_to_snake_case("FFFF")
        self.assertEqual(expected, got)

    def test_fidl_meta_implicit_required_fields_missing(self) -> None:
        with self.assertRaises(NotImplementedError):

            class Test(NoAdditionalRequired):
                ...

    def test_fidl_meta_implicit_required_fields_present(self) -> None:
        class Test(NoAdditionalRequired):
            __fidl_kind__ = "foo"

    def test_fidl_meta_explicit_required_fields_missing(self) -> None:
        with self.assertRaises(NotImplementedError):

            class Test(OneAdditionalRequired):
                __fidl_kind__ = "foo"

    def test_fidl_meta_explicit_required_fields_present(self) -> None:
        class Test(OneAdditionalRequired):
            __fidl_kind__ = "foo"
            foo: int = 1

    def test_fidl_meta_explicit_required_fields_present_but_wrong_type(
        self,
    ) -> None:
        with self.assertRaises(NotImplementedError):

            class Test(OneAdditionalRequired):
                __fidl_kind__ = "foo"
                foo: str = "foo"
