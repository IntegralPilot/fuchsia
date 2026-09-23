#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for ninjatrace2json.py."""

import json
import shutil
import tempfile
import unittest
from pathlib import Path
from typing import Any

import ninjatrace2json


class ExtractTraceEventsTest(unittest.TestCase):
    def test_extract_from_list(self) -> None:
        raw_list = [
            {"name": "event1", "ph": "C"},
            {"name": "event2", "ph": "C"},
        ]
        events = ninjatrace2json.extract_trace_events(raw_list)
        self.assertEqual(events, raw_list)

    def test_extract_from_dict(self) -> None:
        raw_dict = {
            "traceEvents": [
                {"name": "event1", "ph": "C"},
                {"name": "event2", "ph": "C"},
            ],
            "other_field": "val",
        }
        events = ninjatrace2json.extract_trace_events(raw_dict)
        self.assertEqual(
            events,
            [
                {"name": "event1", "ph": "C"},
                {"name": "event2", "ph": "C"},
            ],
        )

    def test_extract_from_invalid_type(self) -> None:
        events = ninjatrace2json.extract_trace_events("invalid string")
        self.assertEqual(events, [])


class MergeProfileTest(unittest.TestCase):
    def setUp(self) -> None:
        self.test_dir = Path(tempfile.mkdtemp())

    def tearDown(self) -> None:
        shutil.rmtree(self.test_dir)

    def test_merge_profile_list_format(self) -> None:
        profile_path = self.test_dir / "profile.json"
        events = [{"name": "event1", "ph": "C"}]
        with open(profile_path, "w") as f:
            json.dump(events, f)

        main_traces: list[dict[str, Any]] = []
        result = ninjatrace2json.merge_profile(profile_path, main_traces)

        self.assertTrue(result)
        self.assertEqual(main_traces, events)

    def test_merge_profile_dict_format(self) -> None:
        profile_path = self.test_dir / "profile.json"
        events_dict = {"traceEvents": [{"name": "event1", "ph": "C"}]}
        with open(profile_path, "w") as f:
            json.dump(events_dict, f)

        main_traces: list[dict[str, Any]] = []
        result = ninjatrace2json.merge_profile(profile_path, main_traces)

        self.assertTrue(result)
        self.assertEqual(main_traces, [{"name": "event1", "ph": "C"}])


if __name__ == "__main__":
    unittest.main()
