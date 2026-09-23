#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""General table-formatting utilities.

Tables are just Sequence[Sequence[Any]], where the number of elements
in each row is required to be the same.
"""

from collections.abc import Iterable, Sequence
from typing import Any


def create_row(num_cols: int, init: Any = None) -> list[Any]:
    """Create and populate one row with initial values."""
    # Use list comprehension instead of list replication to avoid referencing
    # the same object from each cell.
    return [init for _ in range(num_cols)]


def create_table(
    num_rows: int, num_cols: int, init: Any = None
) -> list[list[Any]]:
    """Create and populate a sized table with initial values."""
    # Use list comprehension instead of list replication to avoid referencing
    # the same object from each cell.
    return [create_row(num_cols, init) for _ in range(num_rows)]


def human_readable_size(
    size: int, unit: str = "B", decimal_places: int = 2
) -> str:
    """Print number in human-readable notation, like KiB, MiB.

    adapted from source:
      https://stackoverflow.com/questions/1094841/get-human-readable-version-of-file-size

    Args:
      size: number (any numeric type)
      unit: e.g. "B" for bytes
      decimal_places: precision for formatted number.

    Returns:
      formatted number (str)
    """
    size_float = float(size)
    for suffix in ["", "Ki", "Mi", "Gi", "Ti", "Pi"]:
        if size_float < 1024.0 or suffix == "Pi":
            break
        size_float = size_float / 1024
    return f"{size_float:.{decimal_places}f} {suffix}{unit}"


def auto_size_column_widths(table: Sequence[Sequence[Any]]) -> Sequence[int]:
    """Given a row-major table, compute the max-width of each column.

    Widths are determined using the str() representation of each cell.
    You can also pass string values if you've already pre-formatted
    numbers to a specific representation, e.g. using `human_readable_size()`.

    Returns:
      Sequence of column widths.
    """
    return [
        max(*[len(str(row[c])) for row in table]) for c in range(len(table[0]))
    ]


def make_table_header(columns: Sequence[str], title: str = "") -> Sequence[str]:
    """Build a table row that can be used as a table header."""
    return [title] + list(columns)


def make_separator_row(
    num_columns: int, title: str = "", fill: str = ""
) -> Sequence[str]:
    """Build a table row that can be used as a vertical separator."""
    return [title] + ([fill] * num_columns)


def make_row_formatter(
    alignments: Sequence[str], column_widths: Sequence[int], separator: str
) -> str:
    """Construct a format string based on desired cell alignments and widths.

    Args:
      alignments: '<' for left, '^' for center, '>' for right (str.format()).

    Returns:
      A .format()-able string that expects row values as positional arguments.
      Typical use: result.format(*row_or_array)
    """
    return separator.join(
        f"{{{i}:{align}{width}}}"  # positional index, alignment specifier, width
        for i, (align, width) in enumerate(zip(alignments, column_widths))
    )


def format_numeric_table(table: Sequence[Sequence[str | int]]) -> Iterable[str]:
    # Format table using fitted column widths.
    column_widths = auto_size_column_widths(table)
    alignments = ["<"] + [">"] * (
        len(column_widths) - 1
    )  # left, followed by rights
    row_formatter = make_row_formatter(alignments, column_widths, " ")
    for row in table:
        yield row_formatter.format(*row)
