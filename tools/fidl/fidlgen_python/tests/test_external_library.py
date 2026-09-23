# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import fidl_test_python_otherstruct as test_python_otherstruct
import fidl_test_python_struct as test_python_struct


class ExternalLibraryTestsuite(unittest.TestCase):
    def test_creation_of_struct_containing_external_struct(self) -> None:
        a = test_python_struct.StructWithExternalStructField(
            value=test_python_otherstruct.EmptyStruct()
        )
        b = test_python_struct.StructWithExternalStructField(
            value=test_python_otherstruct.EmptyStruct()
        )
        self.assertEqual(a, b)

    def test_encode_external_struct_in_struct(self) -> None:
        value = test_python_struct.StructWithExternalStructField(
            value=test_python_otherstruct.EmptyStruct()
        )
        encoded_bytes, hdls = value.encode()
        # fmt: off
        self.assertEqual(encoded_bytes, bytearray([0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00]))
        # fmt: on
        self.assertEqual(hdls, [])

    def test_decode_external_struct_in_struct(self) -> None:
        handles: list[int] = []
        # fmt: off
        encoded_bytes = bytearray([0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00])
        # fmt: on
        value = test_python_struct.StructWithExternalStructField.decode(
            encoded_bytes, handles
        )
        self.assertEqual(
            value,
            test_python_struct.StructWithExternalStructField(
                value=test_python_otherstruct.EmptyStruct()
            ),
        )

    def test_encode_external_non_imported_enum_in_struct(self) -> None:
        value = test_python_struct.StructWithExternalEnumField(value=0)
        encoded_bytes, hdls = value.encode()
        # fmt: off
        self.assertEqual(encoded_bytes, bytearray([0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00]))
        # fmt: on
        self.assertEqual(hdls, [])

    def test_decode_external_non_imported_enum_in_struct(self) -> None:
        handles: list[int] = []
        # fmt: off
        encoded_bytes = bytearray([0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00])
        # fmt: on
        value = test_python_struct.StructWithExternalEnumField.decode(
            encoded_bytes, handles
        )
        self.assertEqual(
            value,
            test_python_struct.StructWithExternalEnumField(value=0),
        )
