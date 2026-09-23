# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Diagnostic analysis plugins for perf-analyze."""

import argparse
import io
from dataclasses import dataclass
from typing import Any, NoReturn, Protocol, Sequence, runtime_checkable


class PluginArgumentError(Exception):
    """Exception raised when plugin arguments are invalid."""


class PluginArgumentParser(argparse.ArgumentParser):
    """Custom ArgumentParser that raises PluginArgumentError instead of calling sys.exit."""

    def error(self, message: str) -> NoReturn:
        fp = io.StringIO()
        self.print_usage(fp)
        usage = fp.getvalue().strip()
        raise PluginArgumentError(f"{usage}\nError: {message}")


@dataclass
class SectionResult:
    """A named section within a batch query or analysis plugin result.

    Attributes:
        name: The title or identifier of the section.
        results: Optional list of row dictionaries representing tabular query data.
        error: Optional error message string if the query/analysis failed.
        note: Optional contextual note or diagnostic recommendation.
    """

    name: str
    results: Sequence[dict[str, Any]] | None = None
    error: str | None = None
    note: str | None = None

    def to_dict(self) -> dict[str, Any]:
        """Converts the SectionResult to a dictionary representation."""
        data: dict[str, Any] = {"name": self.name}
        if self.note is not None:
            data["note"] = self.note
        if self.error is not None:
            data["error"] = self.error
        elif self.results is not None:
            data["results"] = self.results
        return data

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "SectionResult":
        """Creates a SectionResult from a dictionary representation."""
        return cls(
            name=data.get("name", ""),
            results=data.get("results"),
            error=data.get("error"),
            note=data.get("note"),
        )


@runtime_checkable
class AnalyzePlugin(Protocol):
    """Protocol defining the interface for performance analysis plugins."""

    name: str
    description: str

    def analyze(
        self,
        remaining_args: Sequence[str],
        trace_path: str,
        cache: bool = True,
    ) -> Sequence[SectionResult]:
        """Executes the analysis specified by args on the given trace."""
        ...
