#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Per App Present trace metrics."""

import logging
import statistics
from typing import MutableSequence

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time, trace_utils

_LOGGER: logging.Logger = logging.getLogger("AppRenderLatencyMetricsProcessor")
_EVENT_CATEGORY: str = "gfx"
_PRESENT_EVENT_NAME: str = "Flatland::PerAppPresent[{}]"
_DISPLAY_VSYNC_EVENT_NAME: str = "Flatland::DisplayCompositor::OnVsync"


class AppRenderLatencyMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes present-latency metrics for a given app.

    App render latency is measured from the moment that the chosen app begins presenting through
    the Flatland API until the first vsync that corresponds with app content being displayed on
    screen. Flow events in the trace enable this class to reliably correlate the correct events to
    calculate this duration.

    By default, this module reports aggregate latency measurements -- such as min, max, average, and
    percentiles -- calculated across all presentations of the chosen app during the test. It can be
    configured to instead report a time series of latency measurements, one for each event.
    """

    def __init__(self, debug_name: str, aggregates_only: bool = True):
        """Constructor.

        Args:
            debug_name: the flatland client debug name.
            aggregates_only: When True, generates AppRenderVsyncLatencyMin,
                AppRenderVsyncLatencyMax, AppRenderVsyncLatencyAverage and
                AppRenderVsyncLatencyP* (percentiles).
                Otherwise generates AppRenderVsyncLatency metric with all
                AppRenderVsyncLatency values.
        """
        self._debug_name: str = debug_name
        self._aggregates_only: bool = aggregates_only

    @property
    def event_patterns(self) -> set[str]:
        return {
            _PRESENT_EVENT_NAME.format(self._debug_name),
            _DISPLAY_VSYNC_EVENT_NAME,
        }

    @property
    def category_names(self) -> set[str]:
        """This processor follows a flow with many parts across `gfx`."""
        return {_EVENT_CATEGORY}

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        # This method looks for a possible race between trace event start in Scenic and magma.
        # We can safely skip these events. See https://fxbug.dev/322849857 for more details.
        model = trace_utils.adjust_to_common_process_start(
            model,
            _PRESENT_EVENT_NAME.format(self._debug_name),
            category=_EVENT_CATEGORY,
            type=trace_model.DurationEvent,
        )

        present_flow_events = trace_utils.filter_events(
            model.all_events(),
            category=_EVENT_CATEGORY,
            name=_PRESENT_EVENT_NAME.format(self._debug_name),
            type=trace_model.Event,
        )

        present_latencies: list[float] = []
        vsync_events: list[trace_model.Event] = []

        for present_flow_event in present_flow_events:
            if not isinstance(present_flow_event, trace_model.FlowEvent):
                continue
            if present_flow_event.phase not in (
                trace_model.FlowEventPhase.START,
                trace_model.FlowEventPhase.STEP,
            ):
                continue

            # Follow the flow event graph.
            vsync = trace_utils.get_nearest_following_flow_event(
                present_flow_event, _EVENT_CATEGORY, _DISPLAY_VSYNC_EVENT_NAME
            )

            # If vsync is None, it could be that a frame was dropped or not rendered, find the next vsync in chronological order.
            if vsync is None:
                following_vsyncs = trace_utils.filter_events(
                    model.all_events(),
                    category=_EVENT_CATEGORY,
                    name=_DISPLAY_VSYNC_EVENT_NAME,
                    type=trace_model.Event,
                )
                vsync = next(
                    (
                        v
                        for v in following_vsyncs
                        if v.start >= present_flow_event.start
                    ),
                    None,
                )

            # If there isn't a vsync, loop through and find the next event.
            if vsync is None:
                continue

            latency = vsync.start - present_flow_event.start
            present_latencies.append(round(latency.to_milliseconds_f(), 3))
            vsync_events.append(vsync)

        if len(vsync_events) == 0:
            _LOGGER.fatal("Not enough valid vsyncs")

        present_latency_mean: float = statistics.mean(present_latencies)
        _LOGGER.info(f"Average Present Latency: {present_latency_mean}")

        if len(vsync_events) < 2:
            _LOGGER.fatal(
                f"Fewer than two vsync events are present. Perhaps the trace "
                f"duration is too short to provide fps information"
            )

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
            _LOGGER.fatal("Not enough valid vsyncs")

        fps_mean: float = statistics.mean(fps_values)
        _LOGGER.info(f"Average FPS: {fps_mean}")

        if self._aggregates_only:
            return trace_utils.standard_metrics_set(
                values=present_latencies,
                label_prefix="AppRenderVsyncLatency",
                unit=metrics.Unit.milliseconds,
            ) + trace_utils.standard_metrics_set(
                values=fps_values,
                label_prefix="AppFps",
                unit=metrics.Unit.framesPerSecond,
            )
        else:
            return [
                metrics.TestCaseResult(
                    "AppRenderVsyncLatency",
                    metrics.Unit.milliseconds,
                    present_latencies,
                ),
                metrics.TestCaseResult(
                    "AppFps", metrics.Unit.framesPerSecond, fps_values
                ),
            ]
