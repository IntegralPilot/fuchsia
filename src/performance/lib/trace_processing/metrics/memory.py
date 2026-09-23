#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Computes metrics from memory traces."""

import collections
import dataclasses
from collections.abc import Collection

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time, trace_utils
from trace_processing.metrics import cpu

MEMORY_SYSTEM_CATEGORY = "memory:kernel"

# The names of threads that perform memory management tasks; CPU time spent in
# these threads gets aggregated into a "management time" cumulative metric.
_MEMORY_MANAGEMENT_THREAD_NAMES = frozenset(
    (
        # TODO(https://fxbug.dev/555184304): Scale computed eviction-thread
        # management time by core count?
        "eviction-thread",
        "kernel-memory-reclaim",
        "memory-pressure-thread",
        "page-queue-lru-thread",
        "page-queue-mru-thread",
        "scanner-request-thread",
        "stall-aggregator",
    )
)

_KERNEL_EVENT_NAMES = (
    "kmem_stats_a",
    "kmem_stats_b",
    "kmem_stats_compression",
    "memory_stall",
)
# Names of some of the metrics that are cumulative, monotonic counters,
# as opposed to gauges.
_CUMULATIVE_METRIC_NAMES = frozenset(
    {
        "compression_time",
        "decompression_time",
        "stall_time_some_ns",
        "stall_time_full_ns",
        "page_refaults",
    }
)


@dataclasses.dataclass(frozen=True)
class _StructuredMetricName:
    structured_name: str
    unit: metrics.Unit


# Names and units of some of the metrics we will export as structured metrics.
# The key is the name of the metric in the trace, and the value is a StructuredMetricName object.
_STRUCTURED_METRIC_NAMES = {
    "stall_time_some_ns": _StructuredMetricName(
        "Memory/System/StallTimeSome", metrics.Unit.nanoseconds
    ),
    "stall_time_full_ns": _StructuredMetricName(
        "Memory/System/StallTimeFull", metrics.Unit.nanoseconds
    ),
    "compression_time": _StructuredMetricName(
        "Memory/System/CompressionTime", metrics.Unit.nanoseconds
    ),
    "decompression_time": _StructuredMetricName(
        "Memory/System/DecompressionTime", metrics.Unit.nanoseconds
    ),
    "page_refaults": _StructuredMetricName(
        "Memory/System/PageRefaults", metrics.Unit.count
    ),
    "total_heap_bytes": _StructuredMetricName(
        "Memory/System/ZirconHeapBytes", metrics.Unit.bytes
    ),
}

_MANAGEMENT_STRUCTURED_METRIC_NAME = _StructuredMetricName(
    "Memory/System/ManagementTime", metrics.Unit.milliseconds
)


def _safe_divide(numerator: float, denominator: float) -> float | None:
    """Divides numerator by denominator, returning None if denominator is 0."""
    if denominator == 0:
        return None
    else:
        return numerator / denominator


def _cumulative_metrics_value(
    values: list[tuple[trace_time.TimePoint, int | float]]
) -> tuple[int | float, float | None]:
    """Returns the change and the rate for the specified cumulative metric."""
    (t0, v0), (t1, v1) = values[0], values[-1]
    return (v1 - v0, _safe_divide(v1 - v0, (t1 - t0).to_nanoseconds()))


def _cumulative_metrics_json(
    values: list[tuple[trace_time.TimePoint, int | float]]
) -> metrics.JSON:
    """Returns a JSON object holding the change and the rate for the specified cumulative metric."""
    (delta, rate) = _cumulative_metrics_value(values)
    return {
        "Delta": delta,
        "Rate": rate,
    }


def _gauges_metrics_values(
    values: list[tuple[trace_time.TimePoint, int | float]]
) -> list[int | float]:
    """Returns a JSON object holding the standard metric value keyed by metric name."""
    return list(v[1] for v in values)


def _gauges_metrics_json(
    values: list[tuple[trace_time.TimePoint, int | float]]
) -> metrics.JSON:
    """Returns a JSON object holding the standard metric value keyed by metric name."""
    results = trace_utils.standard_metrics_set(
        values=_gauges_metrics_values(values),
        label_prefix="",
        unit=metrics.Unit.bytes,
    )
    return {result.label: result.values[0] for result in results}


class MemoryMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes statistics for values published by zircon kernel.

    Returns both freeform metrics and structured metrics.

    Freeform metrics is a JSON object with nested structure with the following path:

    "kernel" / field name / statistic label / float value

    Where:
        kernel: constant "kernel"
        field name: name of a field from `zx_info_kmem_stats_extended` and
            `zx_info_kmem_stats_compression` structures.
        statistic name: label of the statistic published by `trace_utils.standard_metrics_set`.
        float value: metric value.

    Sample:

    {
        "kernel": {
            "total_bytes": {
                "Min": 112
                "P5": 130
                [...]
            } ,
            [...]
        }
    }
    """

    @property
    def event_patterns(self) -> set[str]:
        """Patterns describing the trace events needed to generate these metrics."""
        return set(_KERNEL_EVENT_NAMES)

    FREEFORM_METRICS_FILENAME = "memory"

    def process_freeform_metrics(
        self, model: trace_model.Model
    ) -> tuple[str, metrics.JSON]:
        series_by_name = collections.defaultdict(list)
        for event in trace_utils.filter_events(
            model.all_events(),
            name=_KERNEL_EVENT_NAMES,
            type=trace_model.CounterEvent,
        ):
            for name, value in event.args.items():
                series_by_name[name].append((event.start, value))

        return (
            self.FREEFORM_METRICS_FILENAME,
            dict(
                kernel={
                    name: _cumulative_metrics_json(series)
                    if name in _CUMULATIVE_METRIC_NAMES
                    else _gauges_metrics_json(series)
                    for name, series in series_by_name.items()
                }
            ),
        )

    def process_metrics(
        self, model: trace_model.Model
    ) -> Collection[metrics.TestCaseResult]:
        series_by_name = collections.defaultdict(list)
        for event in trace_utils.filter_events(
            model.all_events(),
            name=_KERNEL_EVENT_NAMES,
            type=trace_model.CounterEvent,
        ):
            for name, value in event.args.items():
                if name in _STRUCTURED_METRIC_NAMES:
                    series_by_name[name].append((event.start, value))

        results = []
        for name, series in series_by_name.items():
            if name in _CUMULATIVE_METRIC_NAMES:
                values = [_cumulative_metrics_value(series)[0]]
            else:
                values = _gauges_metrics_values(series)
            results.append(
                metrics.TestCaseResult(
                    label=_STRUCTURED_METRIC_NAMES[name].structured_name,
                    values=values,
                    unit=_STRUCTURED_METRIC_NAMES[name].unit,
                )
            )

        # TODO(https://fxbug.dev/555204260): factor this properly; it's
        # untoward to be calling one MetricsProcessor from within the
        # implementation of another.
        (
            breakdown,
            unused_total_time,
        ) = cpu.CpuMetricsProcessor().process_metrics_and_get_total_time(model)
        total_management_time = 0.0
        for metric in breakdown:
            if metric.get("thread_name") in _MEMORY_MANAGEMENT_THREAD_NAMES:
                duration = metric.get("duration")
                assert isinstance(duration, (int, float))
                total_management_time += float(duration)
        results.append(
            metrics.TestCaseResult(
                label=_MANAGEMENT_STRUCTURED_METRIC_NAME.structured_name,
                values=(total_management_time,),
                unit=_MANAGEMENT_STRUCTURED_METRIC_NAME.unit,
                direction=metrics.Direction.smallerIsBetter,
            )
        )

        return results
