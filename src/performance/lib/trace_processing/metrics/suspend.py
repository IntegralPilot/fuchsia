# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Suspend trace metrics."""

import itertools
import logging
from typing import Dict, MutableSequence, Tuple

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time, trace_utils

_LOGGER: logging.Logger = logging.getLogger(__name__)

_EVENT_CATEGORY = "power"

# LINT.IfChange
_SYSFS_EVENT_NAME = "starnix-sysfs:suspend"
# LINT.ThenChange(//src/starnix/kernel/power/state.rs)

# LINT.IfChange
_STARNIX_RUNNER_SUSPEND_EVENT_NAME = (
    "starnix-runner:drop-application-activity-lease"
)
_STARNIX_RUNNER_RESUME_EVENT_NAME = (
    "starnix-runner:acquire-application-activity-lease"
)
# LINT.ThenChange(//src/starnix/lib/kernel_manager/src/kernels.rs)

# LINT.IfChange
_SAG_EVENT_NAME = "system-activity-governor:suspend"
# LINT.ThenChange(//src/power/system-activity-governor/src/cpu_manager.rs)

# LINT.IfChange
_SUSPEND_EVENT_NAME = "generic-suspend:suspend"
# LINT.ThenChange(//src/devices/suspend/drivers/generic-suspend/generic-suspend.cc)

EVENT_PATTERNS = {
    _SYSFS_EVENT_NAME,
    _STARNIX_RUNNER_SUSPEND_EVENT_NAME,
    _STARNIX_RUNNER_RESUME_EVENT_NAME,
    _SAG_EVENT_NAME,
    _SUSPEND_EVENT_NAME,
}


class SuspensionInfo:
    # This is the outer duration of a suspension instance.  _SYSFS_EVENT_NAME
    sysfs_event: trace_model.DurationEvent
    # starnix lease dropped _STARNIX_RUNNER_SUSPEND_EVENT_NAME
    starnix_runner_suspend: trace_model.InstantEvent | None
    # system activity governor suspend _SAG_EVENT_NAME
    sag_suspend: trace_model.DurationEvent | None

    suspended_event: trace_model.DurationEvent | None
    # starnix runner resume
    starnix_runner_resume: trace_model.DurationEvent | None

    # cpu idle start for this instance
    cpu_idle_start: trace_time.TimePoint | None
    cpu_idle_end: trace_time.TimePoint | None

    def __init__(self, sysfs_event: trace_model.DurationEvent):
        self.sysfs_event = sysfs_event
        self.starnix_runner_suspend = None
        self.sag_suspend = None
        self.starnix_runner_resume = None
        self.suspended_event = None
        self.cpu_idle_start = None
        self.cpu_idle_end = None

    def duration(self) -> trace_time.TimeDelta | None:
        return self.sysfs_event.duration

    def fill_steps(self, model: trace_model.Model) -> None:
        model_slice = model.slice(
            self.sysfs_event.start, self.sysfs_event.end_time()
        )

        filters = (
            trace_utils.EventFilter(
                category=_EVENT_CATEGORY,
                name=_SAG_EVENT_NAME,
                type=trace_model.DurationEvent,
            ),
            trace_utils.EventFilter(
                category=_EVENT_CATEGORY,
                name=_SUSPEND_EVENT_NAME,
                type=trace_model.DurationEvent,
            ),
            trace_utils.EventFilter(
                category=_EVENT_CATEGORY,
                name=_STARNIX_RUNNER_SUSPEND_EVENT_NAME,
                type=trace_model.InstantEvent,
            ),
            trace_utils.EventFilter(
                category=_EVENT_CATEGORY,
                name=_STARNIX_RUNNER_RESUME_EVENT_NAME,
                type=trace_model.DurationEvent,
            ),
        )

        (
            sag_events,
            aml_driver_events,
            starnix_suspend_events,
            starnix_resume_events,
        ) = trace_utils.filter_events_parallel(
            model_slice.all_events(), filters
        )

        self.starnix_runner_suspend = get_starnix_kernel_suspend_event(
            list(starnix_suspend_events)
        )
        self.sag_suspend = get_earliest_event(list(sag_events))
        self.suspended_event = get_earliest_event(list(aml_driver_events))
        self.starnix_runner_resume = get_earliest_event(
            list(starnix_resume_events)
        )

        if self.suspended_event and self.suspended_event.end_time():
            # Get CPU idle time within the AML driver suspend interval
            start = self.suspended_event.start
            end = self.suspended_event.end_time()
            if start and end:
                cpu_idle_time = get_cpu_idle_time(model, start, end)
                if cpu_idle_time and len(cpu_idle_time) == 2:
                    self.cpu_idle_start = cpu_idle_time[0]
                    self.cpu_idle_end = cpu_idle_time[1]

    def sysfs_to_starnix_kernel(self) -> trace_time.TimeDelta | None:
        return (
            self.starnix_runner_suspend.start - self.sysfs_event.start
            if self.starnix_runner_suspend
            else None
        )

    def sysfs_to_sag(self) -> trace_time.TimeDelta | None:
        return (
            self.sag_suspend.start - self.sysfs_event.start
            if self.sag_suspend
            else None
        )

    def sysfs_to_suspend_driver(self) -> trace_time.TimeDelta | None:
        return (
            self.suspended_event.start - self.sysfs_event.start
            if self.suspended_event
            else None
        )

    def sysfs_to_cpu_idle(self) -> trace_time.TimeDelta | None:
        return (
            self.cpu_idle_start - self.sysfs_event.start
            if self.cpu_idle_start
            else None
        )

    def cpu_idle_to_suspend_driver(self) -> trace_time.TimeDelta | None:
        if self.suspended_event is None:
            return None
        suspend_end = self.suspended_event.end_time()

        if suspend_end is None:
            return None

        if self.cpu_idle_end is None:
            return None

        return suspend_end - self.cpu_idle_end

    def cpu_idle_to_sag(self) -> trace_time.TimeDelta | None:
        if self.sag_suspend is None:
            return None
        suspend_end = self.sag_suspend.end_time()

        if suspend_end is None:
            return None

        if self.cpu_idle_end is None:
            return None

        return suspend_end - self.cpu_idle_end

    def cpu_idle_to_starnix_kernel(self) -> trace_time.TimeDelta | None:
        if self.starnix_runner_resume is None:
            return None
        suspend_end = self.starnix_runner_resume.end_time()

        if suspend_end is None:
            return None

        if self.cpu_idle_end is None:
            return None

        return suspend_end - self.cpu_idle_end

    def cpu_idle_to_sysfs(self) -> trace_time.TimeDelta | None:
        suspend_end = self.sysfs_event.end_time()

        if suspend_end is None:
            return None

        if self.cpu_idle_end is None:
            return None
        return suspend_end - self.cpu_idle_end

    def is_complete(self) -> bool:
        return self.suspended_event is not None

    def is_resumed(self) -> bool:
        return self.starnix_runner_resume is not None


class SuspendMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes suspend/resume metrics."""

    @property
    def event_patterns(self) -> set[str]:
        return EVENT_PATTERNS

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        """Calculate suspend/resume metrics.

        Args:
            model: In-memory representation of a system trace.

        Returns:
            Set of metrics results for this test case.
        """
        filters = (
            trace_utils.EventFilter(
                category=_EVENT_CATEGORY,
                name=_SYSFS_EVENT_NAME,
                type=trace_model.DurationEvent,
            ),
            trace_utils.EventFilter(
                category=_EVENT_CATEGORY,
                name=_SAG_EVENT_NAME,
                type=trace_model.DurationEvent,
            ),
        )

        (
            sysfs_events,
            sag_events,
        ) = trace_utils.filter_events_parallel(model.all_events(), filters)

        # Suspend-resume multi-step measurements. This section calculates the
        # time taken for each step in the suspend/resume process, averaged
        # across multiple occurrences.
        #
        # The steps measured are:
        # 1. sysfs suspend start -> Starnix kernel suspend start
        # 2. sysfs suspend start -> SAG suspend start
        # 3. sysfs suspend start -> AML suspend driver start
        # 4. sysfs suspend start -> CPU idle start
        # 5. CPU idle end -> AML suspend driver end
        # 6. CPU idle end -> SAG suspend end
        # 7. CPU idle end -> Starnix kernel resume end
        # 8. CPU idle end -> sysfs suspend end
        sysfs_list = [SuspensionInfo(e) for e in sysfs_events]
        for sysfs_event in sysfs_list:
            if sysfs_event.duration():
                sysfs_event.fill_steps(model)
        suspend_time = trace_time.TimeDelta(0)
        for sag_event in sag_events:
            if sag_event.duration is not None:
                suspend_time += sag_event.duration

        # Use the start of the first sysfs_event start time as the beginning of the test.
        trace_start_time = min(
            map(lambda e: e.sysfs_event.start, sysfs_list),
        )

        trace_end_time = max(
            [
                e
                for e in map(lambda e: e.sysfs_event.end_time(), sysfs_list)
                if e is not None
            ]
        )
        total_time = trace_end_time - trace_start_time
        running_time = total_time - suspend_time

        result = [
            metrics.TestCaseResult(
                label="UnsuspendedTime",
                unit=metrics.Unit.nanoseconds,
                values=[running_time.to_nanoseconds()],
            ),
            metrics.TestCaseResult(
                label="SuspendTime",
                unit=metrics.Unit.nanoseconds,
                values=[suspend_time.to_nanoseconds()],
            ),
            metrics.TestCaseResult(
                label="SuspendPercentage",
                unit=metrics.Unit.percent,
                values=[(suspend_time / total_time) * 100],
            ),
        ]

        # only count complete spans
        metrics_list: Dict[str, Tuple[float, float]] = dict()
        for s in [s for s in sysfs_list if s.is_complete()]:
            add_to_metrics(
                metrics_list,
                "Suspend.sysfs_to_starnix_kernel",
                s.sysfs_to_starnix_kernel(),
            )
            add_to_metrics(
                metrics_list, "Suspend.sysfs_to_sag", s.sysfs_to_sag()
            )
            add_to_metrics(
                metrics_list,
                "Suspend.sysfs_to_suspend_driver",
                s.sysfs_to_suspend_driver(),
            )
            add_to_metrics(
                metrics_list, "Suspend.sysfs_to_cpu_idle", s.sysfs_to_cpu_idle()
            )

        for s in [s for s in sysfs_list if s.is_resumed()]:
            add_to_metrics(
                metrics_list,
                "Resume.cpu_idle_to_suspend_driver",
                s.cpu_idle_to_suspend_driver(),
            )
            add_to_metrics(
                metrics_list, "Resume.cpu_idle_to_sag", s.cpu_idle_to_sag()
            )
            add_to_metrics(
                metrics_list,
                "Resume.cpu_idle_to_starnix_kernel",
                s.cpu_idle_to_starnix_kernel(),
            )
            add_to_metrics(
                metrics_list, "Resume.cpu_idle_to_sysfs", s.cpu_idle_to_sysfs()
            )

        for label, (val, n) in metrics_list.items():
            result.append(
                metrics.TestCaseResult(
                    label=label,
                    unit=metrics.Unit.nanoseconds,
                    values=[val / n],
                )
            )
        return result


def get_earliest_event(
    events: list[trace_model.DurationEvent],
) -> trace_model.DurationEvent | None:
    """Retrieves the first event from the list."""
    ret = None
    for event in events:
        if not ret or event.start < ret.start:
            ret = event
    return ret


def get_starnix_kernel_suspend_event(
    starnix_kernel_suspend_event: list[trace_model.InstantEvent],
) -> trace_model.InstantEvent | None:
    """Retrieves the starnix kernel suspend event from the trace model.

    It is designed for use cases where only a single suspend/resume
    operation is expected within the trace model.

    Args:
        starnix_kernel_suspend_event: list of starnix kernel suspend events.

    Returns:
        The starnix kernel suspend event if found, otherwise None.
        Raises a ValueError if more than one event is found.
    """
    if len(starnix_kernel_suspend_event) == 1:
        return starnix_kernel_suspend_event[0]
    elif len(starnix_kernel_suspend_event) > 1:
        raise ValueError("Got more than one starnix kernel suspend events")
    return None


def get_cpu_idle_time(
    model: trace_model.Model,
    start: trace_time.TimePoint,
    end: trace_time.TimePoint,
) -> Tuple[trace_time.TimePoint, trace_time.TimePoint] | None:
    """Calculates the longest common CPU idle time across all CPUs in the model
    within a specified time window.

    This function identifies idle time intervals for each CPU overlap with or
    within the given [start, end] time range and then determines the longest
    continuous period where all CPUs were idle.

    Args:
        model: The trace model containing CPU scheduling records.
        start: The start time of the analysis window.
        end: The end time of the analysis window.

    Returns:
        A tuple of TimePoints representing the start and end of the longest
        common CPU idle interval within the [start, end] window.
        Returns None if no common idle time is found.
    """
    idle_times_list = []
    # Iterate over scheduling records for each CPU
    for cpu, records in model.scheduling_records.items():
        context_switch_records = [
            record
            for record in records
            if isinstance(record, trace_model.ContextSwitch)
        ]
        idle_times_list.append(
            get_per_cpu_idle_times(
                # All the records are sorted by start time
                sorted(
                    context_switch_records,
                    key=lambda record: record.start,
                ),
                start,
                end,
            )
        )
    common_idle_times = find_common_idle_times(idle_times_list, start, end)
    if not common_idle_times:
        return None
    return max(
        common_idle_times, key=lambda interval: interval[1] - interval[0]
    )


def get_per_cpu_idle_times(
    records: list[trace_model.SchedulingRecord],
    start: trace_time.TimePoint,
    end: trace_time.TimePoint,
) -> list[Tuple[trace_time.TimePoint, trace_time.TimePoint]]:
    """Calculates the idle time intervals for a single CPU within a specified
    time window, based on scheduling records.

    This function iterates through the scheduling records and identifies periods
    where the CPU is in an idle state, considering only intervals that overlap with
    or fall within the [start, end] time range. It assumes the records are sorted by
    start time.

    Args:
        records: A list of SchedulingRecord objects for a specific CPU, sorted
                 by start time.
        start: The start time of the analysis window.
        end: The end time of the analysis window.

    Returns:
        A list of tuples representing the idle time intervals within the
        [start, end] window. Each tuple contains the start and end TimePoints
        of an idle interval.
    """
    per_cpu_idle_times = []
    for prev_record, curr_record in itertools.pairwise(records):
        # Find intervals that overlap with or fall within the [start, end] window.
        # A CPU might have idle threads scheduled before AML driver suspension
        if (
            prev_record.is_idle()
            and start < curr_record.start
            and prev_record.start < end
        ):
            per_cpu_idle_times.append((prev_record.start, curr_record.start))
    return per_cpu_idle_times


def find_common_idle_times(
    idle_times_list: list[
        list[Tuple[trace_time.TimePoint, trace_time.TimePoint]]
    ],
    boundry_start: trace_time.TimePoint,
    boundry_end: trace_time.TimePoint,
) -> list[Tuple[trace_time.TimePoint, trace_time.TimePoint]]:
    """Finds the common idle time intervals across multiple lists of
    idle time intervals, within a specified time window.

    Args:
      idle_times_list: A list of lists, where each inner list contains tuples
                       representing idle time intervals (start_timestamp,
                       end_timestamp).
      boundry_start: The start time of the analysis window.
      boundry_end: The end time of the analysis window.

    Returns:
      A list of tuples representing the common idle time intervals.
    """

    if not idle_times_list:
        return []

    # Start with the first list as the initial common intervals
    common_intervals = idle_times_list[0]

    # Add the boundary window as a "virtual" list to constrain the results
    # within the [boundry_start, boundry_end] time range. This is needed
    # because idle_times_list might contain time intervals that overlap
    # with the analysis window, not just intervals that fall entirely
    # within it.
    idle_times_list.append([(boundry_start, boundry_end)])

    for idle_times in idle_times_list[1:]:
        new_common_intervals = []
        # Since each list only contains a few idle intervals from a single
        # driver suspension period, performance isn't a concern.
        for interval1 in common_intervals:
            for interval2 in idle_times:
                start = max(interval1[0], interval2[0])
                end = min(interval1[1], interval2[1])
                if start < end:
                    new_common_intervals.append((start, end))
        common_intervals = new_common_intervals

    return common_intervals


def add_to_metrics(
    metrics: Dict[str, Tuple[float, float]],
    label: str,
    value: trace_time.TimeDelta | None,
) -> None:
    if not value:
        return
    (value_sum, n) = metrics.get(label, (0, 0))
    value_sum += value.to_nanoseconds()
    n += 1
    metrics[label] = (value_sum, n)
