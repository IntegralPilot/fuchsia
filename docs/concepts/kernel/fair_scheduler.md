# Zircon fair scheduler

## Introduction

The Zircon fair scheduler is the primary scheduling discipline for general
workloads in the system. It divides CPU bandwidth between competing threads,
such that each receives a weighted proportion of the CPU over time.

For a high-level overview of the scheduler architecture, including Deadline
scheduling and power management, see
[Zircon Scheduler Overview][kernel-scheduling-overview].

## Fair scheduling overview

Fair scheduling is a discipline that divides CPU bandwidth between competing
threads, such that each receives a weighted proportion of the CPU over time.
In this discipline, each thread is assigned a weight, which is somewhat
similar to a priority in other scheduling disciplines. Threads receive CPU
time in proportion to their weight, relative to the weights of other
competing threads. This proportional bandwidth distribution has useful
properties that make fair scheduling a good choice as the primary scheduling
discipline in a general purpose operating system.

Briefly, these properties are:

* **Intuitive bandwidth allocation mechanism**: A thread with twice the weight
  of another thread will receive approximately twice the CPU time, relative to
  the other thread over time. Whereas, a thread with the same weight as another
  will receive approximately the same CPU time, relative to the other thread
  over time.
* **Starvation free for all threads**: Proportional bandwidth division ensures
  that all competing threads receive CPU time in a timely manner, regardless of
  how low the thread weight is relative to other threads. Notably, this property
  prevents unbounded priority inversion.
* **Fair response to system overload**: When the system is overloaded, all
  threads share proportionally in the slowdown. Solving overload conditions is
  often simpler than managing complex priority interactions required in other
  scheduling disciplines.
* **Stability under evolving demands**: Adapts well to a wide range of workloads
  with minimal intervention compared to other scheduling disciplines.

Note: Deadline scheduling is supported in Zircon for specialized,
latency-critical tasks that require rigid timing guarantees (e.g., low-latency
audio, high-frequency sensors). Deadline tasks take precedence over fair tasks
and operate on guaranteed capacity budgets.

## Fair scheduling in Zircon

The Zircon fair scheduler is based primarily on the Weighted Fair Queuing (WFQ)
discipline, with insights from other similar queuing and scheduling disciplines.

The following subsections outline the algorithm as implemented in Zircon. From
here on, "fair scheduler" and "Zircon fair scheduler" are used interchangeably.

### Ordering thread execution

One of the primary jobs of the scheduler is to decide which order to execute
competing threads on the CPU. The fair scheduler makes these decisions
separately on each CPU. Essentially, each CPU runs a separate instance of the
scheduler and manages its own run queue.

In this approach, a thread may compete only on one CPU at a time. A thread can
be in one of three states: _ready_, _running_ or _blocked_ (other states are not
relevant to this discussion.) For each CPU, at most one thread is in the
_running_ state at any time: this thread executes on the CPU, all other
competing threads await execution in the _ready_ state, while blocked threads
are not in competition. The threads in the _ready_ state are enqueued in the
CPU's run queue; the order of threads in the run queue determines which thread
runs next.

The fair scheduler, unlike **O(1)** scheduling disciplines such as priority
round-robin (RR), uses an ordering criteria to compare and order threads in the
run queue. This is implemented using a balanced binary tree, and means that
scheduling decisions generally cost **O(log n)** to perform. While this is more
expensive than an **O(1)** scheduler, the result is a near-optimal worst case
delay bound (queuing time) for all competing threads.

### Ordering criteria

Two concepts are used to order threads in the run queue: _virtual timeline_ and
per-thread _normalized rate_. The _virtual timeline_ tracks when each thread in
the run queue would finish a _normalized time slice_ if it ran to completion.
A _normalized time slice_ is proportional to the thread's _normalized rate_,
which in turn is inversely proportional to the thread's weight. Threads are
ordered in the run queue by ascending _finish time_ in the _virtual timeline_.

The inverse proportional relationship to weight causes higher weighed threads to
be inserted closer to the front of the run queue than lower weighted threads
with similar arrival times. However, this is bounded over time: the longer a
thread waits in the run queue, the less likely a newly arriving thread, however
highly weighted, will be inserted before it. This property is key to the
fairness of the scheduler.

### Yield

Yielding immediately expires the thread's time slice and returns it to the run
queue. This behavior is similar to yielding in **O(1)** scheduling: the yielding
thread is guaranteed to queue behind threads of the same or greater weight.
However, the yielding thread may or may not skip ahead of lower weight threads,
depending on how long other threads have been waiting to run.

<!-- Reference links -->
[kernel-scheduling-overview]: kernel_scheduling.md
