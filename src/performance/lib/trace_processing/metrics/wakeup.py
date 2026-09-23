# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Wakeup trace metrics."""

from typing import MutableSequence

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time


class WakeupMetricsProcessor(trace_metrics.MetricsProcessor):
    """
    Computes time taken for the device to process a "wakeup", as defined by
    the provided series of trace events.

    A wakeup sequence is specified by a list `event_names` of length N. A sequence is defined as
    follows:
    - All events in the sequence are duration events.
    - The sequence begins at the end of an event named `event_names[0]`.
    - The sequence is completed when all further events are observed in a strict sequence, with
      an instance of `event_names[i+1]` beginning strictly after an instance of `event_names[i]`,
      for each i in range(0, N-2).
    - A second instance of `event_names[0]` cannot occur between the instance of `event_names[0]`
      that begins a sequence and the instance of `event_names[N-1] that ends it.
    """

    def __init__(
        self, label: str, event_names: list[str], require_wakeup: bool = False
    ) -> None:
        """
        Args:
            label: Report wakeup durations under this TestCaseResult label.
            event_names: Duration events that define a wakeup sequence. See class docstring for
              details.
            require_wakeup: When true, raise exception when no wakeup observed.
        """
        self._label = label
        self._event_names = event_names
        self._require_wakeup = require_wakeup

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        """Calculate Wakeup metrics.

        Args:
            model: In-memory representation of a system trace.

        Raises:
            RuntimeError: When a wakeup is required and none were observed.

        Returns:
            Sequence with a TestCaseResult of Wakeup durations (or an empty
                list if no wakeup sequences were observed).
        """
        events = sorted(model.all_events(), key=lambda e: e.start)

        wakeup_start: trace_time.TimePoint | None = None
        wakeup_durations: list[float] = []
        next_event_index = 0

        for e in events:
            # An instance of `event_names[0]` will start tracking a new sequence. If one was
            # already in progress, it is discarded.
            if e.name == self._event_names[0]:
                # We might be looking for a duration event that's coupled to a flow event, so if we
                # see a non-duration event, just skip it.
                if not isinstance(e, trace_model.DurationEvent):
                    continue
                if e.duration is None:
                    raise RuntimeError(f"Event {e} has no duration.")

                wakeup_start = e.start + e.duration
                next_event_index = 1
            elif e.name == self._event_names[next_event_index]:
                # We might be looking for a duration event that's coupled to a flow event, so if we
                # see a non-duration event, just skip it.
                if not isinstance(e, trace_model.DurationEvent):
                    continue
                if e.duration is None:
                    raise RuntimeError(f"Event {e} has no duration.")

                if next_event_index == len(self._event_names) - 1:
                    assert wakeup_start is not None
                    wakeup_durations += [
                        (e.start + e.duration - wakeup_start)._delta
                    ]
                    wakeup_start = None
                    next_event_index = 0
                else:
                    next_event_index += 1

        if self._require_wakeup and not wakeup_durations:
            obs = ",".join(self._event_names[:next_event_index])
            mis = self._event_names[next_event_index]
            extras = f"observed events: '{obs}', missing event: '{mis}'"
            raise RuntimeError(
                f"Required wakeup not present in trace, {extras}"
            )

        return (
            [
                metrics.TestCaseResult(
                    label=self._label,
                    unit=metrics.Unit.nanoseconds,
                    values=wakeup_durations,
                )
            ]
            if wakeup_durations
            else []
        )
