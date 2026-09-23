#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""FPS trace metrics."""

import logging
import statistics
from typing import MutableSequence

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time, trace_utils

_LOGGER: logging.Logger = logging.getLogger("FPSMetricsProcessor")
_EVENT_CATEGORY: str = "gfx"
_SCENIC_RENDER_EVENT_NAME: str = "RenderFrame"
_DISPLAY_VSYNC_EVENT_NAME: str = "Flatland::DisplayCompositor::OnVsync"
_RENDER_FLOW_NAME: str = "render_frame_to_vsync"


class FpsMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes FPS (Frames-per-Second) metrics.

    Calculates Scenic's frames-per-second by measuring the window between consecutive vsyncs that
    are triggered by Scenic's frame-rendering code. Flow events in the trace enable this class to
    reliably correlate the correct events to calculate this duration.

    By default, this module reports aggregate latency measurements -- such as min, max, average, and
    percentiles -- calculated across all frames rendered during the test. It can be
    configured to instead report a time series of measurements, one for each event.
    """

    def __init__(self, aggregates_only: bool = True):
        """Constructor.

        Args:
            aggregates_only: When True, generates FpsMin, FpsMax, FpsAverage and FpsP* (percentiles).
                Otherwise generates Fps metric with all Fps values.
        """
        self.aggregates_only: bool = aggregates_only

    @property
    def event_patterns(self) -> set[str]:
        """Patterns describing the trace events needed to generate these metrics."""
        return {
            _SCENIC_RENDER_EVENT_NAME,
            _DISPLAY_VSYNC_EVENT_NAME,
            _RENDER_FLOW_NAME,
        }

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        # This method looks for a possible race between trace event start in Scenic and magma.
        # We can safely skip these events. See https://fxbug.dev/322849857 for more details.
        model = trace_utils.adjust_to_common_process_start(
            model,
            _SCENIC_RENDER_EVENT_NAME,
            category=_EVENT_CATEGORY,
            type=trace_model.DurationEvent,
        )

        cpu_render_start_events = trace_utils.filter_events(
            model.all_events(),
            category=_EVENT_CATEGORY,
            name=_SCENIC_RENDER_EVENT_NAME,
            type=trace_model.DurationEvent,
        )

        # Since `cpu_render_start_events` is a Generator, it cannot be iterated more than once.
        # Therefore, we can't just log `len(list(cpu_render_start_events))` -- turning it into a
        # list like that consumes it and throws it away. We could store it as a list, but it's not
        # worth holding it in memory just so we can log how many there are.
        cpu_render_start_event_count = 0
        vsync_events: list[trace_model.Event] = []
        for start in cpu_render_start_events:
            cpu_render_start_event_count += 1
            next_event = trace_utils.get_nearest_following_flow_event(
                start, _EVENT_CATEGORY, _DISPLAY_VSYNC_EVENT_NAME
            )
            if next_event is not None:
                vsync_events.append(next_event)

        _LOGGER.debug(f"{cpu_render_start_event_count} cpu_render_start_events")
        if len(vsync_events) < 2:
            _LOGGER.warning(
                "Fewer than two vsync events are present. Perhaps the trace "
                "duration is too short to provide fps information"
            )
            return []

        fps_values: list[float] = []
        for i in range(len(vsync_events) - 1):
            # Two renders may be squashed into one.
            if vsync_events[i + 1].start == vsync_events[i].start:
                continue
            fps_values.append(
                trace_time.TimeDelta.from_seconds(1)
                / (vsync_events[i + 1].start - vsync_events[i].start)
            )

        if len(fps_values) == 0:
            _LOGGER.warning("Not enough valid vsyncs")
            return []

        fps_mean: float = statistics.mean(fps_values)
        _LOGGER.info(f"Average FPS: {fps_mean}")

        if self.aggregates_only:
            return trace_utils.standard_metrics_set(
                values=fps_values,
                label_prefix="Fps",
                unit=metrics.Unit.framesPerSecond,
            )
        return [
            metrics.TestCaseResult(
                "Fps", metrics.Unit.framesPerSecond, fps_values
            )
        ]
