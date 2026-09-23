# Zircon scheduler

## Overview

The Zircon scheduler is a hybrid scheduler that supports both **Fair** and
**Deadline** scheduling disciplines. It runs independently on each logical
CPU, managing its own set of run queues and coordinating with other CPUs
via Inter-Processor Interrupts (IPIs).

The primary goal of the scheduler is to share CPU bandwidth among competing
threads while guaranteeing the timing requirements of critical workloads
(such as audio, graphics, and high-frequency sensors).

## Scheduling disciplines

### Fair scheduling

Fair scheduling is the primary scheduling discipline for general-purpose
workloads in the system. It divides CPU bandwidth between competing threads
such that each receives a weighted proportion of the CPU over time.

* **Weight-Based Allocation**: Each thread is assigned a weight. A thread
  with twice the weight of another receives approximately twice the CPU time.
* **Virtual Timeline**: Ready threads are ordered in a balanced binary tree
  (WAVL tree) based on a *virtual finish time*. Higher-weighted threads are
  ordered closer to the front, but lower-weighted threads are guaranteed
  timely execution to prevent starvation and unbounded priority inversion.
* **Scheduling Period**: The scheduling period adapts to the number of
  competing threads. If too many threads compete, the period stretches to
  ensure each gets at least a **Minimum Granularity** slice, improving
  throughput.

For a detailed look at the Fair Scheduling algorithm math and ordering
criteria, see [Fair Scheduler Mechanics][fair-scheduler-mechanics].

### Deadline scheduling

Deadline scheduling is supported for latency-critical tasks that require firm
timing guarantees and must not partake in proportional slowdowns during overload
conditions.

* **Guarantees**: A deadline task specifies a **Capacity** (CPU time) and a
  **Deadline** (period). The scheduler guarantees that a task will receive its
  capacity within its deadline period if the overall deadline demand is
  feasible.
* **Precedence**: Deadline tasks always take precedence over eligible fair
  tasks.
* **Overload Response**: To ensure that all deadline tasks can be scheduled
  within their expected periods, the sum total of deadline demands must not
  exceed the processor's capacity. If the total deadline demand of a processor
  exceeds 100%, stochastic deadline misses are expected and the system will
  behave as though the periods of some or all task were multiplied by the
  overload factor.
* **Critical Priority**: Deadline profiles can optionally be marked as
  **critical**. A critical deadline task receives absolute priority over normal
  deadline tasks during periods of CPU oversubscription, ensuring that essential
  workloads are not degraded by runaway standard deadline threads. In a
  correctly functioning system, total critical utilization should never exceed
  the limits of the processor, otherwise a kernel oops is emitted.

## Run queues and selection

Each CPU maintains three primary run queues, implemented as augmented WAVL trees
to satisfy efficient O(log n) operations:

1. **Critical Deadline Run Queue**: Contains runnable critical deadline tasks.
2. **Deadline Run Queue**: Contains runnable non-critical deadline tasks.
3. **Fair Run Queue**: Contains runnable fair tasks.

### Thread selection

When choosing the next thread to run, the scheduler evaluates queues in the
following order:

1. **Critical Deadline Task**: Picks the eligible critical deadline task with
   the earliest finish time.
2. **Deadline Task**: Picks the eligible deadline task with the earliest finish
   time.
3. **Fair Task**: Picks the eligible fair task with the earliest virtual finish
   time.
4. **Idle/Power Thread**: Runs when no other threads are eligible.

### Preemption and timeslices

The scheduler sets a CPU-local preemption timer based on the selected thread's
timeslice or the arrival of a deadline task. When the timer fires, execution
stops on the current thread, and the scheduler re-evaluates which thread is the
most suitable to run next.

## CPU placement and migration

The scheduler performs load balancing and task placement based on several
factors to optimize performance and thermal efficiency.

### Placement criteria

When a thread wakes up or is unblocked, the scheduler selects a target CPU based
on:

1. **Affinity**: The thread's user-defined CPU affinity mask is prioritized.
2. **Last CPU**: Running on the last CPU helps preserve cache warmness
   (Intra-cluster affinity).
3. **Idle States**: Highly prioritized if the core avoids context-switch
   overhead of a busy CPU.

### Work stealing

To maintain load balance, a CPU with no eligible work will attempt to **steal**
work from other busy CPUs.

* **Cluster Awareness**: The scheduler is conscious of the CPU topology. CPU
  stealing prioritizes local clusters over distant clusters to capitalize on
  better shared cache performance levels and reduce migration penalties.

## Power and energy awareness

The scheduler uses local energy models and power level controls to optimize
power consumption and thermal performance.

* **Performance Scales**: Account for individual CPU performance multipliers
  inside budget calculations.
* **Dynamic Voltage and Frequency Scaling (DVFS)**: Adjust processor workloads
  supporting user-defined limits while striving to meet deadline schedules.

<!-- Reference links -->
[fair-scheduler-mechanics]: fair_scheduler.md
