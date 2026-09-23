#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import itertools
import logging
from typing import Iterable, Iterator, MutableSequence

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_utils

_LOGGER: logging.Logger = logging.getLogger(__name__)
_GPU_USAGE_EVENT_NAME = "GPU Utilization"


class GpuMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes GPU utilization metrics.

    GPU utilization metrics mark the percentage of time the GPU is busy performing
    work. It is a value from 0 to 1.
    """

    def __init__(
        self,
        aggregates_only: bool = True,
    ):
        """Constructor.

        Args:
            aggregates_only: When True, generates GpuMin, GpuMax, GpuAverage and GpuP* (%iles).
                Otherwise generates GpuLoad metric with all gpu values.
        """
        self.aggregates_only: bool = aggregates_only

    @property
    def event_patterns(self) -> set[str]:
        return {_GPU_USAGE_EVENT_NAME}

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        all_events: Iterator[trace_model.Event] = model.all_events()
        gpu_usage_events: Iterable[
            trace_model.CounterEvent
        ] = trace_utils.filter_events(
            all_events,
            name=_GPU_USAGE_EVENT_NAME,
            type=trace_model.CounterEvent,
        )

        gpu_starts: list[float] = []
        gpu_percentages: list[float] = []

        # Parse the start time and percentage for each `Event`.
        for event in gpu_usage_events:
            gpu_starts.append(event.start.to_epoch_delta().to_nanoseconds())
            gpu_percentages.append(event.args.get("utilization") or 0)

        # The duration for each utilization is the time between it and the next one.
        # We don't know the duration for the last event so add one '0'.
        gpu_durations: list[float] = [
            curr - prev for prev, curr in itertools.pairwise(gpu_starts)
        ] + [0]

        if len(gpu_percentages) == 0:
            _LOGGER.info(
                "No gpu usage measurements are present. Perhaps the trace "
                "duration is too short to provide gpu usage information"
            )
            return []

        if len(gpu_durations) != len(gpu_percentages):
            _LOGGER.warning(
                "The number of GPU duration segments don't match the number "
                f"of percentages. ({len(gpu_durations)} != {len(gpu_percentages)}) Data may be truncated."
            )

        if self.aggregates_only:
            return trace_utils.standard_metrics_set(
                values=gpu_percentages,
                label_prefix="Gpu",
                unit=metrics.Unit.percent,
                durations=gpu_durations,
            )
        return [
            metrics.TestCaseResult(
                "GpuUtilization", metrics.Unit.percent, gpu_percentages
            )
        ]
