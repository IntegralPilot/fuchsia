# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import json
import os
import time
from abc import ABC, abstractmethod
from typing import Any, Generator, Generic, TypeVar

P = TypeVar("P")
R = TypeVar("R")


class ActionTimer(ABC, Generic[P, R]):
    """Base class that times an action and records the time to a file.

    This utility class allows a subclass to execute an action 1 or more times,
    capture the wallclock time taken for each iteration, and record them to a
    file.

    Duration measurements will be recorded to a Fuchsia freeform metrics file,
    with the suffix `freeform.json`.
    """

    def execute(
        self,
        test_suite: str,
        label: str,
        results_path: str,
        repetitions: int = 1,
    ) -> None:
        """Executes and measures the duration of an action.

        Args:
            test_suite: the name of the test suite for the output
                freeform.json.
            label: the label for the test within the test suite.
            results_path: the path where the freeform.json file will be
                written.
            repetitions: the number of times to execute an action.
        """
        time_ns: list[int] = []
        self.additional_json: list[Any] = []
        for _ in range(repetitions):
            pre_result = self.pre_action()
            start = time.monotonic_ns()
            result = self.action(pre_result)
            dur = time.monotonic_ns() - start
            time_ns.append(dur)
            self.post_action(result)
        _write_to_file(
            test_suite, label, results_path, time_ns, self.additional_json
        )

    @abstractmethod
    def pre_action(self) -> P:
        """Set up code to execute before the action.

        The duration of this method won't be measured.

        The output of this method will be passed to the action when executing.
        """

    @abstractmethod
    def action(self, pre_result: P) -> R:
        """The action which duration will be measured.

        The duration of this method will be measured.

        The output of this method will be passed to the post_action method when
        executing.

        Args:
            pre_result: the output of self.pre_action.
        """

    @abstractmethod
    def post_action(self, step_output: R) -> None:
        """Executed after the action.

        The duration of this method won't be measured.

        Args:
            step_output: the output of self.action.
        """


def _write_to_file(
    test_suite: str,
    label: str,
    results_path: str,
    time_ns: list[int],
    additional_json: list[Any] | None = None,
) -> None:
    fuchsiaperf_data = [
        {
            "test_suite": test_suite,
            "label": label,
            "values": time_ns,
            "unit": "ns",
        },
    ]
    if additional_json:
        fuchsiaperf_data.extend(additional_json)
    test_perf_file = os.path.join(
        results_path, f"results.{label.lower()}.freeform.json"
    )
    with open(test_perf_file, "w") as f:
        json.dump(fuchsiaperf_data, f, indent=4)


class _Recorder:
    def __init__(self) -> None:
        self.time_ns: list[int] = []

    @contextlib.contextmanager
    def record_iteration(self) -> Generator[None, None, None]:
        """Time how long this context is open and remember it."""
        start = time.monotonic_ns()
        try:
            yield
        finally:
            self.time_ns.append(time.monotonic_ns() - start)


@contextlib.contextmanager
def timer(
    test_suite: str,
    label: str,
    results_path: str,
) -> Generator[_Recorder, None, None]:
    """Supports timing one or more actions and reporting the data as a set of freeform metrics.

    While inside the context created by this context manager, the caller can measure the time
    taken by one or more code blocks. The measurements are reported out as freeform metrics when
    exiting the context.

    Measurements are captured using a nested context:
    ```
        with action_timer.timer("suite", "label", "/path/to/output") as t:
            for _ in range(10):
                with t.record_iteration():
                    intermediate = do_first_thing()
                    side_effect_stuff()
                    result = do_second_thing(intermediate)
                # do stuff with result, if you want
    ```

    Args:
        test_suite: The name of the test suite during which these measurements are captured.
        label: An explanatory name for this set of measurements.
        results_path: A directory in which to emit the metrics.
    """
    recorder = _Recorder()
    try:
        yield recorder
    finally:
        _write_to_file(test_suite, label, results_path, recorder.time_ns)
