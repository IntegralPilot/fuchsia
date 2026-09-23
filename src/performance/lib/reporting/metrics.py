# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Metrics reporting helpers.

This module implements the perf test results schema.

See https://fuchsia.dev/fuchsia-src/development/performance/fuchsiaperf_format
for more details.
"""

import dataclasses
import enum
import inspect as py_inspect
import json
import logging
import pathlib
from typing import (
    Any,
    Callable,
    Iterable,
    Mapping,
    MutableMapping,
    MutableSequence,
    NotRequired,
    Self,
    Sequence,
    TypeAlias,
    TypedDict,
)

_LOGGER: logging.Logger = logging.getLogger("Reporting")

JSON: TypeAlias = (
    Mapping[str, "JSON"] | Sequence["JSON"] | str | int | float | bool | None
)


class Direction(enum.StrEnum):
    biggerIsBetter = "biggerIsBetter"
    smallerIsBetter = "smallerIsBetter"


class Unit(enum.Enum):
    """The set of valid Unit constants.

    This should be kept in sync with the list of supported units in the results
    schema docs linked at the top of this file. These are the unit strings
    accepted by catapult_converter.
    """

    # Time-based units.
    nanoseconds = "nanoseconds"
    milliseconds = "milliseconds"
    # Size-based units.
    bytes = "bytes"
    bytesPerSecond = "bytes/second"
    # Frequency-based units.
    framesPerSecond = "frames/second"
    # Percentage-based units.
    percent = "percent"
    # Count-based units. ("count" can't be used a StrEnum)
    count = "count"
    # Power-based units.
    watts = "W"


class MetricDescription(TypedDict):
    """Describes a single metric."""

    name: str
    doc: str


class MetricsProcessorDescription(TypedDict):
    """Documents a single metrics processor."""

    classname: NotRequired[str]
    doc: str
    code_path: str
    line_no: int
    metrics: list[MetricDescription]


@dataclasses.dataclass(frozen=True)
class TestCaseResult:
    """The results for a single test case.

    See the link at the top of this file for documentation.
    """

    label: str
    unit: Unit
    values: tuple[float, ...]
    doc: str
    # TODO(https://fxbug.dev/483530722) require this field be non-null
    direction: Direction | None = None

    def __init__(
        self,
        label: str,
        unit: Unit,
        values: Sequence[float],
        doc: str = "",
        direction: Direction | None = None,
    ):
        """Allows any Sequence to be used for values while staying hashable."""
        object.__setattr__(self, "label", label)
        object.__setattr__(self, "unit", unit)
        object.__setattr__(self, "values", tuple(values))
        object.__setattr__(self, "doc", doc)
        # If unspecified, Capapult has its own notion of which units default
        # to which directions which we defer to for compatibility.
        if direction:
            object.__setattr__(self, "direction", direction)

    def to_json(self, test_suite: str) -> dict[str, Any]:
        return {
            "label": self.label,
            "test_suite": test_suite,
            "unit": str(
                self.unit.value
                + (f"_{self.direction}" if self.direction else "")
            ),
            "values": list(self.values),
        }

    def describe(self) -> MetricDescription:
        return MetricDescription(name=self.label, doc=self.doc)

    @staticmethod
    def write_fuchsiaperf_json(
        results: Iterable["TestCaseResult"],
        test_suite: str,
        output_path: pathlib.Path,
    ) -> None:
        """Writes the given TestCaseResults into a fuchsiaperf json file.

        Args:
            results: The results to write.
            test_suite: A test suite name to embed in the json.
                E.g. "fuchsia.uiperf.my_metric".
            output_path: Output file path, must end with ".fuchsiaperf.json".
        """
        assert output_path.name.endswith(
            ".fuchsiaperf.json"
        ), f"Expecting path that ends with '.fuchsiaperf.json' but got {output_path}"
        results_json = [r.to_json(test_suite) for r in results]
        with open(output_path, "w") as outfile:
            json.dump(results_json, outfile, indent=4)
        _LOGGER.info(f"Wrote {len(results_json)} results into {output_path}")


class SuiteDescription(TypedDict):
    """Description of a test suite class."""

    test_suite: str
    test_class: str
    doc: str
    code_path: str
    line_no: int


class SuiteDocumentation(SuiteDescription):
    """Documentation for a test suite, including code that generates metrics."""

    metrics_processors: list[MetricsProcessorDescription]


@dataclasses.dataclass
class Report:
    """Metrics data for a test suite."""

    structured: MutableSequence[TestCaseResult]
    """Named single-value (or list-of-values) metrics."""
    freeform: MutableMapping[str, JSON] = dataclasses.field(
        default_factory=dict
    )
    """Mapping of strings to arbitrary JSON data."""
    metrics_processors: list[MetricsProcessorDescription] = dataclasses.field(
        default_factory=list
    )
    """Descriptions of the code that captured the metrics in this report."""

    def extend(self, other: Self) -> Self:
        self.structured.extend(other.structured)
        self.freeform.update(other.freeform)
        self.metrics_processors.extend(other.metrics_processors)
        return self

    def generate_docs(self, desc: SuiteDescription) -> SuiteDocumentation:
        return SuiteDocumentation(
            **desc, metrics_processors=self.metrics_processors
        )


class CallableDescription(TypedDict):
    """Documentation for a callable used to generate metrics."""

    doc: str
    code_path: str
    line_no: int


def describe_callable(c: Callable[..., Any]) -> CallableDescription:
    docstring = py_inspect.getdoc(c)
    assert docstring, c
    return CallableDescription(
        doc=docstring,
        code_path=py_inspect.getfile(c),
        line_no=py_inspect.getsourcelines(c)[1],
    )
