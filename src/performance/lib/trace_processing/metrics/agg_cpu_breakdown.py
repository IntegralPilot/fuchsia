#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, TypedDict

from reporting import metrics
from trace_processing.metrics import cpu

# Default cut-off for the percentage CPU. Any process that has CPU below this
# won't be listed in the results. User can pass in a cutoff.
DEFAULT_PERCENT_CUTOFF = 0.0


class Record(TypedDict):
    tid: int
    process_name: str
    thread_name: str
    cpu: int
    duration: float
    percent: float


class AggregateRecord(TypedDict):
    process_name: str
    thread_name: str
    duration: float
    percent: float


def record_from_dict(t: dict[str, metrics.JSON]) -> Record:
    record: dict[str, Any] = {}
    for key, key_type in Record.__annotations__.items():
        assert key in t and isinstance(
            t[key], key_type
        ), f"{t} must contain {key} of type {key_type}"
        record[key] = t[key]
    # Types are manually verified above, and mypy can't figure out that it's
    # safe to unpack `record` and build a `Record` typed-dict with it.
    return Record(**record)  # type: ignore


class AggCpuBreakdownMetricsProcessor:
    """
    Aggregates a given breakdown over the cores for each available frequency,
    and outputs in a free-form metrics format.
    """

    def __init__(
        self,
        # A map from cpu numbers to their frequency.
        # e.g. { 0: 1.8, 1: 1.8, 2: 2.2, 3: 2.2, 4: : 2.2, 5: 2.2 }
        cpu_to_freq: dict[int, float],
        total_time: float,
        percent_cutoff: float = DEFAULT_PERCENT_CUTOFF,
    ) -> None:
        # Transforms the frequency config to a map from cpu to frequency. Used
        # to determine which frequency each record should contribute its duration to.
        self._cpu_to_freq = cpu_to_freq
        self._percent_cutoff = percent_cutoff
        self._total_time = total_time

    def aggregate_metrics(
        self, breakdown: cpu.Breakdown
    ) -> dict[float, list[AggregateRecord]]:
        """
        Given the breakdown of duration per thread, iterates through all the threads' durations for each
        CPU and aggregates them over each CPU frequency.

        Args:
            breakdown: The json output from the cpu_breakdown script.

        """
        # Map from frequency to tid to aggregated duration.
        freq_to_tid_durs: dict[float, dict[int, float]] = {}
        # Tracks the final output. Contains a map from frequency to list of
        # threads with their durations and percentages.
        agg_breakdown: dict[float, list[AggregateRecord]] = {}
        tid_to_thread_name: dict[int, str] = {}
        tid_to_process_name: dict[int, str] = {}
        for r in breakdown:
            # Save process and thread name for tid
            t = record_from_dict(r)
            tid = t["tid"]
            tid_to_process_name[tid] = t["process_name"]
            tid_to_thread_name[tid] = t["thread_name"]

            # Get frequency for the cpu
            freq = self._cpu_to_freq[t["cpu"]]

            # Set the duration sum for the tid to 0 if nonexistent
            freq_to_tid_durs.setdefault(freq, {})
            freq_to_tid_durs[freq].setdefault(tid, 0)

            # Add the duration to the tid
            duration = t["duration"]
            freq_to_tid_durs[freq][tid] += duration

        for freq, tid_durs in freq_to_tid_durs.items():
            dur_list: list[AggregateRecord] = []
            for tid, dur in tid_durs.items():
                percent = (dur / self._total_time) * 100
                if percent >= self._percent_cutoff:
                    dur_list.append(
                        AggregateRecord(
                            {
                                "process_name": tid_to_process_name[tid],
                                "thread_name": tid_to_thread_name[tid],
                                "duration": round(dur, 3),
                                "percent": round(percent, 3),
                            }
                        )
                    )
            agg_breakdown[freq] = sorted(
                dur_list,
                key=lambda m: (m["duration"]),
                reverse=True,
            )
        return agg_breakdown
