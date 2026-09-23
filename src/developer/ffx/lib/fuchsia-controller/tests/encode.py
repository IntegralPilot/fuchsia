# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import fidl_fuchsia_controller_othertest as fc_othertest
import fidl_fuchsia_controller_test as fc_test


class TestEncodeDecode(unittest.TestCase):
    def test_noop_table(self) -> None:
        original = fc_test.NoopTable(
            dub=1.25,
            str_="test_string",
            integer=42,
            union_field=fc_test.NoopUnion(union_int=100),
        )
        payload, handles = original.encode()
        decoded = fc_test.NoopTable.decode(payload, handles)
        self.assertEqual(original, decoded)

    def test_noop_union(self) -> None:
        for variant in [
            fc_test.NoopUnion(union_str="test"),
            fc_test.NoopUnion(union_bool=True),
            fc_test.NoopUnion(union_int=-12345),
        ]:
            payload, handles = variant.encode()
            decoded = fc_test.NoopUnion.decode(payload, handles)
            self.assertEqual(variant, decoded)

    def test_cross_library_struct(self) -> None:
        original = fc_othertest.CrossLibraryStruct(
            value=fc_test.NoopUnion(union_bool=False)
        )
        payload, handles = original.encode()
        decoded = fc_othertest.CrossLibraryStruct.decode(payload, handles)
        self.assertEqual(original, decoded)


if __name__ == "__main__":
    unittest.main()
