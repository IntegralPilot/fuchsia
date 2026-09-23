# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from collections import OrderedDict
from dataclasses import dataclass, field
from enum import StrEnum


class Category(StrEnum):
    ENVIRONMENT = "environmentInfo"
    SOURCE = "sourceInfo"
    BUILD = "buildInfo"

    def pretty_str(self) -> str:
        if self == Category.ENVIRONMENT:
            return "Environment Info"
        elif self == Category.SOURCE:
            return "Source Info"
        elif self == Category.BUILD:
            return "Build Info"


@dataclass(frozen=True)
class Item:
    """An individual item that will be displayed in the output."""

    # The category of the item. Items with matching categories are
    # grouped together.
    category: Category

    # The key of the item, used in the JSON output.
    key: str

    # The title of the item, shown in the text output.
    title: str

    # The value of the item.
    value: str | int | float | None | bool | list[str]

    # Additional notes on the source of this value, optionally.
    notes: str | None = None

    def to_dict(
        self,
    ) -> OrderedDict[str, str | None | bool | int | float | list[str]]:
        """Convert this item to a dictionary that preserves key ordering."""
        return OrderedDict(title=self.title, value=self.value, notes=self.notes)

    def normalized_value(self) -> str:
        """Format the value slightly different than the default.

        - Bools are formatted lowercase.
        - Lists do not have quotes surrounding string members.

        All other values are represented normally using their __str__
        implementation.

        Returns:
            str: Formatted string value
        """
        if isinstance(self.value, list):
            list_val = ", ".join(self.value)
            return f"[{list_val}]"
        elif isinstance(self.value, bool):
            return "true" if self.value else "false"
        else:
            return str(self.value)


@dataclass(frozen=True)
class Result:
    """A wrapper for a result of a collector, which can either be
    an item to display or an error.
    """

    item: Item | None = None
    error: str | None = None


@dataclass
class Data:
    """Data accumulated from multiple collectors."""

    items: OrderedDict[Category, list[Item]] = field(
        default_factory=OrderedDict
    )

    errors: list[str] = field(default_factory=list)

    def add_results(self, results: list[Result]) -> None:
        """Aggregate results into this container.

        Args:
            results (list[Result]): Data to add.
        """
        for value in results:
            assert value.error is None or value.item is None
            if value.error is not None:
                self.errors.append(value.error)
            elif value.item is not None:
                item = value.item
                if item.category not in self.items:
                    self.items[item.category] = []
                self.items[item.category].append(item)
