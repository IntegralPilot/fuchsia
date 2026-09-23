#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""input trace metrics."""

import logging
import statistics
from typing import MutableSequence

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time, trace_utils

_LOGGER: logging.Logger = logging.getLogger("InputLatencyMetricsProcessor")
_CATEGORY_INPUT: str = "input"
_INPUT_EVENT_NAME: str = "input-device-process-reports"
_POINTERINJECTOR_REGISTER_EVENT_NAME: str = "PointerinjectorRegistry::Register"
_CATEGORY_GFX: str = "gfx"
_DISPLAY_VSYNC_EVENT_NAME: str = "Flatland::DisplayCompositor::OnVsync"


class InputLatencyMetricsProcessor(trace_metrics.MetricsProcessor):
    """Measures the time it takes for an input event to result in user-visible change on-screen.

    Calculates the time needed for an input event to progress through the through input pipeline to
    and result in a change on the screen at vsync. Flow events in the trace enable this class to
    reliably corellate input events to the first user-visible-output that results.

    By default, this module reports aggregate latency measurements -- such as min, max, average, and
    percentiles -- calculated across all input events generated during the test. It can be
    configured to instead report a time series of latency measurements, one for each input event.
    """

    def __init__(self, aggregates_only: bool = True):
        """Constructor.

        Args:
            aggregates_only: When True, generates InputLatencyMin,
                InputLatencyMax, InputLatencyAverage and
                InputLatencyP* (percentiles).
                Otherwise generates InputLatency metric with all
                InputLatency values.
        """
        self._aggregates_only: bool = aggregates_only

    @property
    def event_patterns(self) -> set[str]:
        """This processor follows a flow with many parts, so defer to category_names."""
        return set()

    @property
    def category_names(self) -> set[str]:
        """This processor follows a flow with many parts across `gfx` and `input`."""
        return {_CATEGORY_GFX, _CATEGORY_INPUT}

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        # PointerinjectorRegistry::Register occurs once per Scenic startup when
        # the first event is sent. It may not exist in the trace.
        register_events = trace_utils.filter_events(
            model.all_events(),
            category=_CATEGORY_INPUT,
            name=_POINTERINJECTOR_REGISTER_EVENT_NAME,
            type=trace_model.DurationEvent,
        )
        last_register_end_time: trace_time.TimePoint | None = max(
            (end for e in register_events if (end := e.end_time()) is not None),
            default=None,
        )

        input_events = trace_utils.filter_events(
            model.all_events(),
            category=_CATEGORY_INPUT,
            name=_INPUT_EVENT_NAME,
            type=trace_model.DurationEvent,
        )

        latencies: list[float] = []

        max_latency: float = -1.0
        max_latency_ts: trace_time.TimePoint | None = None

        for e in input_events:
            if (
                last_register_end_time is not None
                and e.start < last_register_end_time
            ):
                continue
            vsync = trace_utils.get_nearest_following_flow_event(
                e, _CATEGORY_GFX, _DISPLAY_VSYNC_EVENT_NAME
            )

            # If vsync is None, it could be that a frame was dropped or not rendered, find the next vsync in chronological order.
            if vsync is None:
                following_vsyncs = trace_utils.filter_events(
                    model.all_events(),
                    category=_CATEGORY_GFX,
                    name=_DISPLAY_VSYNC_EVENT_NAME,
                    type=trace_model.Event,
                )
                vsync = next(
                    (v for v in following_vsyncs if v.start >= e.start),
                    None,
                )

            # If there isn't a vsync, loop through and find the next event.
            if vsync is None:
                continue

            latency = vsync.start - e.start
            latency_ms = latency.to_milliseconds_f()
            latencies.append(latency_ms)
            if latency_ms > max_latency:
                max_latency = latency_ms
                max_latency_ts = e.start

        if not latencies:
            _LOGGER.warning("No valid input latency events found in trace.")
            return []

        if max_latency_ts is not None:
            _LOGGER.info(
                f"InputLatencyMax: {max_latency} ms at timestamp {max_latency_ts}"
            )

        latency_mean: float = statistics.mean(latencies)
        _LOGGER.info(f"Average Present Latency: {latency_mean}")

        if self._aggregates_only:
            return trace_utils.standard_metrics_set(
                values=latencies,
                label_prefix="InputLatency",
                unit=metrics.Unit.milliseconds,
            )
        return [
            metrics.TestCaseResult(
                "total_input_latency",
                metrics.Unit.milliseconds,
                latencies,
            ),
        ]
