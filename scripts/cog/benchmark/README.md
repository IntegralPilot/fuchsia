# Benchmark Suite

This document describes how to use the benchmark suite to run and analyze benchmarks.

## Running Benchmarks

To run all benchmarks, execute the `run-benchmarks` script:

```bash
./run-benchmarks
```

To run one or more specific benchmarks, provide each benchmark name with its own flag:

```bash
./run-benchmarks --benchmark <benchmark_name_1> --benchmark <benchmark_name_2>
```

## Creating a Benchmark

To create a new benchmark, create a new Python file in the `benchmark` directory and define a class that inherits from `Benchmark`.

```python
from base import Benchmark

class MyBenchmark(Benchmark):
    def __init__(self):
        super().__init__(
            name="my_benchmark",
            description="A description of my benchmark.",
            expected_to_pass=True,
            compare=["other_benchmark"]
        )

    def run(self):
        # Benchmark code goes here.
        pass
```

## Understanding the Output

The script will print a report with the following information for each benchmark:

*   **Name**: The name of the benchmark.
*   **Description**: The description of the benchmark.
*   **Time taken**: The time it took to run the benchmark.
*   **Pass/Fail status**: Whether the benchmark passed or failed.
*   **Expected**: Whether the result was expected.
