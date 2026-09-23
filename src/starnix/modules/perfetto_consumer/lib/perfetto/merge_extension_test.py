# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

from merge_extension import (
    extract_message_def,
    find_conflict,
    find_oneof_data_bounds,
    is_in_extensions,
    merge_extension_field,
    remove_field_from_extensions,
)


class TestMergeExtension(unittest.TestCase):
    def test_find_oneof_data_bounds(self) -> None:
        proto_standard = (
            "message TracePacket {\n"
            "  oneof data {\n"
            "    string test = 1;\n"
            "  }\n"
            "}\n"
        )
        start, end = find_oneof_data_bounds(proto_standard)
        self.assertEqual(
            proto_standard[start : end + 1],
            "oneof data {\n    string test = 1;\n  }",
        )

        proto_varied_spaces = (
            "message TracePacket {\n"
            "  oneof    data   \n"
            "  {\n"
            "    string test = 1;\n"
            "  }\n"
            "}\n"
        )
        start2, end2 = find_oneof_data_bounds(proto_varied_spaces)
        self.assertEqual(
            proto_varied_spaces[start2 : end2 + 1],
            "oneof    data   \n  {\n    string test = 1;\n  }",
        )

    def test_is_in_extensions(self) -> None:
        self.assertTrue(is_in_extensions("  extensions 76;\n", "76"))
        self.assertTrue(
            is_in_extensions("  extensions 10 to 20, 76, 100 to 200;\n", "76")
        )
        self.assertTrue(is_in_extensions("  extensions 70 to 80;\n", "76"))
        self.assertTrue(is_in_extensions("  extensions 1000 to max;\n", "1500"))
        # Word boundary check: field 76 should not match 760 to 800
        self.assertFalse(is_in_extensions("  extensions 760 to 800;\n", "76"))
        self.assertFalse(is_in_extensions("  extensions 176;\n", "76"))

    def test_find_conflict(self) -> None:
        oneof_content = (
            "  oneof data {\n"
            "    FrameTimelineEvent frame_timeline_event = 76;\n"
            "    string other_field = 100;\n"
            "  }\n"
        )
        self.assertEqual(
            find_conflict(oneof_content, "76"),
            ("FrameTimelineEvent", "frame_timeline_event"),
        )
        self.assertIsNone(find_conflict(oneof_content, "99"))

        # Test field with options
        oneof_content_with_options = (
            "  oneof data {\n"
            '    FrameTimelineEvent frame_timeline_event = 76 [json_name = "frameTimelineEvent"];\n'
            "  }\n"
        )
        self.assertEqual(
            find_conflict(oneof_content_with_options, "76"),
            ("FrameTimelineEvent", "frame_timeline_event"),
        )

        # Test commented out fields are ignored
        oneof_commented = (
            "  oneof data {\n"
            "    // FrameTimelineEvent frame_timeline_event = 76;\n"
            "    /* string block_comment = 76; */\n"
            "  }\n"
        )
        self.assertIsNone(find_conflict(oneof_commented, "76"))

    def test_find_conflict_ignores_other_messages(self) -> None:
        # Field 76 is defined in FtraceEvent, but in oneof data we have different fields
        oneof_content = (
            "  oneof data {\n"
            "    string test = 1;\n"
            "    int32 something_else = 2;\n"
            "  }\n"
        )
        # It should not find any conflict with ID 76 in oneof_content
        self.assertIsNone(find_conflict(oneof_content, "76"))

    def test_remove_field_from_extensions(self) -> None:
        cases = [
            ("  extensions 76;\n", "76", ""),
            (
                "  extensions 76, 100 to 200;\n",
                "76",
                "  extensions 100 to 200;\n",
            ),
            ("  extensions 10 to 20, 76;\n", "76", "  extensions 10 to 20;\n"),
            (
                "  extensions 10 to 20, 76, 100 to 200;\n",
                "76",
                "  extensions 10 to 20, 100 to 200;\n",
            ),
            (
                "  extensions\n    76,\n    100 to 200;\n",
                "76",
                "  extensions\n    100 to 200;\n",
            ),
            (
                "  extensions\n    10 to 20,\n    76;\n",
                "76",
                "  extensions\n    10 to 20;\n",
            ),
            (
                "  extensions\n    10 to 20,\n    76,\n    100 to 200;\n",
                "76",
                "  extensions\n    10 to 20,\n    100 to 200;\n",
            ),
            (
                "  extensions 70 to 80;\n",
                "76",
                "  extensions 70 to 75, 77 to 80;\n",
            ),
            (
                "  extensions 1000 to max;\n",
                "1000",
                "  extensions 1001 to max;\n",
            ),
            ("  extensions 70 to 76;\n", "76", "  extensions 70 to 75;\n"),
            ("  extensions 76 to 80;\n", "76", "  extensions 77 to 80;\n"),
            (
                "  // extensions 76;\n  extensions 76;\n",
                "76",
                "  // extensions 76;\n",
            ),
        ]
        for input_content, field_id, expected in cases:
            actual = remove_field_from_extensions(input_content, field_id)
            self.assertEqual(actual, expected)

    def test_merge_extension_field(self) -> None:
        input_proto = (
            "message TracePacket {\n"
            "  oneof data {\n"
            "    string test = 1;\n"
            "  }\n"
            "}\n"
        )
        expected = (
            "message TracePacket {\n"
            "  oneof data {\n"
            "    string test = 1;\n"
            "    FrameTimelineEvent frame_timeline_event = 76;\n"
            "  }\n"
            "}\n"
        )
        actual = merge_extension_field(
            input_proto, "FrameTimelineEvent", "frame_timeline_event", "76"
        )
        self.assertEqual(actual, expected)

        input_proto_4spaces = (
            "message TracePacket {\n"
            "    oneof data {\n"
            "        string test = 1;\n"
            "    }\n"
            "}\n"
        )
        expected_4spaces = (
            "message TracePacket {\n"
            "    oneof data {\n"
            "        string test = 1;\n"
            "        FrameTimelineEvent frame_timeline_event = 76;\n"
            "    }\n"
            "}\n"
        )
        actual_4spaces = merge_extension_field(
            input_proto_4spaces,
            "FrameTimelineEvent",
            "frame_timeline_event",
            "76",
        )
        self.assertEqual(actual_4spaces, expected_4spaces)

    def test_extract_message_def(self) -> None:
        ext_content = (
            "message FrameTimelineEvent {\n"
            "  enum JankType {\n"
            "    JANK_NONE = 1;\n"
            "  }\n"
            "}\n"
            "\n"
            "message FrameworksNativeTracePacket {\n"
            "  extend perfetto.protos.TracePacket {\n"
            "    optional FrameTimelineEvent frame_timeline_event = 76;\n"
            "    optional EvdevEvent evdev_event = 121;\n"
            "  }\n"
            "}\n"
        )
        expected = (
            "message FrameTimelineEvent {\n"
            "  enum JankType {\n"
            "    JANK_NONE = 1;\n"
            "  }\n"
            "}"
        )

        actual = extract_message_def(ext_content, "FrameTimelineEvent")
        self.assertEqual(actual, expected)

    def test_find_oneof_data_bounds_with_comments(self) -> None:
        """Verifies that braces inside comments or strings do not corrupt bounds detection."""
        proto_with_comments = (
            "message TracePacket {\n"
            "  oneof data {\n"
            "    // Tricky { brace in comment\n"
            '    string test = 1; /* and a } in block comment with "{string_brace}" */\n'
            "  }\n"
            "}\n"
        )
        start, end = find_oneof_data_bounds(proto_with_comments)
        extracted = proto_with_comments[start : end + 1]
        self.assertTrue(extracted.startswith("oneof data {"))
        self.assertTrue(extracted.endswith("}"))
        self.assertIn("string test = 1;", extracted)

    def test_find_conflict_multiline_declaration(self) -> None:
        """Verifies conflict detection when a field declaration spans multiple lines."""
        oneof_multiline = (
            "  oneof data {\n"
            "    FrameTimelineEvent frame_timeline_event =\n"
            '        76 [json_name = "frameTimelineEvent"];\n'
            "  }\n"
        )
        self.assertEqual(
            find_conflict(oneof_multiline, "76"),
            ("FrameTimelineEvent", "frame_timeline_event"),
        )

    def test_is_in_extensions_malformed_ranges(self) -> None:
        """Verifies that malformed range strings are safely ignored."""
        self.assertFalse(is_in_extensions("  extensions 100 to;\n", "100"))
        self.assertFalse(is_in_extensions("  extensions to 200;\n", "200"))
        self.assertFalse(is_in_extensions("  extensions 200 to 100;\n", "150"))
        self.assertFalse(is_in_extensions("  extensions max to 1000;\n", "500"))

    def test_merge_extension_field_empty_block_indentation(self) -> None:
        """Verifies indentation calculation when oneof data is completely empty."""
        input_proto_2spaces = (
            "message TracePacket {\n" "  oneof data {\n" "  }\n" "}\n"
        )
        expected_2spaces = (
            "message TracePacket {\n"
            "  oneof data {\n"
            "    FrameTimelineEvent frame_timeline_event = 76;\n"
            "  }\n"
            "}\n"
        )
        actual_2spaces = merge_extension_field(
            input_proto_2spaces,
            "FrameTimelineEvent",
            "frame_timeline_event",
            "76",
        )
        self.assertEqual(actual_2spaces, expected_2spaces)

        input_proto_4spaces = (
            "message TracePacket {\n" "    oneof data {\n" "    }\n" "}\n"
        )
        expected_4spaces = (
            "message TracePacket {\n"
            "    oneof data {\n"
            "      FrameTimelineEvent frame_timeline_event = 76;\n"
            "    }\n"
            "}\n"
        )
        actual_4spaces = merge_extension_field(
            input_proto_4spaces,
            "FrameTimelineEvent",
            "frame_timeline_event",
            "76",
        )
        self.assertEqual(actual_4spaces, expected_4spaces)

    def test_find_conflict_with_urls_in_options(self) -> None:
        """Verifies that // in URLs or string literals is not treated as a comment."""
        oneof_with_url = (
            "  oneof data {\n"
            '    FrameTimelineEvent frame_timeline_event = 76 [doc = "http://fuchsia.dev/schema"];\n'
            "  }\n"
        )
        self.assertEqual(
            find_conflict(oneof_with_url, "76"),
            ("FrameTimelineEvent", "frame_timeline_event"),
        )

    def test_remove_field_from_extensions_ignores_comments(self) -> None:
        """Verifies that commented-out extension statements are not matched or modified."""
        proto_with_commented_ext = (
            "message TracePacket {\n"
            "  /*\n"
            "    extensions 76;\n"
            "  */\n"
            "  extensions 70 to 80;\n"
            "}\n"
        )
        expected = (
            "message TracePacket {\n"
            "  /*\n"
            "    extensions 76;\n"
            "  */\n"
            "  extensions 70 to 75, 77 to 80;\n"
            "}\n"
        )
        actual = remove_field_from_extensions(proto_with_commented_ext, "76")
        self.assertEqual(actual, expected)


if __name__ == "__main__":
    unittest.main()
