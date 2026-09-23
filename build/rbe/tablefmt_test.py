#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import tablefmt


class CreateRowTests(unittest.TestCase):
    def test_construction(self) -> None:
        init = 11
        row = tablefmt.create_row(3, init)
        self.assertEqual(row, [init, init, init])
        # also make sure cells are not aliased to the same reference
        row[0] = 9
        row[1] = 8
        row[2] = 7
        self.assertEqual(row, [9, 8, 7])


class CreateTableTests(unittest.TestCase):
    def test_construction(self) -> None:
        init = 42
        table = tablefmt.create_table(2, 3, init)
        self.assertEqual(table, [[init, init, init], [init, init, init]])
        # also make sure cells are not aliased to the same reference
        table[0][0] = 5
        table[0][1] = 4
        table[0][2] = 3
        table[1][0] = 9
        table[1][1] = 8
        table[1][2] = 7
        self.assertEqual(table, [[5, 4, 3], [9, 8, 7]])


class HumanReadableSizeTests(unittest.TestCase):
    def test_formatting(self) -> None:
        test_data = [
            # size, unit, dec, expected
            (0, "", 1, "0.0 "),
            (0, "B", 2, "0.00 B"),
            (1000, "B", 1, "1000.0 B"),
            (2048, "B", 1, "2.0 KiB"),
            (44444444, "B", 1, "42.4 MiB"),
            (1234567890, "B", 1, "1.1 GiB"),
        ]
        for size, unit, dec, expected in test_data:
            self.assertEqual(
                tablefmt.human_readable_size(size, unit, dec), expected
            )


class AutoSizeColumnWidthsTests(unittest.TestCase):
    def test_strings(self) -> None:
        table = [["foo", "cat", "longcat"], ["bar", "frog", "dog"]]
        widths = tablefmt.auto_size_column_widths(table)
        self.assertEqual(widths, [3, 4, 7])

    def test_integers(self) -> None:
        table = [[0, 144, 8128], [496, 6, 28]]
        widths = tablefmt.auto_size_column_widths(table)
        self.assertEqual(widths, [3, 3, 4])


class MakeTableHeaderTests(unittest.TestCase):
    def test_with_title(self) -> None:
        self.assertEqual(
            tablefmt.make_table_header(["arms", "legs"], "title"),
            ["title", "arms", "legs"],
        )

    def test_no_title(self) -> None:
        self.assertEqual(
            tablefmt.make_table_header(["cats", "bears"]), ["", "cats", "bears"]
        )


class MakeSeparatorRowTests(unittest.TestCase):
    def test_with_title_and_fill(self) -> None:
        self.assertEqual(
            tablefmt.make_separator_row(2, title="lang", fill="***"),
            ["lang", "***", "***"],
        )

    def test_blank(self) -> None:
        self.assertEqual(tablefmt.make_separator_row(3), ["", "", "", ""])


class MakeRowFormatterTests(unittest.TestCase):
    def test_with_separator(self) -> None:
        self.assertEqual(
            tablefmt.make_row_formatter([">", "^", "<"], [2, 3, 4], " | "),
            "{0:>2} | {1:^3} | {2:<4}",
        )


class FormatNumericTableTests(unittest.TestCase):
    def test_format(self) -> None:
        table: list[list[str | int]] = [
            ["Title", "a", "b"],
            ["Alice", 20, 5],
            ["Bob", 101, 2048],
        ]
        row_text = list(tablefmt.format_numeric_table(table))
        self.assertEqual(
            row_text,
            [
                "Title   a    b",
                "Alice  20    5",
                "Bob   101 2048",
            ],
        )


if __name__ == "__main__":
    unittest.main()
