# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import unittest

import fuchsia_inspect

EXAMPLE_DATA = """
[
  {
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo",
        "timestamp": 181016000000000,
        "file_name": "foo.txt"
    },
    "moniker": "core/example",
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 1
  },
  {
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo2",
        "timestamp": 181016000000000
    },
    "moniker": "core/example",
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 1
  },
  {
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo2",
        "timestamp": 181016000000000,
        "errors": [
            "Unknown failure"
        ]
    },
    "moniker": "core/example",
    "payload": null,
    "version": 1
  }
]
"""

BAD_VERSION = """
{
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo",
        "timestamp": 181016000000000,
        "file_name": "foo.txt"
    },
    "moniker": "core/example",
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 2
  }
"""

BAD_DATA_SOURCE = """
{
    "data_source": "Logs",
    "metadata": {
        "component_url": "foo",
        "timestamp": 181016000000000,
        "file_name": "foo.txt"
    },
    "moniker": "core/example",
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 1
  }
"""

BAD_TYPE = """
{
    "data_source": "Inspect",
    "metadata": 10,
    "moniker": "core/example",
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 1
  }
"""

MISSING_MONIKER = """
{
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo",
        "timestamp": 181016000000000,
        "file_name": "foo.txt"
    },
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 1
  }
"""


class TestInspect(unittest.TestCase):
    def test_load_example(self) -> None:
        vals = json.loads(EXAMPLE_DATA)
        assert isinstance(vals, list)
        processed = list(map(fuchsia_inspect.InspectData.from_dict, vals))
        self.assertEqual(len(processed), 3)
        self.assertEqual(processed[0].moniker, "core/example")
        self.assertEqual(processed[0].metadata.timestamp.seconds(), 181016.0)
        self.assertEqual(
            processed[0].metadata.timestamp.nanoseconds(), 181016000000000
        )
        self.assertEqual(processed[0].metadata.file_name, "foo.txt")
        self.assertEqual(processed[0].metadata.component_url, "foo")
        self.assertIsNone(processed[1].metadata.file_name)
        assert processed[0].payload is not None
        self.assertEqual(processed[0].payload["root"]["value"], 100)
        assert processed[2].metadata.errors is not None
        self.assertEqual(processed[2].metadata.errors[0], "Unknown failure")

    def test_failures(self) -> None:
        self.assertRaises(
            fuchsia_inspect.VersionMismatchError,
            lambda: fuchsia_inspect.InspectData.from_dict(
                json.loads(BAD_VERSION)
            ),
        )
        self.assertRaises(
            fuchsia_inspect.InvalidDataTypeError,
            lambda: fuchsia_inspect.InspectData.from_dict(
                json.loads(BAD_DATA_SOURCE)
            ),
        )
        self.assertRaises(
            fuchsia_inspect.InvalidFieldError,
            lambda: fuchsia_inspect.InspectData.from_dict(json.loads(BAD_TYPE)),
        )
        self.assertRaises(
            fuchsia_inspect.MissingFieldError,
            lambda: fuchsia_inspect.InspectData.from_dict(
                json.loads(MISSING_MONIKER)
            ),
        )
