#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import bisect
import collections
import dataclasses
import itertools
import logging
import sys
from collections.abc import Mapping, Sequence
from typing import (
    Iterable,
    Iterator,
    MutableSequence,
    NotRequired,
    Self,
    TypeAlias,
    TypedDict,
    cast,
)

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time, trace_utils

_LOGGER: logging.Logger = logging.getLogger(__name__)
_CPU_USAGE_EVENT_NAME = "cpu_usage"
_PROCESSING_RATE_EVENT_NAME = "Processing Rate"
# The kernel reports processing rates where 1000 represents 100% capacity.
# This constant is used as a default value when rate events are missing.
_DEFAULT_PROCESSING_RATE = 1000.0
_DEFAULT_PERCENT_CUTOFF = 0.0
_ONE_S_IN_NS = 1_000_000_000
_NS_PER_MS = 1_000_000.0


class BreakdownMetric(TypedDict):
    """Internal representation of a single CPU breakdown entry."""

    process_name: str
    thread_name: NotRequired[str]
    tid: NotRequired[int]
    cpu: int
    percent: float
    duration: float
    normalized_percent: NotRequired[float]
    normalized_duration: NotRequired[float]
    duration_per_rate: NotRequired[dict[str, float]]


Breakdown: TypeAlias = list[dict[str, metrics.JSON]]


@dataclasses.dataclass(frozen=True)
class ProcessingRateSample:
    """A single processing rate sample at a given timestamp."""

    timestamp_ms: float
    rate: float  # ranges from 0 to 1000, where 1000 represents 100% processing rate of the CPU.


@dataclasses.dataclass(frozen=True)
class VirtualProcessingSlice:
    """A slice with a constant processing rate."""

    duration: float
    rate: float


class CpuProcessingRateTimeline:
    """Encapsulates the processing rate shifts for a single CPU core."""

    def __init__(self, rates: Iterable[ProcessingRateSample]):
        """Constructor.

        Args:
            rates: An iterable of ProcessingRateSample objects.
        """
        self.rates = sorted(rates, key=lambda r: r.timestamp_ms)

    def get_virtual_slices(
        self, start_ts: float, stop_ts: float
    ) -> list[VirtualProcessingSlice]:
        """Splits a schedule slice into virtual slices based on rate changes.

        Args:
            start_ts: Start of the slice (ms).
            stop_ts: End of the slice (ms).

        Returns:
            A list of VirtualProcessingSlice objects.
        """
        if start_ts >= stop_ts:
            return []

        virtual_slices: list[VirtualProcessingSlice] = []
        curr_ts = start_ts

        idx = bisect.bisect_right(
            self.rates, curr_ts, key=lambda r: r.timestamp_ms
        )
        current_rate = (
            _DEFAULT_PROCESSING_RATE if idx == 0 else self.rates[idx - 1].rate
        )

        while curr_ts < stop_ts:
            next_change_ts = (
                self.rates[idx].timestamp_ms
                if idx < len(self.rates)
                else float("inf")
            )
            seg_end = min(stop_ts, next_change_ts)

            virtual_slices.append(
                VirtualProcessingSlice(
                    duration=seg_end - curr_ts,
                    rate=current_rate,
                )
            )

            curr_ts = seg_end
            if curr_ts >= next_change_ts:
                current_rate = self.rates[idx].rate
                idx += 1

        return virtual_slices


@dataclasses.dataclass
class ProcessingRateStats:
    """Aggregates durations spent at different processing rates."""

    duration_per_rate: dict[str, float] = dataclasses.field(
        default_factory=dict
    )

    def add(self, rate: float, duration: float) -> None:
        rate_str = str(int(rate))  # One of "1000", "798", "506", and "360"
        self.duration_per_rate[rate_str] = (
            self.duration_per_rate.get(rate_str, 0.0) + duration
        )


@dataclasses.dataclass
class ThreadCpuStats:
    """Encapsulates CPU usage statistics for a single thread on a single CPU."""

    duration: float = 0.0
    normalized_duration: float = 0.0
    rate_stats: ProcessingRateStats = dataclasses.field(
        default_factory=ProcessingRateStats
    )


class CpuMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes CPU utilization metrics, both structured and freeform.

    CPU load metrics are reported as a percentage of the total load across all cores, with 100%
    mapping to full utilization of every core simultaneously. Some tools will report a process using
    both cores on a dual core system as taking up 200% of CPU; that is not the case here. Depending
    on configuration, this module will report a time series of average utiliztion samples taken
    periodically for the duration of the test, or aggregate measurements-- such as min, max,
    average, and percentiles -- calculated over that data.

    Freeform CPU load metrics report per-core average load for every thread in every process on the
    system. These data are not suited for automated changepoint detection, and so are instead
    piped to a dashboard.
    """

    FREEFORM_METRICS_FILENAME = "cpu_breakdown"

    def __init__(
        self,
        aggregates_only: bool = True,
        percent_cutoff: float = _DEFAULT_PERCENT_CUTOFF,
    ):
        """Constructor.

        Args:
            aggregates_only: When True, generates CpuMin, CpuMax, CpuAverage and CpuP* (%iles).
                Otherwise generates CpuLoad metric with all cpu values.
            percent_cutoff: Any process that has CPU below this won't be listed in the CPU usage
                breakdown reported in freeform metrics.
        """
        self.aggregates_only: bool = aggregates_only
        self._percent_cutoff = percent_cutoff

    @property
    def event_patterns(self) -> set[str]:
        return {_CPU_USAGE_EVENT_NAME, f"{_PROCESSING_RATE_EVENT_NAME}.*"}

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        all_events: Iterator[trace_model.Event] = model.all_events()
        cpu_usage_events: Iterable[
            trace_model.CounterEvent
        ] = trace_utils.filter_events(
            all_events,
            name=_CPU_USAGE_EVENT_NAME,
            type=trace_model.CounterEvent,
        )

        cpu_starts: list[float] = []
        cpu_percentages: list[float] = []

        # Parse the start time and percentage for each `Event`.
        for event in cpu_usage_events:
            cpu_starts.append(event.start.to_epoch_delta().to_nanoseconds())
            cpu_percentages.append(
                event.args.get("average_cpu_percentage") or 0
            )

        # The suspend times calculated from https://source.corp.google.com/h/turquoise-internal/turquoise/+/main:src/cobalt/bin/system-metrics/cpu_stats_fetcher_impl.cc
        # are offset by 1 event. Add that event in. It's approximately 1 s.
        cpu_durations: list[float] = [_ONE_S_IN_NS] + [
            curr - prev for prev, curr in itertools.pairwise(cpu_starts)
        ]

        if len(cpu_percentages) == 0:
            _LOGGER.info(
                "No cpu usage measurements are present. Perhaps the trace "
                "duration is too short to provide cpu usage information"
            )
            return []

        if len(cpu_durations) != len(cpu_percentages):
            _LOGGER.warning(
                "The number of CPU duration segments don't match the number "
                "of percentages. Data may be truncated."
            )

        results: list[metrics.TestCaseResult] = []
        # Calculate metrics without normalization
        if self.aggregates_only:
            results.extend(
                trace_utils.standard_metrics_set(
                    values=cpu_percentages,
                    label_prefix="Cpu",
                    unit=metrics.Unit.percent,
                    durations=cpu_durations,
                )
            )
        else:
            results.append(
                metrics.TestCaseResult(
                    "CpuLoad", metrics.Unit.percent, cpu_percentages
                )
            )

        # Calculate normalized metrics if Processing Rate events are present
        cpu_timelines = self._get_cpu_rates(model)
        if cpu_timelines:
            total_cpu_count = max(1, len(model.scheduling_records))
            norm_percentages = self._calculate_normalized_percentages(
                cpu_starts=cpu_starts,
                cpu_durations=cpu_durations,
                cpu_percentages=cpu_percentages,
                cpu_timelines=cpu_timelines,
                total_cpu_count=total_cpu_count,
            )
            if self.aggregates_only:
                results.extend(
                    trace_utils.standard_metrics_set(
                        values=norm_percentages,
                        label_prefix="CpuNormalized",
                        unit=metrics.Unit.percent,
                        durations=cpu_durations,
                    )
                )
            else:
                results.append(
                    metrics.TestCaseResult(
                        "CpuNormalizedLoad",
                        metrics.Unit.percent,
                        norm_percentages,
                    )
                )

        return results

    def process_freeform_metrics(
        self, model: trace_model.Model
    ) -> tuple[str, Breakdown]:
        """
        Given trace_model.Model, iterates through all the SchedulingRecords and calculates the
        duration for each Process's Threads, and saves them by CPU.

        Args:
            model: The input trace model.

        Returns:
            str: stable identifier to use in freeform metrics file name.
            Breakdown: Per-process, per-thread CPU usage breakdown.
        """
        (breakdown, _) = self.process_metrics_and_get_total_time(model)
        return self.FREEFORM_METRICS_FILENAME, breakdown

    def process_metrics_and_get_total_time(
        self, model: trace_model.Model
    ) -> tuple[Breakdown, float]:
        """
        Given trace_model.Model, iterates through all the SchedulingRecords and calculates the
        duration for each Process's Threads, and saves them by CPU.

        Args:
            model: The input trace model.

        Returns:
            Breakdown: Per-process, per-thread CPU usage breakdown.
            float: The total duration of the trace.
        """
        # Map tids to names.
        tid_to_process_name: dict[int, str] = {}
        tid_to_thread_name: dict[int, str] = {}
        for p in model.processes:
            for t in p.threads:
                tid_to_process_name[t.tid] = p.name
                tid_to_thread_name[t.tid] = t.name

        # Parse Processing rate events for DVFS CPU normalization.
        cpu_timelines = self._get_cpu_rates(model)

        # Calculate durations for each CPU for each tid.
        durations = DurationsBreakdown.calculate(
            model.scheduling_records,
            tid_to_thread_name,
            cpu_timelines,
        )

        # Calculate the percent of time the thread spent on this CPU,
        # compared to the total CPU duration.
        # If the percent spent is at or above our cutoff, add metric to
        # breakdown.
        full_breakdown: list[BreakdownMetric] = []
        for tid, cpu_stats_map in durations.tid_to_stats.items():
            if tid in tid_to_thread_name:
                for cpu, stats in cpu_stats_map.items():
                    duration = stats.duration
                    percent = (
                        duration / durations.cpu_to_total_duration[cpu] * 100
                        if durations.cpu_to_total_duration[cpu] > 0
                        else 0.0
                    )

                    normalized_duration = stats.normalized_duration
                    normalized_percent = (
                        normalized_duration
                        / durations.cpu_to_total_normalized_duration[cpu]
                        * 100
                        if durations.cpu_to_total_normalized_duration[cpu] > 0
                        else 0.0
                    )

                    if percent >= self._percent_cutoff:
                        metric: BreakdownMetric = {
                            "process_name": tid_to_process_name[tid],
                            "thread_name": tid_to_thread_name[tid],
                            "tid": tid,
                            "cpu": cpu,
                            "percent": percent,
                            "duration": duration,
                            "normalized_percent": normalized_percent,
                            "normalized_duration": normalized_duration,
                            "duration_per_rate": dict(
                                sorted(
                                    stats.rate_stats.duration_per_rate.items(),
                                    key=lambda item: int(
                                        item[0]
                                    ),  # Sort by rate (as int)
                                    reverse=True,
                                )
                            ),
                        }
                        full_breakdown.append(metric)

        if durations.cpu_to_skipped_duration:
            _LOGGER.warning(
                "Possibly missing ContextSwitch record(s) in trace for these CPUs and durations: "
                f"{durations.cpu_to_skipped_duration}"
            )

        # Sort metrics by CPU (desc) and percent (desc).
        full_breakdown.sort(
            key=lambda m: (m["cpu"], m["percent"]),
            reverse=True,
        )
        return (
            cast(Breakdown, full_breakdown),
            durations.max_timestamp - durations.min_timestamp,
        )

    def _get_cpu_rates(
        self, model: trace_model.Model
    ) -> dict[int, CpuProcessingRateTimeline]:
        """Extracts DVFS processing rate events from the trace model for all CPUs.

        Args:
            model: The input trace model containing 'kernel:power' events.

        Returns:
            A dictionary keyed by cpu_index, containing CpuProcessingRateTimeline objects.
        """
        power_events = list(
            e
            for e in trace_utils.filter_events(
                model.all_events(),
                type=trace_model.CounterEvent,
            )
            if e.name and e.name.startswith(_PROCESSING_RATE_EVENT_NAME)
        )

        rates_by_cpu: dict[int, list[ProcessingRateSample]] = {}
        for event in power_events:
            # For "Processing Rate" events, the CPU index is encoded in `event.id`.
            # When the CPU index is 0, the `event.id` field is omitted from the trace.
            cpu_idx = event.id if event.id is not None else 0
            rate = event.args.get("CPU", _DEFAULT_PROCESSING_RATE)

            rates_by_cpu.setdefault(cpu_idx, []).append(
                ProcessingRateSample(
                    timestamp_ms=DurationsBreakdown._timestamp_ms(event.start),
                    rate=rate,
                )
            )

        return {
            cpu_idx: CpuProcessingRateTimeline(rates)
            for cpu_idx, rates in rates_by_cpu.items()
        }

    def _calculate_normalized_percentages(
        self,
        *,
        cpu_starts: list[float],
        cpu_durations: list[float],
        cpu_percentages: list[float],
        cpu_timelines: dict[int, CpuProcessingRateTimeline],
        total_cpu_count: int,
    ) -> list[float]:
        """Computes the normalized CPU percentages for periodic aggregate windows.

        For each reporting window (e.g., 1-second buckets), this calculates the exact
        normalized CPU utilization by comparing the DVFS-weighted CPU time against
        the total wall-clock time of the window.

        Args:
            cpu_starts: Start timestamps (in ns) of the reporting windows.
            cpu_durations: Durations (in ns) of the reporting windows.
            cpu_percentages: The raw (unnormalized) CPU percentages for the windows.
            cpu_timelines: The processing rate timelines for each CPU.
            total_cpu_count: The total number of CPUs in the system.

        Returns:
            A list of normalized CPU percentages corresponding to each window.
        """
        norm_percentages = []
        cpu_count = max(1, total_cpu_count)
        missing_cpus = max(0, cpu_count - len(cpu_timelines))

        for start_ns, dur_ns, pct in zip(
            cpu_starts, cpu_durations, cpu_percentages
        ):
            start_ms = start_ns / _NS_PER_MS
            end_ms = (start_ns + dur_ns) / _NS_PER_MS
            total_dur = dur_ns / _NS_PER_MS

            total_norm_dur = 0.0

            if total_dur > 0 and cpu_timelines:
                for timeline in cpu_timelines.values():
                    virtual_slices = timeline.get_virtual_slices(
                        start_ms, end_ms
                    )
                    total_norm_dur += sum(
                        s.duration * (s.rate / _DEFAULT_PROCESSING_RATE)
                        for s in virtual_slices
                    )
                # Add full capacity duration for CPUs without rate events
                total_norm_dur += missing_cpus * total_dur

                avg_norm_dur = total_norm_dur / cpu_count
                avg_rate_ratio = avg_norm_dur / total_dur
            else:
                avg_rate_ratio = 1.0

            norm_percentages.append(pct * avg_rate_ratio)
        return norm_percentages


class DurationsBreakdown:
    def __init__(self) -> None:
        # Maps TID to a dict of CPUs to stats on that CPU.
        self.tid_to_stats: collections.defaultdict[
            int, collections.defaultdict[int, ThreadCpuStats]
        ] = collections.defaultdict(
            lambda: collections.defaultdict(ThreadCpuStats)
        )
        # Map of CPU to total duration used (ms).
        self.cpu_to_total_duration: dict[int, float] = {}
        # Map of CPU to total normalized duration used (ms).
        self.cpu_to_total_normalized_duration: dict[int, float] = {}
        self.cpu_to_skipped_duration: dict[int, float] = {}
        self.min_timestamp: float = sys.float_info.max
        self.max_timestamp: float = 0

    def _calculate_duration_per_cpu(
        self,
        cpu: int,
        records: list[trace_model.ContextSwitch],
        tid_to_thread_name: dict[int, str],
        rate_timeline: CpuProcessingRateTimeline | None,
    ) -> None:
        """
        Calculates the total duration for each thread, on a particular CPU.

        Uses a list of sorted ContextSwitch records to sum up the duration for each thread.
        It's possible that consecutive records do not have matching incoming_tid and outgoing_tid.
        """
        smallest_timestamp = self._timestamp_ms(records[0].start)
        if smallest_timestamp < self.min_timestamp:
            self.min_timestamp = smallest_timestamp
        largest_timestamp = self._timestamp_ms(records[-1].start)
        if largest_timestamp > self.max_timestamp:
            self.max_timestamp = largest_timestamp
        total_duration = largest_timestamp - smallest_timestamp
        skipped_duration = 0.0
        self.cpu_to_total_duration[cpu] = total_duration

        if rate_timeline:
            virtual_slices = rate_timeline.get_virtual_slices(
                smallest_timestamp, largest_timestamp
            )
            self.cpu_to_total_normalized_duration[cpu] = sum(
                s.duration * (s.rate / _DEFAULT_PROCESSING_RATE)
                for s in virtual_slices
            )
        else:
            self.cpu_to_total_normalized_duration[cpu] = (
                largest_timestamp - smallest_timestamp
            )

        for prev_record, curr_record in itertools.pairwise(records):
            # Check that the previous ContextSwitch's incoming_tid ("this thread is starting work
            # on this CPU") matches the current ContextSwitch's outgoing_tid ("this thread is being
            # switched away from"). If so, there is a duration to calculate. Otherwise, it means
            # maybe there is skipped data or something.
            if prev_record.tid != curr_record.outgoing_tid:
                start_ts = self._timestamp_ms(prev_record.start)
                stop_ts = self._timestamp_ms(curr_record.start)
                skipped_duration += stop_ts - start_ts
            # Purposely skip saving idle thread durations.
            elif prev_record.is_idle():
                continue
            else:
                start_ts = self._timestamp_ms(prev_record.start)
                stop_ts = self._timestamp_ms(curr_record.start)
                duration = stop_ts - start_ts
                assert duration >= 0

                if curr_record.outgoing_tid in tid_to_thread_name:
                    # Add stats to the total duration for that tid and CPU.
                    cpu_stats = self.tid_to_stats[curr_record.outgoing_tid][cpu]
                    cpu_stats.duration += duration

                    if rate_timeline:
                        virtual_slices = rate_timeline.get_virtual_slices(
                            start_ts, stop_ts
                        )
                        for s in virtual_slices:
                            norm_dur = s.duration * (
                                s.rate / _DEFAULT_PROCESSING_RATE
                            )
                            cpu_stats.normalized_duration += norm_dur
                            cpu_stats.rate_stats.add(s.rate, s.duration)
                    else:
                        cpu_stats.normalized_duration += duration
                        cpu_stats.rate_stats.add(
                            _DEFAULT_PROCESSING_RATE, duration
                        )

        if skipped_duration > 0:
            self.cpu_to_skipped_duration[cpu] = skipped_duration

    @staticmethod
    def _timestamp_ms(timestamp: trace_time.TimePoint) -> float:
        """
        Return timestamp in ms.
        """
        return timestamp.to_epoch_delta().to_milliseconds_f()

    @classmethod
    def calculate(
        cls,
        per_cpu_scheduling_records: Mapping[
            int, Sequence[trace_model.SchedulingRecord]
        ],
        tid_to_thread_name: dict[int, str],
        cpu_timelines: dict[int, CpuProcessingRateTimeline],
    ) -> Self:
        durations = cls()
        for cpu, records in per_cpu_scheduling_records.items():
            durations._calculate_duration_per_cpu(
                cpu,
                sorted(
                    trace_utils.filter_records(
                        records, trace_model.ContextSwitch
                    ),
                    key=lambda record: record.start,
                ),
                tid_to_thread_name,
                cpu_timelines.get(cpu),
            )
        return durations


def group_by_process_name(breakdown: Breakdown) -> Breakdown:
    """
    Given a breakdown, group the metrics by process_name only,
    ignoring thread name.
    """
    # Cast to internal type for better type safety inside this function.
    typed_breakdown = cast(list[BreakdownMetric], breakdown)
    if not typed_breakdown:
        return []

    # Group metrics by (cpu, process_name)
    grouped: collections.defaultdict[
        tuple[int, str], list[BreakdownMetric]
    ] = collections.defaultdict(list)
    for metric in typed_breakdown:
        grouped[(metric["cpu"], metric["process_name"])].append(metric)

    consolidated_breakdown: list[BreakdownMetric] = [
        _aggregate_metrics(metrics) for metrics in grouped.values()
    ]

    return cast(
        Breakdown,
        sorted(
            consolidated_breakdown,
            key=lambda m: (m["cpu"], m["percent"]),
            reverse=True,
        ),
    )


def _aggregate_metrics(metrics: list[BreakdownMetric]) -> BreakdownMetric:
    """
    Combines a list of thread-level metrics into a single process-level metric.

    The combination rules are as follows:
    - Numerical fields (`percent`, `duration`, `normalized_percent`, `normalized_duration`)
      are combined by adding them together.
    - The `duration_per_rate` dictionaries are merged by summing their corresponding rate values.
    - The returned metric retains the `process_name` and `cpu` keys from the first metric.
    - Thread-specific keys (`thread_name`, `tid`) are intentionally omitted as this operates
      at the process level.

    Args:
        metrics: A non-empty list of BreakdownMetrics for the same process and CPU.

    Returns:
        A single aggregated BreakdownMetric.
    """
    first = metrics[0]
    merged: BreakdownMetric = {
        "process_name": first["process_name"],
        "cpu": first["cpu"],
        "percent": sum(m["percent"] for m in metrics),
        "duration": sum(m["duration"] for m in metrics),
    }

    if "normalized_duration" in first:
        merged["normalized_percent"] = sum(
            m.get("normalized_percent", 0.0) for m in metrics
        )
        merged["normalized_duration"] = sum(
            m.get("normalized_duration", 0.0) for m in metrics
        )

    if "duration_per_rate" in first:
        rates: dict[str, float] = {}
        for m in metrics:
            for k, v in m.get("duration_per_rate", {}).items():
                rates[k] = rates.get(k, 0.0) + v
        merged["duration_per_rate"] = dict(
            sorted(rates.items(), key=lambda x: int(x[0]), reverse=True)
        )

    return merged
