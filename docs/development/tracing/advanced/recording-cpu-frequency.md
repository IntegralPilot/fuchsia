# Recording CPU frequency in a trace

## Overview

Fuchsia tracing can capture CPU frequency changes (referred to as "Processing
Rate" in the kernel) on devices that support Dynamic Voltage and Frequency
Scaling (DVFS). These changes appear as counters in the trace data.

Note: Frequency values are only emitted on frequency change. If the frequency
does not change during the trace, no values will appear in the tracks.

These tracks are particularly useful when analyzing performance on devices using
RPPM (Relative Performance Power Management), because frequency changes can
significantly affect execution duration.

## Capture the trace

The Zircon kernel handles CPU frequency tracking, which is part of the
`kernel:power` category. Specify this category when starting the trace:

```posix-terminal
ffx trace start --categories "kernel:power"
```

## Understand CPU frequency counters

After you have captured the trace, you can view and analyze the CPU frequency
data in [Perfetto](https://ui.perfetto.dev/):

1.  Open the trace file in Perfetto.
2.  In the track list, look for the **Processing Rate:CPU:N** counters (where
    *N* is the CPU core index, for example, `Processing Rate:CPU:0`) under the
    kernel process.

The "Processing Rate" counters display values representing the CPU's operating
frequency relative to a normalized scale:

*   A value of **1000** represents the CPU running at its **maximum frequency**
    (100%).
*   Lower values represent lower frequencies (for example, a value of 524
    represents the CPU running at approximately 52.4% of its maximum frequency).

To calculate the actual frequency, multiply this percentage by the maximum
frequency of the CPU. For example, if the maximum frequency is 2.7 GHz, a trace
value of 524 translates to roughly 1.4 GHz (52.4% of 2.7 GHz).

These values are derived from the kernel's internal processing rate multiplier,
scaled by 1000.

## Understand bandwidth demand counters

The `kernel:power` category also includes counters for bandwidth demand, which
are the **Constant BW Demand:CPU:N** counters (where *N* is the CPU core index).

The "Constant BW Demand" counters represent the **Total Deadline Utilization**
(requested CPU bandwidth) for a core. This is the fraction of CPU time requested
by threads with specific timing requirements.

Similar to the processing rate, these counters display values relative to a
normalized scale:

*   A value of **1000** represents the CPU core at **maximum utilization**
    (100%).
*   Lower values represent lower utilization (for example, a value of 750
    represents deadline threads requesting 75% of the core's total capacity).

### Relationship between demand and processing rate

These two counters typically move together:

*   **Constant BW Demand** is the *requirement*: It shows how much work the
    scheduled threads are asking to perform.
*   **Processing Rate** is the *response*: The kernel adjusts the CPU frequency
    based on this demand.

When the bandwidth demand increases, the kernel generally increases the
processing rate (frequency) to ensure the deadlines can be met. When demand
decreases, the processing rate may be lowered.
