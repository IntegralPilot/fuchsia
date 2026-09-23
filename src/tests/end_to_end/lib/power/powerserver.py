#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Fuchsia power test powerserver integration library."""

import csv
import dataclasses
import enum
import itertools
import logging
import operator
import os
import signal
import stat
import subprocess
import sys
import time
from collections import deque
from collections.abc import Iterable, Mapping
from contextlib import contextmanager
from importlib import resources
from typing import Generator, Sequence

from power import data  # type: ignore[attr-defined]
from power import sampler
from trace_processing import trace_model, trace_time

SAMPLE_INTERVAL_NS = 200000

_LOGGER = logging.getLogger(__name__)

# The measurepower tool's path. The tool is expected to periodically output
# power measurements into a csv file, with _PowerCsvHeaders.all() columns.
_MEASUREPOWER_PATH_ENV_VARIABLE = "MEASUREPOWER_PATH"


def weighted_average(arr: Iterable[float], weights: Iterable[int]) -> float:
    return sum(
        itertools.starmap(
            operator.mul,
            zip(arr, weights, strict=True),
        )
    ) / sum(weights)


def normalize(lst: Iterable[float]) -> list[float]:
    """Return a copy of the list normalized between [0, 1.0]."""
    lst_min = min(lst)
    lst_max = max(lst)
    return [(f - lst_min) / (lst_max - lst_min) for f in lst]


# We don't have numpy in the vendored python libraries so we'll have to roll our own correlate
# functionality which will be slooooow.
#
# Normally, we'd compute the cross correlation and then take the argmax to line up the signals the
# closest. Instead we'll do the argmax and the correlation at the same time to save on allocations.
#
# Precondition: len(signal) must be >= len(feature) for accurate results
def cross_correlate_arg_max(
    signal: Sequence[float], feature: Sequence[float]
) -> tuple[float, int]:
    """Cross correlate two 1d signals and return the maximum correlation. Correlation is only done
    where the signals overlap completely.

    Returns
        (correlation, idx)
        Where correlation is the maximum correlation between the signals and idx is the argmax that
        it occurred at.
    """

    assert len(signal) >= len(feature)

    # Slide our feature across the signal and compute the dot product at each point
    return max(
        # Produce items of the form [(correlation1, idx1), (correlation2, idx2), ...] of which we
        # want the highest correlation.
        map(
            lambda base: (
                # Fancy itertools based dot product
                sum(
                    itertools.starmap(
                        operator.mul,
                        zip(
                            itertools.islice(signal, base, base + len(feature)),
                            feature,
                            strict=True,
                        ),
                    )
                ),
                base,
            ),
            # Take a sliding window dot product. E.g. if we have the arrays
            # [1,2,3,4] and [1,2]
            #
            # The sliding window dot product is
            # [
            #     [1,2] o [1,2],
            #     [2,3] o [1,2],
            #     [3,4] o [1,2],
            # ]
            range(len(signal) - len(feature) + 1),
        )
    )


# Constants class
#
# One of two formats for the CSV data is expected.  If the first column is named
# simply "Timestamp", where the scripts are expected to synthesize nominal
# timestamps for the samples.  If it is named "Mandatory Timestamp" instead,
# then it is data captured from a script which has already synthesized its own
# timestamps, accounting for dropped samples, calibration samples, and the power
# monitor deviations from nominal.  These timestamps should be simply taken and
# used as is.
#
# In the second case, the script can (if asked to) also capture a 4th column of
# data representing the raw 16 bit readings from the fine auxiliary current
# channel.  When present, these readings provide an alternative method for
# aligning the timelines of the power monitor data with the trace data.
#
class _PowerCsvHeaders(enum.StrEnum):
    MANDATORY_TIMESTAMP = "Mandatory Timestamp"
    TIMESTAMP = "Timestamp"
    CURRENT = "Current"
    VOLTAGE = "Voltage"
    AUX_CURRENT = "Raw Aux"

    @staticmethod
    def assert_header(header: list[str]) -> None:
        assert header[1] == _PowerCsvHeaders.CURRENT
        assert header[2] == _PowerCsvHeaders.VOLTAGE
        if header[0] == _PowerCsvHeaders.MANDATORY_TIMESTAMP:
            assert len(header) == 3 or header[3] == _PowerCsvHeaders.AUX_CURRENT
        else:
            assert header[0] == _PowerCsvHeaders.TIMESTAMP


class _NoopPowerSampler(sampler.PowerSampler):
    """A no-op power sampler, used in environments where _MEASUREPOWER_PATH_ENV_VARIABLE isn't set."""

    def _start_impl(self) -> None:
        pass

    def _stop_impl(self) -> None:
        pass


class _RealPowerSampler(sampler.PowerSampler):
    """Wrapper for the measurepower command-line tool."""

    def __init__(
        self,
        config: sampler.PowerSamplerConfig,
        monsoon_serial: str | None = None,
    ) -> None:
        """Constructor.

        Args:
            config (PowerSamplerConfig): Configuration.
        """
        super().__init__(config)
        assert config.measurepower_path
        self._measurepower_proc: subprocess.Popen[str] | None = None
        self._csv_output_path = os.path.join(
            self._config.output_dir,
            f"{self._config.metric_name}_power_samples.csv",
        )
        self._sampled_data = False
        self._monsoon_serial = monsoon_serial

    def _start_impl(self) -> None:
        _LOGGER.info("Starting power sampling")
        self._start_power_measurement()
        self._await_first_sample()
        self._sampled_data = True

    def _stop_impl(self) -> None:
        _LOGGER.info("Stopping power sampling...")
        self._stop_power_measurement()
        _LOGGER.info("Power sampling stopped")

    def _start_power_measurement(self) -> None:
        assert self._config.measurepower_path
        cmd = []
        if self._config.use_hermetic_measurepower:
            cmd += [sys.executable]
        cmd += [
            self._config.measurepower_path,
            "-format",
            "csv",
            "-out",
            self._csv_output_path,
        ]
        if self._monsoon_serial:
            cmd += ["--serialno", self._monsoon_serial]
        _LOGGER.debug(f"Power measurement cmd: {cmd}")
        self._measurepower_proc = subprocess.Popen(
            cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )

    def _await_first_sample(self, timeout_sec: float = 60) -> None:
        _LOGGER.debug(f"Awaiting 1st power sample (timeout_sec={timeout_sec})")
        assert self._measurepower_proc
        proc = self._measurepower_proc
        csv_path = self._csv_output_path
        deadline = time.time() + timeout_sec

        while time.time() < deadline:
            if proc.poll():
                stdout = proc.stdout.read() if proc.stdout else None
                stderr = proc.stderr.read() if proc.stderr else None
                raise RuntimeError(
                    f"Measure power failed to start with status "
                    f"{proc.returncode} stdout: {stdout} "
                    f"stderr: {stderr}"
                )
            if os.path.exists(csv_path) and os.path.getsize(csv_path):
                _LOGGER.debug(f"Received 1st power sample in {csv_path}")
                return
            time.sleep(1)

        raise TimeoutError(
            f"Timed out after {timeout_sec} seconds while waiting for power samples"
        )

    def _stop_power_measurement(self, timeout_sec: float = 60) -> None:
        _LOGGER.debug("Stopping the measurepower process...")
        proc = self._measurepower_proc
        assert proc
        proc.send_signal(signal.SIGINT)
        result = proc.wait(timeout_sec)
        if result:
            stdout = proc.stdout.read() if proc.stdout else None
            stderr = proc.stderr.read() if proc.stderr else None
            raise RuntimeError(
                f"Measure power failed once stopped with status"
                f"{proc.returncode} stdout: {stdout} "
                f"stderr: {stderr}"
            )
        _LOGGER.debug("measurepower process stopped.")

    def should_generate_load(self) -> bool:
        return True

    def has_samples(self) -> bool:
        return self._sampled_data

    def extract_samples(self) -> Sequence[sampler.Sample]:
        return read_power_samples(self._csv_output_path)


def create_power_sampler(
    config: sampler.PowerSamplerConfig, fallback_to_stub: bool = True
) -> sampler.PowerSampler:
    """Creates a power sampler.

    In the absence of `_MEASUREPOWER_PATH_ENV_VARIABLE`, creates a no-op sampler.
    """

    measurepower_path = config.measurepower_path or os.environ.get(
        _MEASUREPOWER_PATH_ENV_VARIABLE
    )
    if config.disabled:
        _LOGGER.info("Using no-op power sampler due to disabled configuration.")
        return _NoopPowerSampler(config)
    elif not measurepower_path:
        if not fallback_to_stub:
            raise RuntimeError(
                f"{_MEASUREPOWER_PATH_ENV_VARIABLE} env variable must be set"
            )

        _LOGGER.warning(
            f"{_MEASUREPOWER_PATH_ENV_VARIABLE} env variable not set. "
            "Using a no-op power sampler instead."
        )
        return _NoopPowerSampler(config)
    else:
        config = dataclasses.replace(
            config, measurepower_path=measurepower_path
        )
    return _RealPowerSampler(config)


@contextmanager
def with_hermetic_monsoon_power_sampler(
    output_dir: str,
    metric_name: str,
    monsoon_serial: str,
) -> Generator[sampler.PowerSampler, None, None]:
    """Returns a monsoon-backed power sampler."""

    if not monsoon_serial:
        raise ValueError("monsoon_serial must be provided")

    _MONSOON_MEASURE_POWER_TOOL = "monsoon_measure_power.pyz"

    with resources.path(data, _MONSOON_MEASURE_POWER_TOOL) as measurepower_path:
        if not measurepower_path:
            raise RuntimeError(
                f"Could not find {_MONSOON_MEASURE_POWER_TOOL} in package {data}"
            )

        st = os.stat(measurepower_path)
        os.chmod(measurepower_path, st.st_mode | stat.S_IEXEC)

        config = sampler.PowerSamplerConfig(
            output_dir, metric_name, False, str(measurepower_path), True
        )
        yield _RealPowerSampler(config, monsoon_serial)


def _read_fuchsia_trace_cpu_usage(
    model: trace_model.Model,
) -> Mapping[int, Sequence[tuple[trace_time.TimePoint, float]]]:
    """
    Read through the given fuchsia trace and return:

    Args:
        model: the model to extrace cpu usage data from

    Returns:
        {cpu: [timestamp, usage]}
        where the timestamp is in ticks and usage is a 0.0 or 1.0 depending on if a
        processes was scheduled for that interval.
    """
    scheduling_intervals = {}

    for cpu, intervals in model.scheduling_records.items():
        scheduling_intervals[cpu] = list(
            map(
                lambda record: (record.start, 0.0 if record.is_idle() else 1.0),
                # Drop the waking records, the don't count towards cpu usage.
                filter(
                    lambda record: isinstance(
                        record, trace_model.ContextSwitch
                    ),
                    intervals,
                ),
            )
        )
        # The records are _probably_ sorted as is, but there's no guarantee of it.
        # Let's guarantee it.
        scheduling_intervals[cpu].sort(key=lambda kv: kv[0])

    return scheduling_intervals


def read_power_samples(power_trace_path: str) -> list[sampler.Sample]:
    """Return a tuple of the current and power samples from the power csv"""
    samples: list[sampler.Sample] = []
    with open(power_trace_path, "r") as power_csv:
        reader = csv.reader(power_csv)
        header = next(reader)
        sample_count = 0

        if header[0] == "Mandatory Timestamp":
            if len(header) < 4:
                create_sample = lambda line, count: sampler.Sample(
                    line[0], line[1], line[2], None
                )
            else:
                create_sample = lambda line, count: sampler.Sample(
                    line[0], line[1], line[2], line[3]
                )
        else:
            create_sample = lambda line, count: sampler.Sample(
                count * SAMPLE_INTERVAL_NS, line[1], line[2], None
            )

        for line in reader:
            samples.append(create_sample(line, sample_count))
            sample_count += 1

    return samples


def build_usage_samples(
    scheduling_intervals: Mapping[
        int, Sequence[tuple[trace_time.TimePoint, float]]
    ]
) -> dict[int, list[float]]:
    usage_samples = {}
    # Our per cpu records likely don't all start and end at the same time.
    # We'll pad the other cpu tracks with idle time on either side.
    max_len = 0
    earliest_ts = min(x[0][0] for x in scheduling_intervals.values())
    for cpu, intervals in scheduling_intervals.items():
        # The power samples are a fixed interval apart. We'll synthesize cpu
        # usage samples that also track over the same interval in this array.
        cpu_usage_samples = []

        # To make the conversion, let's start by converting our list of
        # [(start_time, work)] into [(duration, work)]. We could probably do this
        # in one pass, but doing this conversion first makes the logic easier
        # to reason about.
        (prev_ts, prev_work) = intervals[0]

        # Idle pad the beginning of our intervals to start from a fixed timestamp.
        weighted_work = deque([(prev_ts - earliest_ts, 0.0)])
        for ts, work in intervals[1:]:
            weighted_work.append((ts - prev_ts, prev_work))
            (prev_ts, prev_work) = (ts, work)

        # Finally, to get our fixed sample intervals, we'll use our [(duration,
        # work)] list and pop chunks of work `sample_interval` ticks at a time.
        # Then once we've accumulated enough weighted work to fill the
        # schedule, we'll take the weighted average and call that our cpu usage
        # for the interval.
        usage: list[float] = []
        durations: list[int] = []
        interval_duration_remaining = trace_time.TimeDelta(SAMPLE_INTERVAL_NS)

        for duration, work in weighted_work:
            if interval_duration_remaining - duration >= trace_time.TimeDelta():
                # This duration doesn't fill or finish a full sample interval,
                # just append it to the accumulator lists.
                usage.append(work)
                durations.append(duration.to_nanoseconds())
                interval_duration_remaining -= duration
            else:
                # We have enough work to record a sample. Insert what we need
                # to top off the interval and add it to cpu_usage_samples
                partial_duration = interval_duration_remaining
                usage.append(work)
                durations.append(partial_duration.to_nanoseconds())
                duration -= partial_duration

                average = weighted_average(usage, durations)
                cpu_usage_samples.append(average)

                # Now use up the rest of the duration. Not that it's possible
                # that this duration might actually be longer a full sampling
                # interval so we should synthesize multiple samples in that
                # case.
                remaining_duration = trace_time.TimeDelta(
                    duration.to_nanoseconds() % SAMPLE_INTERVAL_NS
                )
                num_extra_intervals = int(
                    duration.to_nanoseconds() / SAMPLE_INTERVAL_NS
                )
                cpu_usage_samples.extend(
                    [work for _ in range(0, num_extra_intervals)]
                )

                # Reset our accumulators with the leftover bits
                usage = [work]
                durations = [remaining_duration.to_nanoseconds()]
                interval_duration_remaining = (
                    trace_time.TimeDelta(SAMPLE_INTERVAL_NS)
                    - remaining_duration
                )

        usage_samples[cpu] = cpu_usage_samples
        max_len = max(max_len, len(cpu_usage_samples))

    # Idle pad the end of our intervals to all contain the same number
    for cpu, samples in usage_samples.items():
        samples.extend([0 for _ in range(len(samples), max_len)])
    return usage_samples


def merge_power_data(
    model: trace_model.Model,
    power_samples: Sequence[sampler.Sample],
    fxt_path: str,
) -> None:
    """Merges power_samples into model, placing merged trace at fxt_path.

    NB: model MUST be the in-memory representation of the serialized trace stored at fxt_path.
        This function merely appends data to fxt_path; it cannot write trace events other than
        synthetic events representing the data from power_samples.

    Upon return, the serialized trace at fxt_path will include the events from model along with
    synthesized events representing the data in power_samples.

    Args:
        model: The in-memory representation of the trace model stored at fxt_path.
        power_samples: Power readings to merge with model.
        fxt_path: A path to a serialized trace which has been read into memory and provided in
                  model.
    """
    # We'll start by reading in the fuchsia cpu data from the trace model
    scheduling_intervals = _read_fuchsia_trace_cpu_usage(model)

    # We can't just append the power data to the beginning of the trace. The
    # trace starts before the power collection starts at some unknown offset.
    # We'll have to first find this offset.
    #
    # We know that CPU usage and current draw are highly correlated. So we'll
    # have the test start by running a cpu intensive workload in a known
    # pattern for the first few seconds before starting the test. We can then
    # synthesize cpu usage over the same duration and attempt to correlate the
    # signals.

    # The power samples come in at fixed intervals. We'll construct cpu usage
    # data in the same intervals to attempt to correlate it at each offset.
    # We'll assume the highest correlated offset is the delay the power
    # sampling started at and we'll insert the samples starting at that
    # timepoint.
    earliest_ts = min(x[0][0] for x in scheduling_intervals.values())

    # The trace model give us per cpu information about which processes start at
    # which time. To compare it to the power samples we need to convert it into
    # something of the form ["usage_sample_1", "usage_sample_2", ...] where
    # each "usage_sample_n" is the cpu usage over a 200us duration.
    usage_samples = build_usage_samples(scheduling_intervals)

    # Now take take the average cpu usage across each cpu to get overall cpu usage which should
    # correlate to our power/current samples.
    merged = [samples for _, samples in usage_samples.items()]
    avg_cpu_combined = [0.0] * len(merged[0])

    for i, _ in enumerate(avg_cpu_combined):
        total: float = 0
        for sample_list in merged:
            total += sample_list[i]
        avg_cpu_combined[i] = total / len(merged)

    # Finally, we can get the cross correlation between power and cpu usage. We run a known cpu
    # heavy workload in the first 5ish seconds of the test so we limit our signal correlation to
    # that portion. Power and CPU readings can be offset in either direction, but shouldn't be
    # separated by more than a second.

    # Due to limits on the maximum size of our traces, there is potential for particularly busy
    # traces to not contain the entire first 5 seconds of the test. In order to ensure the
    # correlation logic operates correctly, we need to compare consistent counts of samples for
    # power and CPU readings. Thus, we take the a subset of the two datasets limited to a maximum
    # of ~5 seconds worth of samples. (5khz * 5 seconds = 25000 samples) and compare it to the
    # first ~4 seconds (80% of ~5 seconds) of the opposite dataset to attempt to match them up.
    # This amounts to nominally comparing the first 4 seconds of power samples to the first 5
    # seconds of CPU readings, and then vice-versa.
    max_sequence_samples = 25000
    signal_samples = min(
        max_sequence_samples, len(avg_cpu_combined), len(power_samples)
    )

    current_samples = normalize([sample.current for sample in power_samples])
    avg_cpu_combined = normalize(avg_cpu_combined)

    # Ensures feature list is always shorter than the number of signal samples
    feature_samples = int(0.8 * signal_samples)
    (
        power_after_cpu_correlation,
        power_after_cpu_correlation_idx,
    ) = cross_correlate_arg_max(
        avg_cpu_combined[0:signal_samples],
        current_samples[0:feature_samples],
    )
    (
        cpu_after_power_correlation,
        cpu_after_power_correlation_idx,
    ) = cross_correlate_arg_max(
        current_samples[0:signal_samples],
        avg_cpu_combined[0:feature_samples],
    )
    starting_ticks = 0
    if power_after_cpu_correlation >= cpu_after_power_correlation:
        offset_ns = power_samples[power_after_cpu_correlation_idx].timestamp

        print(f"Delaying power readings by {offset_ns/1000/1000}ms")
        starting_ticks = int(
            (earliest_ts + trace_time.TimeDelta(offset_ns))
            .to_epoch_delta()
            .to_nanoseconds()
            * sampler.TICKS_PER_NS
        )
    else:
        offset_ns = power_samples[cpu_after_power_correlation_idx].timestamp

        print(f"Delaying CPU trace by {offset_ns/1000/1000}ms")
        starting_ticks = int(
            (earliest_ts - trace_time.TimeDelta(offset_ns))
            .to_epoch_delta()
            .to_nanoseconds()
            * sampler.TICKS_PER_NS
        )

    sampler.append_power_data(fxt_path, power_samples, starting_ticks)
