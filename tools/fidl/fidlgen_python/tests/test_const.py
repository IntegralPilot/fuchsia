# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import fidl_test_constants as test_constants


class ConstTestsuite(unittest.TestCase):
    def test_value_of_integer_const(self) -> None:
        self.assertEqual(0b100, test_constants.UINT8)

    def test_value_of_float_const(self) -> None:
        self.assertEqual(3.14159, test_constants.FLOAT32)
        self.assertEqual(3.14159, test_constants.FLOAT64)

    def test_value_of_string_const(self) -> None:
        self.assertEqual("string", test_constants.STRING)

    def test_value_of_bool_const(self) -> None:
        self.assertTrue(test_constants.BOOL)

    def test_value_of_enum_const(self) -> None:
        self.assertEqual(1 | 2, test_constants.EnumType.VALUE)
        self.assertEqual(0b100, test_constants.EnumType.SECOND_VALUE)

        self.assertEqual(0b10101010, test_constants.Enum.E)

    def test_value_from_enum(self) -> None:
        self.assertEqual(test_constants.ENUM_VAL, test_constants.EnumType.VALUE)
        self.assertEqual(
            test_constants.ENUM_PRIMITIVE_VAL, test_constants.EnumType.VALUE
        )

    def test_value_of_bits_const(self) -> None:
        self.assertEqual(1, test_constants.BitsType.VALUE)
        self.assertEqual(0b100, test_constants.BitsType.SECOND_VALUE)
        self.assertEqual(2, test_constants.BitsType.THIRD_VALUE)

        self.assertEqual(0x8, test_constants.Bits.B)

    def test_value_from_bits(self) -> None:
        self.assertEqual(test_constants.BITS_VAL, test_constants.BitsType.VALUE)
        self.assertEqual(
            test_constants.BITS_PRIMITIVE_VAL, test_constants.BitsType.VALUE
        )

    def test_value_of_bits_binary_operation(self) -> None:
        self.assertEqual(0b111, test_constants.OR_RESULT)
        self.assertEqual(
            test_constants.OR_RESULT,
            test_constants.BitsType.VALUE
            | test_constants.BitsType.SECOND_VALUE
            | test_constants.BitsType.THIRD_VALUE,
        )
        self.assertEqual(0b101, test_constants.OR_RESULT_PRIMITIVE_VAL)
        self.assertEqual(
            test_constants.OR_RESULT_PRIMITIVE_VAL,
            test_constants.BitsType.VALUE
            | test_constants.BitsType.SECOND_VALUE,
        )

    def test_struct_defaults_not_supported_at_runtime(self) -> None:
        with self.assertRaises(NotImplementedError) as e:
            test_constants.Struct(_unsupported=None)  # type: ignore[arg-type]
        exception_message = e.exception.args[0]
        self.assertIn("int64_with_default", exception_message)
        self.assertIn("string_with_default", exception_message)
        self.assertIn("bool_with_default", exception_message)
        self.assertIn("enum_with_default", exception_message)
        self.assertIn("bits_with_default", exception_message)

    def test_struct_defaults_not_supported_at_compile_time(self) -> None:
        with self.assertRaises(TypeError) as e:
            test_constants.Struct()  # type: ignore[call-arg]
        exception_message_no_args = e.exception.args[0]
        self.assertIn("unsupported", exception_message_no_args)

        with self.assertRaises(TypeError):
            test_constants.Struct(None)  # type: ignore[arg-type, call-arg]
        exception_message_args = e.exception.args[0]
        self.assertIn("unsupported", exception_message_args)
