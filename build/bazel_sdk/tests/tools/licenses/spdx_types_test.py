# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for spdx_types.py"""

import unittest

from fuchsia.tools.licenses.common_types import LicenseException
from fuchsia.tools.licenses.spdx_types import *


class TestSpdxTypes(unittest.TestCase):
    def test_license_expression_simple(self):
        input = "LicenseRef-Some-Id.1"
        exp = SpdxLicenseExpression.create(input)
        self.assertEqual((input,), exp.license_ids)
        self.assertEqual("{0}", exp.expression_template)
        self.assertEqual(input, exp.serialize())

    def test_license_expression_non_standard(self):
        input = "License-13a9b0a42ef7d1ff"
        exp = SpdxLicenseExpression.create(input)
        self.assertEqual((input,), exp.license_ids)
        self.assertEqual("{0}", exp.expression_template)
        self.assertEqual(input, exp.serialize())

    def test_license_expression_complex(self):
        input = (
            "LicenseRef-X AND (LicenseRef-Y+ OR LicenseRef-Z WITH LicenseRef-X)"
        )
        exp = SpdxLicenseExpression.create(input)
        self.assertEqual(
            ("LicenseRef-X", "LicenseRef-Y", "LicenseRef-Z"), exp.license_ids
        )
        self.assertEqual(
            "{0} AND ({1}+ OR {2} WITH {0})", exp.expression_template
        )
        self.assertEqual(input, exp.serialize())

    def test_license_expression_replace_ids(self):
        input = (
            "LicenseRef-X AND (LicenseRef-Y+ OR LicenseRef-Z WITH LicenseRef-X)"
        )
        exp = SpdxLicenseExpression.create(input)

        id_replacer = SpdxIdReplacer()
        id_replacer.replace_id(old_id="LicenseRef-X", new_id="LicenseRef-0")
        id_replacer.replace_id(old_id="LicenseRef-Y", new_id="LicenseRef-1")
        id_replacer.replace_id(old_id="LicenseRef-Z", new_id="LicenseRef-2")
        exp = exp.replace_license_ids(id_replacer)

        self.assertEqual(
            "LicenseRef-0 AND (LicenseRef-1+ OR LicenseRef-2 WITH LicenseRef-0)",
            exp.serialize(),
        )

    def test_license_id_replacer(self):
        id_replacer = SpdxIdReplacer()
        id_replacer.replace_id(old_id="old", new_id="new")
        id_replacer.replace_id(old_id="foo", new_id="bar")

        with self.assertRaises(LicenseException):
            id_replacer.replace_id(old_id="foo", new_id="baz")

        self.assertEqual(id_replacer.get_replaced_id("old"), "new")
        self.assertEqual(id_replacer.get_replaced_id("foo"), "bar")

        with self.assertRaises(LicenseException):
            id_replacer.get_replaced_id("baz")


if __name__ == "__main__":
    unittest.main()
