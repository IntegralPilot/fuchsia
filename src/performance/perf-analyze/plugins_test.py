# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for the plugins module."""

import unittest

from plugins import (
    PluginArgumentError,
    PluginArgumentParser,
    SectionResult,
)


class PluginsTest(unittest.TestCase):
    """Tests for the plugins module."""

    def test_section_result_to_dict(self) -> None:
        """Tests SectionResult attribute access and to_dict conversion."""
        sr = SectionResult(
            name="Section A",
            results=[{"x": 1}],
            note="Notice",
        )
        self.assertEqual(sr.name, "Section A")
        self.assertEqual(sr.results, [{"x": 1}])
        self.assertEqual(sr.note, "Notice")
        self.assertIsNone(sr.error)
        self.assertEqual(
            sr.to_dict(),
            {
                "name": "Section A",
                "note": "Notice",
                "results": [{"x": 1}],
            },
        )

        sr_err = SectionResult(
            name="Section B",
            error="Something failed",
        )
        self.assertEqual(
            sr_err.to_dict(),
            {
                "name": "Section B",
                "error": "Something failed",
            },
        )

    def test_section_result_from_dict(self) -> None:
        """Tests SectionResult.from_dict construction."""
        data = {
            "name": "Section From Dict",
            "results": [{"a": 1}],
            "note": "A note",
            "error": None,
        }
        sr = SectionResult.from_dict(data)
        self.assertEqual(sr.name, "Section From Dict")
        self.assertEqual(sr.results, [{"a": 1}])
        self.assertEqual(sr.note, "A note")
        self.assertIsNone(sr.error)

        sr_err = SectionResult.from_dict(
            {"name": "Failed Section", "error": "Disk full"}
        )
        self.assertEqual(sr_err.name, "Failed Section")
        self.assertEqual(sr_err.error, "Disk full")
        self.assertIsNone(sr_err.results)

    def test_plugin_argument_parser(self) -> None:
        """Tests PluginArgumentParser raises PluginArgumentError on invalid args."""
        parser = PluginArgumentParser(prog="test_plugin")
        parser.add_argument("--limit", type=int, required=True)

        with self.assertRaises(PluginArgumentError) as ctx:
            parser.parse_args(["--limit", "10", "--unknown"])
        self.assertIn(
            "Error: unrecognized arguments: --unknown", str(ctx.exception)
        )


if __name__ == "__main__":
    unittest.main()
