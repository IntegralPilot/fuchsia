#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Power trace metrics."""

import dataclasses
import itertools
import logging
from typing import Callable, Iterator, MutableSequence, Sequence

from reporting import metrics
from trace_processing import trace_metrics, trace_model, trace_time, trace_utils
from trace_processing.metrics import suspend as suspend_metrics

_LOGGER: logging.Logger = logging.getLogger(__name__)
_LOAD_GEN = "load_generator"
_SAG = "system-activity-governor"
_EVENT_CATEGORY = "power"
# LINT.IfChange
_SAG_EVENT_NAME = "system-activity-governor:suspend"
# LINT.ThenChange(//src/power/system-activity-governor/src/cpu_manager.rs)


@dataclasses.dataclass
class _Sample:
    """A sample of collected power metrics.

    Args:
      timestamp: timestamp of sample in nanoseconds since epoch.
      voltage: voltage in Volts.
      current: current in milliAmpere.
      raw_aux: (optional) The raw 16 bit fine aux channel reading from a Monsoon power monitor.  Can
               optionally be used for log synchronization and alignment
    """

    timestamp: int
    voltage: float
    current: float
    raw_aux: int | None

    @property
    def power(self) -> float:
        """Power in Watts."""
        return self.voltage * self.current * 1e-3


def _running_avg(avg: float, value: float, count: int) -> float:
    return avg + (value - avg) / count


@dataclasses.dataclass
class _AggregateMetrics:
    """Aggregate power metrics representation.

    Represents aggregated metrics over a number of power metrics samples.

    Args:
      sample_count: number of power metric samples.
      max_power: maximum power in Watts over all samples.
      mean_power: average power in Watts over all samples.
      min_power: minimum power in Watts over all samples.
    """

    sample_count: int = 0
    max_power: float | None = None
    mean_power: float = 0
    min_power: float | None = None

    DESCRIPTION_BASE = "Power usage sampled during test"
    """Stable format for descriptions of aggregate metrics."""

    def process_sample(self, sample: _Sample) -> None:
        """Process a sample of power metrics.

        Args:
            sample: A sample of power metrics.
        """
        self.sample_count += 1
        self.max_power = (
            max(self.max_power, sample.power)
            if self.max_power is not None
            else sample.power
        )
        self.mean_power = _running_avg(
            self.mean_power, sample.power, self.sample_count
        )
        self.min_power = (
            min(self.min_power, sample.power)
            if self.min_power is not None
            else sample.power
        )

    @property
    def is_empty(self) -> bool:
        """Returns true if no samples have been processed yet."""
        return self.max_power is None or self.min_power is None

    def _build_expl(self, condition: str, aggregate: str) -> str:
        """Builds a properly formatted explanation for a given power metric."""
        if not condition:
            return f"{_AggregateMetrics.DESCRIPTION_BASE}, {aggregate}"
        return f"{_AggregateMetrics.DESCRIPTION_BASE} {condition}, {aggregate}"

    def to_fuchsiaperf_results(
        self, tag: str, condition: str
    ) -> list[metrics.TestCaseResult]:
        """Converts Power metrics to fuchsiaperf JSON object.

        Args:
            tag: a descriptive word to add to the end of metric names, e.g. "suspend"
            condition: a descriptive string to add to the explanatory text used for these metrics,
                       e.g. "while device is suspended"

        Returns:
          List of JSON object.
        """
        assert self.min_power is not None and self.max_power is not None
        suffix = f"_{tag}" if tag else ""
        results: list[metrics.TestCaseResult] = [
            metrics.TestCaseResult(
                label="MinPower" + suffix,
                unit=metrics.Unit.watts,
                values=[self.min_power],
                doc=self._build_expl(condition, "minimum"),
            ),
            # TODO(cmasone): Add MedianPower metrics
            metrics.TestCaseResult(
                label="MeanPower" + suffix,
                unit=metrics.Unit.watts,
                values=[self.mean_power],
                doc=self._build_expl(condition, "mean"),
            ),
            metrics.TestCaseResult(
                label="MaxPower" + suffix,
                unit=metrics.Unit.watts,
                values=[self.max_power],
                doc=self._build_expl(condition, "maximum"),
            ),
        ]
        return results


class PowerMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes aggregate power consumption metrics.

    Given a trace containing power samples, separately computes aggregate power usage metrics for:
    * The duration of the entire workload captured by the trace.
    * Any periods of suspension for the device.
    * Periods during which the device is NOT suspended (iff suspends are detected).
    """

    def __init__(
        self,
        good_suspend_pred: Callable[[trace_time.Window], bool] | None = None,
    ) -> None:
        """
        Args:
            good_suspend_pred: Optional predicate for which suspend windows should be included
                in metrics calculation. If provided, must return True for any window the caller
                wishes to take into account, False for those that should be ignored.
        """
        self._good_suspend_pred = good_suspend_pred

    @property
    def event_patterns(self) -> set[str]:
        return suspend_metrics.EVENT_PATTERNS

    def process_metrics(
        self, model: trace_model.Model
    ) -> MutableSequence[metrics.TestCaseResult]:
        """Calculate power metrics, excluding power/trace sync signals.

        In order to sync power measurements with a system trace, our test harness
        generates some structured CPU load (and corresponding power consumption data).
        This portion of the data must be excluded from metrics calculation.

        Args:
            model: In-memory representation of a merged power and system trace.

        Returns:
            Set of metrics results for this test case.
        """
        test_start = _find_test_start(model)
        if test_start is None:
            _LOGGER.info(
                "No load_generator scheduling records present. Power data may not have been "
                "merged into the model."
            )
            return []

        post_sync_model = model.slice(test_start)
        metrics_events = trace_utils.filter_events(
            post_sync_model.all_events(),
            name="Metrics",
            type=trace_model.CounterEvent,
        )

        suspend_windows = [
            window
            for window in _find_suspend_windows(post_sync_model)
            if self.is_good_suspend_window(window)
        ]
        _LOGGER.info(f"Identified suspend windows: {suspend_windows}")

        power_metrics = _AggregateMetrics()
        running_power_metrics = _AggregateMetrics()
        suspend_power_metrics = _AggregateMetrics()
        for me in metrics_events:
            # These args are set in _append_power_data()
            # found in //src/tests/end_to_end/power/power_test_utils.py
            if "Voltage" in me.args and "Current" in me.args:
                sample = _Sample(
                    timestamp=me.start.to_epoch_delta().to_nanoseconds(),
                    voltage=float(me.args["Voltage"]),
                    current=float(me.args["Current"]),
                    raw_aux=int(me.args["Raw Aux"])
                    if "Raw_Aux" in me.args
                    else None,
                )
                power_metrics.process_sample(sample)
                if any(me.start in window for window in suspend_windows):
                    suspend_power_metrics.process_sample(sample)
                else:
                    running_power_metrics.process_sample(sample)

        if power_metrics.is_empty:
            _LOGGER.warning(
                "No power metric records after load_generator sync signal. "
                "There may have been an error processing power or trace data."
            )
            return []

        results = power_metrics.to_fuchsiaperf_results("", "")

        # If the system was suspended, also report suspended and non-suspended
        # (running) power metrics.
        if not suspend_power_metrics.is_empty:
            results.extend(
                suspend_power_metrics.to_fuchsiaperf_results(
                    "suspend", "while device is suspended"
                )
            )
            results.extend(
                running_power_metrics.to_fuchsiaperf_results(
                    "running", "while device is awake"
                )
            )

        return results

    def is_good_suspend_window(self, window: trace_time.Window) -> bool:
        return self._good_suspend_pred is None or self._good_suspend_pred(
            window
        )


def _find_test_start(model: trace_model.Model) -> trace_time.TimePoint | None:
    """Identify the point at which the test workload began.

    In order to sync power measurements with a system trace, our test harness
    runs a process on-device that generates structured CPU load. This process
    is named `load_generator.cm`. The first TimePoint after all the threads of
    this process exit is the first moment that data should be used for metrics
    calculation.

    Args:
        model: In-memory representation of a merged power and system trace.

    Returns:
        The first TimePoint after power/system trace sync signals complete.
    """
    load_generator: trace_model.Process | None = None
    for proc in model.processes:
        if proc.name and proc.name.startswith(_LOAD_GEN):
            load_generator = proc
            break

    if not load_generator:
        return None

    load_generator_threads = [t.tid for t in load_generator.threads]

    def is_last_generator_thread_record(
        r: trace_model.ContextSwitch,
    ) -> bool:
        return (
            r.outgoing_tid in load_generator_threads
            and r.outgoing_state == trace_model.ThreadState.ZX_THREAD_STATE_DEAD
        )

    records = sorted(
        filter(
            is_last_generator_thread_record,
            trace_utils.filter_records(
                itertools.chain.from_iterable(
                    model.scheduling_records.values()
                ),
                trace_model.ContextSwitch,
            ),
        ),
        key=lambda r: r.start,
    )
    return records[-1].start if records else None


def _find_suspend_windows(
    model: trace_model.Model,
) -> Sequence[trace_time.Window]:
    """Identify periods of suspension in model.

    Inspect scheduling records in TimePoint order across all CPUs to identify periods of
    suspension.

        Args:
            model: In-memory representation of a system trace.

        Returns:
            Windows of time during which the device was suspended.
    """
    system_activity_governor: trace_model.Process | None = None
    for proc in model.processes:
        if proc.name and proc.name.startswith(_SAG):
            system_activity_governor = proc
            break

    if not system_activity_governor:
        _LOGGER.info(f"No {_SAG} process; skipping suspend metrics")
        return []

    suspend_windows: list[trace_time.Window] = []
    events = filter(
        lambda e: e.pid == system_activity_governor.pid,
        list(_filter_sag_suspend_events(model)),
    )
    for suspend in events:
        if suspend.duration is None:
            _LOGGER.warning("Skipping suspend event with empty duration")
            continue
        suspend_windows.append(
            trace_time.Window(suspend.start, suspend.start + suspend.duration)
        )

    return suspend_windows


def _filter_sag_suspend_events(
    model: trace_model.Model,
) -> Iterator[trace_model.DurationEvent]:
    """Extract SAG suspend duration events from the provided trace model.

    Args:
        model: In-memory representation of a system trace.

    Returns:
        Iterator of SAG suspend duration events emitted by the power subsystem.

    """
    return trace_utils.filter_events(
        model.all_events(),
        category=_EVENT_CATEGORY,
        name=_SAG_EVENT_NAME,
        type=trace_model.DurationEvent,
    )
