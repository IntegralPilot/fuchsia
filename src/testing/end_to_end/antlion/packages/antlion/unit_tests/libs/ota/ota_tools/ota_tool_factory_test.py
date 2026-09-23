#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

from antlion.libs.ota.ota_tools import ota_tool_factory


class MockOtaTool(object):
    def __init__(self, command):
        self.command = command


class OtaToolFactoryTests(unittest.TestCase):
    def setUp(self):
        ota_tool_factory._constructed_tools = {}

    def test_create_constructor_exists(self):
        ota_tool_factory._CONSTRUCTORS = {
            MockOtaTool.__name__: lambda command: MockOtaTool(command),
        }
        ret = ota_tool_factory.create(MockOtaTool.__name__, "command")
        self.assertEqual(type(ret), MockOtaTool)
        self.assertTrue(ret in ota_tool_factory._constructed_tools.values())

    def test_create_not_in_constructors(self):
        ota_tool_factory._CONSTRUCTORS = {}
        with self.assertRaises(KeyError):
            ota_tool_factory.create(MockOtaTool.__name__, "command")

    def test_create_returns_cached_tool(self):
        ota_tool_factory._CONSTRUCTORS = {
            MockOtaTool.__name__: lambda command: MockOtaTool(command),
        }
        ret_a = ota_tool_factory.create(MockOtaTool.__name__, "command")
        ret_b = ota_tool_factory.create(MockOtaTool.__name__, "command")
        self.assertEqual(ret_a, ret_b)


if __name__ == "__main__":
    unittest.main()
