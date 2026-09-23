# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for exception_utils."""

import unittest

from libs.exception_utils import unroll_and_raise


class UnrollAndRaiseTest(unittest.TestCase):
    def test_standalone(self) -> None:
        with self.assertRaises(ValueError) as cm:
            unroll_and_raise(ValueError("Just one error"))
        self.assertEqual(str(cm.exception), "Just one error")

    def test_implicit_chain(self) -> None:
        try:
            try:
                raise KeyError("Original Test Failure (A)")
            except KeyError:
                raise RuntimeError("Teardown Failure (B)")
        except Exception as e:
            with self.assertRaises(KeyError) as cm:
                unroll_and_raise(e)
            self.assertEqual(str(cm.exception), "'Original Test Failure (A)'")

    def test_explicit_chain(self) -> None:
        try:
            try:
                raise KeyError("Low-level Error (A)")
            except KeyError as e:
                raise RuntimeError("High-level Wrapper (B)") from e
        except Exception as e:
            with self.assertRaises(KeyError) as cm:
                unroll_and_raise(e)
            self.assertEqual(str(cm.exception), "'Low-level Error (A)'")

    def test_both_context_and_cause(self) -> None:
        try:
            try:
                raise KeyError("The Root Cause (A)")
            except KeyError as e_a:
                try:
                    raise IndexError("The Side-Effect (B)")
                except IndexError:
                    raise RuntimeError("The Final Outcome (C)") from e_a
        except Exception as e:
            with self.assertRaises(KeyError) as cm:
                unroll_and_raise(e)
            self.assertEqual(str(cm.exception), "'The Root Cause (A)'")

    def test_suppressed_context(self) -> None:
        try:
            try:
                raise KeyError("Secret Root (A)")
            except KeyError:
                raise RuntimeError("Visible Error (B)") from None
        except Exception as e:
            with self.assertRaises(KeyError) as cm:
                unroll_and_raise(e)
            self.assertEqual(str(cm.exception), "'Secret Root (A)'")


if __name__ == "__main__":
    unittest.main()
