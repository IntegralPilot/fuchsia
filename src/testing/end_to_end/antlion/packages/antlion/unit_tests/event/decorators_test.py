#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import unittest
from unittest import TestCase

from antlion.event.decorators import subscribe_static
from antlion.event.subscription_handle import SubscriptionHandle
from mock import Mock


class DecoratorsTest(TestCase):
    """Tests the decorators found in antlion.event.decorators."""

    def test_subscribe_static_return_type(self):
        """Tests that the subscribe_static is the correct type."""
        mock = Mock()

        @subscribe_static(type)
        def test(_):
            return mock

        self.assertTrue(isinstance(test, SubscriptionHandle))

    def test_subscribe_static_calling_the_function_returns_normally(self):
        """Tests that functions decorated by subscribe_static can be called."""
        static_mock = Mock()

        @subscribe_static(type)
        def test(_):
            return static_mock

        self.assertEqual(test(Mock()), static_mock)


if __name__ == "__main__":
    unittest.main()
