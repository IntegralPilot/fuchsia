#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Scenic trace metrics."""

import logging
import statistics
from typing import MutableSequence, Tuple

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time, trace_utils

_LOGGER: logging.Logger = logging.getLogger("ScenicMetricsProcessor")
_EVENT_CATEGORY: str = "gfx"
_SCENIC_START_EVENT_NAME: str = "ApplyScheduledSessionUpdates"
_SCENIC_RENDER_EVENT_NAME: str = "RenderFrame"
_DISPLAY_VSYNC_READY_EVENT_NAME: str = "Flatland::DisplayCompositor::OnVsync"
_PREP_AND_RENDER_FLOW_NAME: str = "scenic_frame"
_RENDER_FLOW_NAME: str = "render_frame_to_vsync"
_FRAME_NUMBER_ARG_NAME: str = "frame_number"


class _ScenicTracingEvent:
    start_event: trace_model.Event
    render_event: trace_model.DurationEvent
    vsync_event: trace_model.Event

    def __init__(
        self,
        start_event: trace_model.Event,
        render_event: trace_model.DurationEvent,
        vsync_event: trace_model.Event,
    ):
        self.start_event = start_event
        self.render_event = render_event
        self.vsync_event = vsync_event


class ScenicMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes CPU and GPU time spent rendering frames in Scenic.

    Calculates total time-to-render-frame by measuring from the moment that Scenic reports it has
    begun computing frame contents to the moment that is ready for vsync. Also tracks time spent on
    CPU-bound operations for each frame. Flow events in the trace enable this class to reliably
    correlate the correct events to calculate this duration.

    By default, this module reports aggregate latency measurements -- such as min, max, average, and
    percentiles -- calculated across all frames rendered during the test. It can be
    configured to instead report a time series of measurements, one for each event.
    """

    def __init__(
        self, aggregates_only: bool = True, include_render_total: bool = True
    ):
        """Constructor.

        Args:
            aggregates_only: When True, generates RenderCpu[Min|Max|Average|P*] and
                RenderTotal[Min|Max|Average|P*].
                Otherwise generates RenderCpu and RenderTotal with the raw values.
            include_render_total: When True, collects RenderTotal metrics.
        """
        self.aggregates_only: bool = aggregates_only
        self.include_render_total: bool = include_render_total

    @property
    def event_patterns(self) -> set[str]:
        """Patterns describing the trace events needed to generate these metrics."""
        return {
            _SCENIC_START_EVENT_NAME,
            _SCENIC_RENDER_EVENT_NAME,
            _DISPLAY_VSYNC_READY_EVENT_NAME,
            _PREP_AND_RENDER_FLOW_NAME,
            _RENDER_FLOW_NAME,
        }

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        # This method looks for a possible race between trace event start in Scenic and magma.
        # We can safely skip these events. See https://fxbug.dev/322849857 for more details.
        model = trace_utils.adjust_to_common_process_start(
            model,
            _SCENIC_START_EVENT_NAME,
            category=_EVENT_CATEGORY,
            type=trace_model.DurationEvent,
        )

        scenic_start_events = self._get_scenic_start_events(model)
        tracing_events: list[_ScenicTracingEvent] = []
        for e in scenic_start_events:
            render_event = trace_utils.get_nearest_following_flow_event(
                e, _EVENT_CATEGORY, _SCENIC_RENDER_EVENT_NAME
            )
            if render_event is None:
                continue
            assert isinstance(render_event, trace_model.DurationEvent)
            if not render_event.duration:
                continue
            vsync_ready_event = trace_utils.get_nearest_following_flow_event(
                render_event, _EVENT_CATEGORY, _DISPLAY_VSYNC_READY_EVENT_NAME
            )
            if vsync_ready_event is None:
                continue
            tracing_events.append(
                _ScenicTracingEvent(e, render_event, vsync_ready_event)
            )

        if len(tracing_events) < 1:
            _LOGGER.warning(
                "No render or vsync events are present. Perhaps the trace "
                "duration is too short to provide scenic render information"
            )
            return []

        frame_windows: list[
            tuple[trace_time.TimePoint, trace_time.TimePoint]
        ] = []
        for tracing_event in tracing_events:
            if tracing_event.render_event.duration is None:
                continue
            # We know `render_event` and `start_event` run on the same thread.
            start_time = tracing_event.start_event.start
            end_time = (
                tracing_event.render_event.start
                + tracing_event.render_event.duration
            )
            frame_windows.append((start_time, end_time))

        # We know `render_event` and `start_event` run on the same thread.
        tid = tracing_events[0].render_event.tid
        running_durations = _get_thread_running_durations(
            model, tid, frame_windows
        )
        cpu_render_times = [d.to_milliseconds_f() for d in running_durations]

        cpu_render_mean: float = statistics.mean(cpu_render_times)
        _LOGGER.info(f"Average CPU render time: {cpu_render_mean} ms")

        metrics_list: list[Tuple[str, list[float]]] = [
            ("RenderCpu", cpu_render_times),
        ]

        if self.include_render_total:
            total_render_times: list[float] = []
            for tracing_event in tracing_events:
                total_render_times.append(
                    (
                        tracing_event.vsync_event.start
                        - tracing_event.start_event.start
                    ).to_milliseconds_f()
                )

            total_render_mean: float = statistics.mean(total_render_times)
            _LOGGER.info(f"Average Total render time: {total_render_mean} ms")
            metrics_list.append(("RenderTotal", total_render_times))

        test_case_results: list[metrics.TestCaseResult] = []
        for name, values in metrics_list:
            if self.aggregates_only:
                test_case_results.extend(
                    trace_utils.standard_metrics_set(
                        values=values,
                        label_prefix=name,
                        unit=metrics.Unit.milliseconds,
                    )
                )
            else:
                test_case_results.append(
                    metrics.TestCaseResult(
                        name, metrics.Unit.milliseconds, values
                    )
                )

        return test_case_results

    def _get_scenic_start_events(
        self, model: trace_model.Model
    ) -> list[trace_model.DurationEvent]:
        # Filter out squashed frames by checking for the latest `frame_number` instances.
        # If Scenic's frame scheduler applies updates and then decides that no frame needs to be
        # displayed, the frame number is NOT incremented. So, by going through the
        # _SCENIC_START_EVENT_NAME in chronological order and stomping any earlier one with the same
        # frame number, we keep only those that have a corresponding _SCENIC_RENDER_EVENT_NAME.
        rendered_scenic_start_events: dict[int, trace_model.DurationEvent] = {}

        for event in trace_utils.filter_events(
            model.all_events(),
            category=_EVENT_CATEGORY,
            name=_SCENIC_START_EVENT_NAME,
            type=trace_model.DurationEvent,
        ):
            frame_number = event.args.get(_FRAME_NUMBER_ARG_NAME)
            if frame_number is None:
                raise ValueError(
                    f"Trace event '{_SCENIC_START_EVENT_NAME}' at {event.start} is missing the load-bearing '{_FRAME_NUMBER_ARG_NAME}' argument."
                )
            if (
                frame_number not in rendered_scenic_start_events
                or event.start
                > rendered_scenic_start_events[frame_number].start
            ):
                rendered_scenic_start_events[frame_number] = event

        scenic_start_events = list(rendered_scenic_start_events.values())
        scenic_start_events.sort(key=lambda e: e.start)
        return scenic_start_events


def _get_thread_running_durations(
    model: trace_model.Model,
    tid: int,
    windows: list[tuple[trace_time.TimePoint, trace_time.TimePoint]],
) -> list[trace_time.TimeDelta]:
    """Calculates the active running time of a thread for a list of time windows
    using an optimized two-pointer sweep over the thread's running segments and
    the supplied time windows.

    Args:
      model: Trace model.
      tid: Thread id.
      windows: Sorted list of time windows, each containing a start and end time.

    Returns:
      List of time deltas representing the active running time of the thread
      for each time window.

    Raises:
      ValueError: If scheduling records are missing from the trace model.
    """
    if not windows:
        return []

    if not model.scheduling_records:
        raise ValueError("Scheduling records are missing from the trace model.")

    # Compile the thread's running segments across all CPU cores
    running_segments: list[
        tuple[trace_time.TimePoint, trace_time.TimePoint]
    ] = []
    last_window_end = max(w[1] for w in windows)

    for cpu, records in model.scheduling_records.items():
        # Filter for target thread's context switches
        thread_switches = [
            r
            for r in records
            if isinstance(r, trace_model.ContextSwitch)
            and (r.tid == tid or r.outgoing_tid == tid)
        ]
        thread_switches.sort(key=lambda r: r.start)

        # Construct segments from the sorted thread switches
        i = 0
        while i < len(thread_switches) - 1:
            if thread_switches[i].tid == tid:
                # The immediately following switch must be the thread descheduled
                if thread_switches[i + 1].outgoing_tid == tid:
                    running_segments.append(
                        (
                            thread_switches[i].start,
                            thread_switches[i + 1].start,
                        )
                    )
                    i += 2
                else:
                    # Mismatched switch, log warning and skip
                    _LOGGER.warning(
                        f"Mismatched context switch detected on CPU {cpu}: "
                        f"Consecutive incoming switches for thread {tid} without an "
                        f"intermediate deschedule event at timestamp {thread_switches[i].start}."
                    )
                    i += 1
            else:
                # Outgoing switch without preceding incoming switch.
                # If this is the very first switch on the CPU, it means the thread was already
                # running when trace recording began.
                if i == 0:
                    # We can assume a start time of 0 for this segment, since we are comparing
                    # intersections with `windows` and, in this case, would get the real start
                    # time from `windows` anyway.
                    running_segments.append(
                        (trace_time.TimePoint(0), thread_switches[0].start)
                    )
                else:
                    # Mismatched switch, log warning and skip
                    _LOGGER.warning(
                        f"Mismatched context switch detected on CPU {cpu}: "
                        f"Outgoing switch for thread {tid} without a matching preceding "
                        f"incoming switch at timestamp {thread_switches[i].start}."
                    )
                i += 1

        # Handle the last context switch if it is Scenic starting
        if thread_switches:
            last_switch = thread_switches[-1]
            if last_switch.tid == tid and last_switch.start < last_window_end:
                running_segments.append((last_switch.start, last_window_end))

    # Sort running segments chronologically.
    running_segments.sort(key=lambda s: s[0])

    # Merge overlapping segments to prevent double-counting from trace anomalies (e.g. dropped events)
    merged_segments = [running_segments[0]]
    for start, end in running_segments[1:]:
        prev_start, prev_end = merged_segments[-1]
        if start <= prev_end:
            merged_segments[-1] = (prev_start, max(prev_end, end))
        else:
            merged_segments.append((start, end))
    running_segments = merged_segments
    results = [trace_time.TimeDelta.zero() for _ in range(len(windows))]
    first_active_seg_idx = 0
    for idx, (w_start, w_end) in enumerate(windows):
        # Advance first_active_seg_idx past segments that end before the start of this window.
        while (
            first_active_seg_idx < len(running_segments)
            and running_segments[first_active_seg_idx][1] <= w_start
        ):
            first_active_seg_idx += 1

        # Scan forward to find all segments overlapping with the current time window.
        curr_seg_idx = first_active_seg_idx
        while curr_seg_idx < len(running_segments):
            s_start, s_end = running_segments[curr_seg_idx]

            # Since segments are sorted chronologically, if this segment starts after the
            # window ends, all subsequent segments will also start after the window ends.
            if s_start >= w_end:
                break

            overlap_start = max(s_start, w_start)
            overlap_end = min(s_end, w_end)
            results[idx] += overlap_end - overlap_start
            curr_seg_idx += 1

    return results
