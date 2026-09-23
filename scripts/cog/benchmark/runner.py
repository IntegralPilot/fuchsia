# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import importlib
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List

from base import Benchmark


@dataclass
class BenchmarkResult:
    """Holds the result of a single benchmark run.

    Attributes:
        name: The name of the benchmark.
        description: The description of the benchmark.
        time_taken: The time taken to run the benchmark in seconds.
        passed: Whether the benchmark passed.
        expected: Whether the result was expected.
        compare: A list of other benchmark names to compare against.
    """

    name: str
    description: str
    time_taken: float
    passed: bool
    expected: bool
    compare: List[str]


class BenchmarkResults:
    """Holds a collection of benchmark results."""

    def __init__(self) -> None:
        self.results: List[BenchmarkResult] = []
        self._results_by_name: Dict[str, BenchmarkResult] = {}

    def add(self, result: BenchmarkResult) -> None:
        """Adds a benchmark result.

        Args:
            result: The benchmark result to add.
        """
        self.results.append(result)
        self._results_by_name[result.name] = result

    def print_reports(self) -> None:
        """Prints a report for each benchmark."""
        if not self.results:
            print("No benchmark results to display.")
            return

        for i, result in enumerate(self.results):
            if i > 0:
                print("")  # Add a newline between reports

            print(f"Benchmark: {result.name}")
            print(f"Description: {result.description}")

            if not result.passed:
                if result.expected:
                    print("This benchmark failed to run which was expected")
                else:
                    print("This benchmark failed to run which was not expected")
            else:
                minutes = int(result.time_taken // 60)
                seconds = result.time_taken % 60
                print(
                    f"Execution time in seconds: {result.time_taken:.1f} seconds"
                )
                print(
                    f"Execution time: {minutes} minutes {seconds:.1f} seconds"
                )

            print("Comparisons:")
            if not result.passed:
                print("Unable to run comparisons since this failed")
                continue

            if not result.compare:
                print("<None>")
                continue

            for other_name in result.compare:
                other_result = self._results_by_name.get(other_name)
                if not other_result:
                    print(
                        f"Compared to {other_name}: Unable to compare, benchmark not found."
                    )
                    continue

                if not other_result.passed:
                    print(
                        f"Compared to {other_name}: Unable to compare since {other_name} failed to execute"
                    )
                    continue

                time_diff = result.time_taken - other_result.time_taken
                time_diff_val = int(round(abs(time_diff)))
                if time_diff == 0:
                    comparison_text = "took the same amount of time"
                elif time_diff > 0:
                    comparison_text = f"{time_diff_val} seconds slower"
                else:
                    comparison_text = f"{time_diff_val} seconds faster"
                print(f"Compared to {other_name}: {comparison_text}")


def find_benchmarks() -> List[Benchmark]:
    """Finds all defined benchmarks by importing all .py files in this dir.

    Returns:
        A list of all found benchmark instances.
    """
    benchmarks: List[Benchmark] = []
    benchmark_dir = Path(__file__).parent
    for f in benchmark_dir.iterdir():
        if (
            f.is_file()
            and f.suffix == ".py"
            and f.name
            not in (
                "runner.py",
                "base.py",
                "__init__.py",
            )
        ):
            module_name = f.stem
            importlib.import_module(module_name)

    for subclass in Benchmark.__subclasses__():
        benchmarks.append(subclass())
    return benchmarks


def run_benchmarks(benchmarks: List[Benchmark]) -> BenchmarkResults:
    """Runs a set of benchmarks and returns the results.

    Args:
        benchmarks: A list of benchmarks to run.

    Returns:
        The results of the benchmark runs.
    """
    print("Running benchmarks...")
    results = BenchmarkResults()
    for benchmark in benchmarks:
        print(f" * Running benchmark: {benchmark.name} ...")
        start_time = time.time()
        try:
            benchmark.setup()
            start_time = time.time()
            benchmark.run()
            end_time = time.time()
            passed = True
        except Exception as e:
            end_time = time.time()
            passed = False
            print(f"Benchmark {benchmark.name} failed with exception: {e}")
        finally:
            benchmark.cleanup()

        time_taken = end_time - start_time
        expected = benchmark.expected_to_pass == passed

        results.add(
            BenchmarkResult(
                name=benchmark.name,
                description=benchmark.description,
                time_taken=time_taken,
                passed=passed,
                expected=expected,
                compare=benchmark.compare,
            )
        )
    return results


def main() -> None:
    """Runs the benchmarking suite."""
    parser = argparse.ArgumentParser(description="Run benchmarks.")
    parser.add_argument(
        "--benchmark",
        action="append",
        type=str,
        help="The name of a benchmark to run. Can be specified multiple times.",
    )
    args = parser.parse_args()

    benchmarks = find_benchmarks()

    if args.benchmark:
        benchmarks = [b for b in benchmarks if b.name in args.benchmark]

    results = run_benchmarks(benchmarks)
    print("\n--- Benchmark Results ---\n")
    results.print_reports()
    print("\n")


if __name__ == "__main__":
    main()
