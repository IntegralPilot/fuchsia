#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Metrics processing common code for trace models."""

import abc
import inspect as py_inspect
import logging
from collections.abc import Collection, Sequence
from typing import MutableSequence

from reporting import metrics
from trace_processing import trace_model

_LOGGER: logging.Logger = logging.getLogger("Performance")


class MetricsProcessor(metaclass=abc.ABCMeta):
    """MetricsProcessor converts a trace_model.Model into TestCaseResults.

    This base class is extended to implement various types of metrics.

    MetricsProcessor subclasses can be used as follows:

    ```
    processor = MetricsProcessorSet([
      CpuMetricsProcessor(aggregates_only=True),
      FpsMetricsProcessor(aggregates_only=False),
      MyCustomProcessor(...),
      power_sampler.metrics_processor(),
    ])

    ... gather traces, start and stop the power sampler, create the model ...

    metrics.TestCaseResult.write_fuchsiaperf_json(
        processor.process_metrics(model), test_suite_name, output_path
    )
    ```
    NB: `output_path` must end in `fuchsiaperf.json`
    """

    @property
    def name(self) -> str:
        return self.__class__.__name__

    @property
    def event_patterns(self) -> set[str]:
        """Patterns describing the trace events needed to generate these metrics.

        Metrics may be calculated from kernel scheduler records, named trace events or a combination
        of the two. In order to reduce processing time and memory usage, an implementation must
        provide a set of patterns that describe all the events required to generate metrics. This
        can be as simple as a list of event names, or a full regexp. Filters are applied with OR
        semantics, so if an event passes any filter, it will appear in the final trace model.

        If your metrics processor requires no events, return the empty set.

        Scheduler records cannot be filtered out and will always be present in the traces provided
        for metrics processing.
        """
        return {r".*"}  # Default to requesting all events.

    @property
    def category_names(self) -> set[str]:
        """Categories needed to generate these metrics.

        Metrics may be calculated from kernel scheduler records, named trace events or a combination
        of the two. In order to reduce processing time and memory usage, an implementation must
        provide for excluding unneeded trace events.

        ** Please use `event_patterns` if at all reasonable. **

        Providing a precise set of event patterns enables much more significant RAM savings.
        That said, some metrics require the presence of "flows" in the traces that are made up of
        multiple duration events and flow events all working in concert. In those cases, it can be
        onerous and brittle to specify the full list of events needed for a metric and listing one
        or more categories by name is acceptable.

        As filters are applied using OR semantics, it is not necessary to include an event by both
        category AND by providing a pattern that matches it by name. For the same reason, an event
        cannot be excluded by declining to reference its category here; if the name matches one of
        the provided `event_patterns`, the event will be filtered in.

        Thus, returning the empty set does _not_ filter out all events. Rather, it defers filtering
        entirely to `event_patterns`.

        Scheduler records cannot be filtered out and will always be present in the traces provided
        for metrics processing.
        """
        return set()

    def process_metrics_with_fxt(
        self, fxt_path: str
    ) -> MutableSequence[metrics.TestCaseResult]:
        """Generates metrics from the file at the given fxt_path.

        Args:
            fxt_path: The path of the fxt tracing file.

        Returns:
            The generated metrics.
        """
        return []

    @abc.abstractmethod
    def process_metrics(
        self, model: trace_model.Model
    ) -> Collection[metrics.TestCaseResult]:
        """Generates metrics from the given model.

        Args:
            model: The input trace model.

        Returns:
            The generated metrics.
        """
        raise NotImplementedError()

    def process_freeform_metrics(
        self, model: trace_model.Model
    ) -> tuple[str, metrics.JSON]:
        """Computes freeform metrics as JSON.

        This can output structured data, as opposite to `process_metrics` which return as list.
        These metrics are in addition to those produced by process_metrics()

        This method returns a tuple of (filename, JSON) so that processors can provide an
        identifier more stable than its own classname for use when filing freeform metrics. Since
        filenames are included when freeform metrics are ingested into the metrics backend, basing
        that name on a class name would mean that a refactor could unintentionally break downstream
        consumers of metrics.

        Args:
            model: trace events to be processed.

        Returns:
            str: stable identifier to use in freeform metrics file name.
            JSON: structure holding aggregated metrics, or None if not supported.
        """
        return (self.name, None)

    @classmethod
    def describe(
        cls, data: Sequence[metrics.TestCaseResult]
    ) -> metrics.MetricsProcessorDescription:
        docstring = py_inspect.getdoc(cls)
        assert docstring
        return metrics.MetricsProcessorDescription(
            classname=cls.__name__,
            doc=docstring,
            code_path=py_inspect.getfile(cls),
            line_no=py_inspect.getsourcelines(cls)[1],
            metrics=[tcr.describe() for tcr in data],
        )
