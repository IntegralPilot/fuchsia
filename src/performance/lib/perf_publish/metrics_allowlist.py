# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Provides an abstraction to assert that a set of metrics matches the expected one."""

import os
from collections.abc import Set

_OPTIONAL_SUFFIX: str = " [optional]"


class MetricsAllowlist:
    def __init__(self, file_path: str | os.PathLike[str]):
        """Creates a new metric allowlist.

        Args:
          file_path: path to the file containing the expected metrics.
        """
        self.file_path = file_path
        self.optional_metrics: set[str] = set()
        self.expected_metrics: set[str] = set()
        self._should_summarize = True

        allow_command = True
        with open(self.file_path) as f:
            for line in f:
                line = line.rstrip("\n")
                if line.strip() == "[no-summarize-metrics]":
                    if not allow_command:
                        raise ValueError(
                            "[no-summarize-metrics] can only appear at the beginning of the file"
                        )
                    self._should_summarize = False
                    continue
                if line.strip().startswith("#") or line.strip() == "":
                    continue
                if line.endswith(_OPTIONAL_SUFFIX):
                    self.optional_metrics.add(
                        line.removesuffix(_OPTIONAL_SUFFIX)
                    )
                else:
                    self.expected_metrics.add(line)
                allow_command = False

    @property
    def should_summarize(self) -> bool:
        return self._should_summarize

    def check(self, actual_metrics: Set[str]) -> None:
        """Checks that the given `actual_metrics` matches the expected metrics.

        Args:
          actual_metrics: the set of metrics to check.

        Raises:
          ValueError: when the metrics don't match. Includes the diff.
        """
        if self.expected_metrics == actual_metrics - self.optional_metrics:
            return
        union = sorted(
            (actual_metrics | self.expected_metrics) - self.optional_metrics
        )
        lines = []
        for entry in union:
            if entry in actual_metrics:
                if entry not in self.expected_metrics:
                    lines.append("\t+" + entry)
            else:
                lines.append("\t-" + entry)
        diff = "\n".join(lines)

        raise ValueError(
            (
                f"Metric names produced by the test differ from the expectations in "
                f"{self.file_path}:\n\n"
                f"{diff}\n\n"
                "One way to update the expectation file is to run the test locally with this "
                "environment variable set:\n"
                "\tFUCHSIA_EXPECTED_METRIC_NAMES_DEST_DIR="
                "$(pwd)/src/tests/end_to_end/perf/expected_metric_names\n\n"
                "See https://fuchsia.dev/fuchsia-src/development/performance/metric_name_expectations\n"
            )
        )
