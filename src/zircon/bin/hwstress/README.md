# `hwstress`

`hwstress` is a tool for exercising hardware components, such as CPU, RAM. It can be used both to
test that hardware is correctly functioning (revealing bad RAM, or system heating problems), and
also as a system load generator (for example, running the CPU at 50% utilization and consuming 50%
of RAM).

## Usage

```
hwstress <subcommand> [options]

Attempts to stress hardware components by placing them under high load.

Subcommands:
  cpu                    Perform a CPU stress test.
  light                  Perform a device light / LED stress test.
  memory                 Perform a RAM stress test.

Global options:
  -d, --duration=<secs>  Test duration in seconds. A value of "0" (the default)
                         indicates to continue testing until stopped.
  -l, --logging-level    Level of logging to show: terse, normal (the default)
                         or verbose.
  -h, --help             Show this help.

CPU test options:
  -u, --utilization=<percent>
                         Percent of system CPU to use. A value of
                         100 (the default) indicates that all the
                         CPU should be used, while 50 would indicate
                         to use 50% of CPU. Must be strictly greater
                         than 0, and no more than 100.
  -w, --workload=<name>  Run a specific CPU workload. The full list
                         can be determined by using "--workload=list".
                         If not specified, each of the internal
                         workloads will be iterated through repeatedly.
  -p, --cpu-cores=<cores>
                         CPU cores to run the test on. A comma separated list
                         of CPU indices. If not specified all the CPUs will be
                         tested.

Light test options:
  --light-on-time=<seconds>
                         Time in seconds each "on" blink should be.
                         Defaults to 0.5.
  --light-off-time=<seconds>
                         Time in seconds each "off" blink should be.
                         Defaults to 0.5.

Memory test options:
  -m, --memory=<size>    Amount of RAM to test, in megabytes.
  --percent-memory=<percent>
                         Percentage of total system RAM to test.
```

## Test details

### CPU

The CPU stress test will run various workloads across all the CPUs in the
system. The workloads are a mix of integer and floating point arithmetic, and
have basic checks that performed calculations were correct.

### RAM

The RAM stress tests attempt to:

*   Find bad bits in RAM by writing patterns (both random data and deterministic
    patterns) and verifying that they can be correctly read back again.

*   Find RAM affected by the [Row hammer][rowhammer] vulnerability.

[rowhammer]: https://en.wikipedia.org/wiki/Row_hammer

## See also

*   `loadgen` (`src/zircon/bin/loadgen`) which has many threads performing
    cycles of idle / CPU work, useful for exercising the kernel's scheduler.

*   `kstress` (`src/zircon/bin/kstress`) which is designed to stress test the
    kernel itself.
