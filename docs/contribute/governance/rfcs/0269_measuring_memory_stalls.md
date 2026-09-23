<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0269" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Zircon does not currently expose information about transient stalls caused by
memory pressure, which Starnix needs in order to implement the
[PSI][linux-psi-doc] interface (`/proc/pressure/memory`).

This RFC addresses the issue of synthesizing compatible measurements and
notifying observers when a given threshold is crossed.

## Summary

This document proposes a mechanism to detect stalls caused by memory pressure
and notify userspace. The mechanism will be based on measuring time spent in
pressure-induced activities that could have otherwise been spent doing useful
work.

## Motivation

Linux-based operating systems (both [Android][android-lmkd] and
[systemd][systemd-oomd]) rely on a designated userspace daemon to be notified
and act on delays ("stalls") induced by memory pressure. The action performed by
such programs is usually just killing entire processes/services in response.

In the context of Starnix, whose goal is to implement the Linux API *as she is
spoke* ([RFC-0082]), we need to offer the same stall detection API, which is
called [Pressure Stall Information (PSI)][linux-psi-doc] on Linux.

Linux's PSI quantifies pressure-induced memory stalls by timing the delays
caused by the additional actions that are necessary to free up memory, and those
stats can be accessed through the `/proc/pressure/memory` virtual file. This RFC
proposes the approach of measuring pressure-induced delays similarly to Linux.

This document's focus is only on Starnix and how to implement Linux's PSI
interface in order to reach feature parity. Whether other Fuchsia components,
outside of Starnix, might be able to use it, too, is out of its scope and left
for a future phase.

## Stakeholders

_Facilitator:_ lindkvist@google.com

_Reviewers:_

- adanis@google.com
- eieio@google.com
- maniscalco@google.com
- rashaeqbal@google.com

_Consulted:_

- davemoore@google.com
- lindkvist@google.com
- wez@google.com

_Socialization:_

This RFC has been discussed with the Zircon team over email, chat and meetings.

## Requirements

Given the focus on Starnix, the requirements are driven by the interface to be
offered to Linux programs.

On Linux, reading the `/proc/pressure/memory` file returns the following
information:

* Two monotonic timers since boot, labeled `some` and `full` (whose unit is
  microseconds): according to the official documentation, the `some` time grows
  when at least one thread is memory-stalled, and the `full` time grows when all
  otherwise-runnable threads are memory-stalled. Note that the `some` condition
  can occur even if some other thread is able to make progress, whereas the
  `full` condition implies that no thread at all is making progress. As a
  consequence, the `some` time is always greater or equal to the `full` time.
* The averages of the above two timers' growth rates (as a ratio of their
  variations over the monotonic clock, thus giving a range of values between
  0.00 and 1.00) over the last 10, 60 and 300 seconds.

In addition to reading those stats, Linux makes it also possible to be notified
when a certain stall level is reached by opening that file, writing which timer
to observe (either `some` or `full`), a threshold and an observation window
(both in microseconds), and then polling that file descriptor. Poll will then
return a `POLLPRI` event when the selected timer has grown by more than the
given threshold over the configured observation window.

It's worth noting that, on Linux, the same measuring and notification mechanism
can also be enabled at cgroup-level by means of the `memory.pressure` file,
which offers the same interface. While the design proposed in this RFC is only
concerned with implementing the global `/proc/pressure/memory` file at this
time, the internal measurement mechanisms described in this RFC can be reused to
implement a similar feature, too, should the need arise (see
[Drawbacks, alternatives, and unknowns](#drawbacks_alternatives_and_unknowns)
for more details).

For the sake of minimizing divergences in the magnitude of the measured memory
stalls in comparison to Linux, we intend to initially adapt the same formulas to
Zircon, and leave further adjustments/deviations for a later tuning phase. The
proposed adaptation of those formulas for Zircon is detailed in the following
section.

## Design

The goal of this RFC is to precisely define the notion of a stall in Zircon, how
stall measurements are generated and aggregated, how to expose this data to
userspace and how to let userspace be notified when the system stalls more than
a given level.

In short, Zircon will measure memory stalls, aggregate them and expose two
system-wide values (as a new [`zx_object_get_info`][zx_object_get_info] topic)
and a notification interface (through a new syscall), both gated by a new
resource type. These two new values (called `some` and `full` stall time) will
be expressed in nanoseconds, and they will grow monotonically and continuously,
at a fraction (between zero and one) of the rate of the monotonic clock that
depends on the current stall level.

As an example, let's suppose that, after being up for 300000 nanoseconds without
experiencing any stall, the system detects a 25% stall for 100000 nanoseconds
and then a 50% stall for 200000 extra nanoseconds. The corresponding reported
value will be `0% * 300000 + 25% * 100000 + 50% * 200000 = 125000` nanoseconds.

### Identifying memory stalls

In principle, any kind of delay induced by memory pressure that prevents a
thread from otherwise doing useful work is to be regarded as a stall for that
thread.

In practice, given the current codebase, only time spent while waiting for
memory to be freed by the dedicated kernel thread (or, in other words, time
spent while `AnonymousPageRequester::WaitOnRequest` exists on the call stack)
will be regarded as a memory stall. This happens when a new memory allocation is
attempted but the amount of free pages is currently below the *delay threshold*
(which is close to the Out Of Memory level and computed
[here](https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/kernel/object/memory_watchdog.cc;drc=98c3a0e7d8446eb24acbca592258fc6207211c73;l=349-352)).

The above operations involve tracking the stalling thread through many different
scheduler states. Furthermore, the stall tracking mechanism aims at being
generic enough to make it easy to mark other code sections as memory stalls in
the future. In order to avoid making the current scheduler more complex by
adding new _ad hoc_ states, this RFC proposes to delimit sections of code to be
regarded as a stall with a guard object (`ScopedMemoryStall`) and classify
threads for the purpose of tracking stalls as either `IGNORED`, `PROGRESSING` or
`STALLING`, according to both their current scheduler state and whether they are
executing in a guarded region or not.

| Scheduler state | If inside guard | If outside guard |
|-|-|-|
| `INITIAL` | n/a | `IGNORED` |
| `READY` | `STALLING` | `IGNORED` |
| `RUNNING` | `STALLING` | `PROGRESSING` |
| `BLOCKED` and `THREAD_BLOCKED_READ_LOCK` | `STALLING` | `IGNORED` |
| `SLEEPING` | `STALLING` | `IGNORED` |
| `SUSPENDED` | `STALLING` | `IGNORED` |
| `DEATH` | n/a | `IGNORED` |

In order to eliminate noise in measurements resulting from internal memory
book-keeping activities in the kernel, non-usermode threads will always be
regarded as `IGNORED`.

It's worth noting that, in the above model, threads are considered as `STALLING`
if they are either running inside a guard, or were running inside a guard before
blocking or being preempted.

### Growth rate of the `some` and `full` timers

The stall timers will not be merely the sum of each thread's `STALLING` time.
Rather, they will be the result of a continuous evaluation of the overall number
of `STALLING` and `PROGRESSING` threads (performed _locally_ per-CPU) and then
aggregated periodically. This will match the Linux model as documented by
[this reference][linux-psi-model]. Internally, two levels of measurements will
be added to Zircon.

The first level, local to each CPU and tracked by per-CPU `StallAccumulator`
objects, will measure time spent in each of the following three
non-mutually-exclusive conditions (using the monotonic clock as the time base):

* `full`: At least one thread tied to this CPU is in `STALLING` state and none
  is `PROGRESSING`.
* `some`: At least one thread tied to this CPU is in `STALLING` state.
* `active`: At least one `PROGRESSING` or `STALLING` thread is tied to this CPU.

Note: For the purpose of stall tracking, non-running threads will stay tied to
the last CPU they ran on.

The second level of measurements, implemented by the `StallAggregator`
singleton, will update the stats at system level, by periodically querying
all `StallAccumulator`s. In particular, it will run a background thread that
periodically wakes up and averages the `some` and `full` stall time increments
observed by each `StallAccumulator`s, weighted by the respective `active` time,
and increment the global `some` and `full` values accordingly.

In addition, the `StallAggregator` will notify registered watchers when the
resulting growth rate exceeds the threshold that they set (see
[The new `zx_system_watch_memory_stall` syscall](#the_new_zx_system_watch_memory_stall_syscall)
for details).

### The Stall resource

A new type of kernel resource, called `ZX_RSRC_SYSTEM_STALL_BASE`, will be
introduced.

Since this RFC is only concerned with measuring the global stall level (the
equivalent of `/proc/pressure/memory`), it is enough to use a single resource
to control access to global stall measurements.

A handle to this resource will be created at boot and served by component
manager through the `fuchsia.kernel.StallResource` protocol (as a
*built-in capability*), which will simply return duplicated handles to it.
Controlling who this capability is routed to will control who has access to
stall measurements.

### The new `ZX_INFO_MEMORY_STALL` topic

A new `ZX_INFO_MEMORY_STALL` topic (on the stall resource) will be added,
exposing the two following fields:

```c
typedef struct zx_info_memory_stall_t {
    // Total monotonic time spent with at least one memory-stalled thread.
    zx_duration_mono_t stalled_time_some;

    // Total monotonic time spent with all threads memory-stalled.
    zx_duration_mono_t stalled_time_full;
} zx_info_memory_stall_t;
```

This topic will be queried by Starnix to synthesize the `/proc/pressure/memory`
file. In addition, Starnix will also sample it at regular intervals to
internally compute the rolling averages over the last 10, 60 and 300 seconds.

### The new `zx_system_watch_memory_stall` syscall

Similarly to the existing [`zx_system_get_event`][zx_system_get_event] syscall,
the new `zx_system_watch_memory_stall` syscall will also return an event handle,
on which the kernel will assert/deassert `ZX_EVENT_SIGNALED` according to the
current growth rate of the selected stall timer.

The full syscall prototype will be:

```c
zx_status_t zx_system_watch_memory_stall(zx_handle_t stall_resource,
                                         zx_system_memory_stall_type_t kind,
                                         zx_duration_mono_t threshold,
                                         zx_duration_mono_t window,
                                         zx_handle_t* event);
```

Arguments:

* `stall_resource`: A handle to the stall resource.
* `kind`: Either `ZX_SYSTEM_MEMORY_STALL_SOME` or `ZX_SYSTEM_MEMORY_STALL_FULL`,
   to select the timer to be observed.
* `threshold`: Minimum stall time that will trigger the signal (nanoseconds).
* `window`: Duration of the observation window (nanoseconds).
* `event`: Filled by the kernel in return, a handle to an event that will be
  asserted (`ZX_EVENT_SIGNALED`) if, in the course of the last observation
  `window`, the stall timer selected by `kind` has increased by at least
  `threshold`.

The returned event will stay asserted for as long as the trigger condition
applies, and it will be deasserted by the kernel when it no longer holds.

Possible errors:

* `ZX_ERR_BAD_HANDLE`: `stall_resource` is not a valid handle.
* `ZX_ERR_WRONG_TYPE`: `stall_resource`'s kind is not `ZX_RSRC_KIND_SYSTEM`.
* `ZX_ERR_OUT_OF_RANGE`: `stall_resource` is not the stall resource.
* `ZX_ERR_INVALID_ARGS`: invalid `kind`, `threshold` or `window` (see note
  below).

The caller's job policy must allow `ZX_POL_NEW_EVENT`.

Note that even if the API defines `threshold` and `window` in terms of
nanoseconds, Zircon will be allowed to not use the full precision of the
requested values. Furthermore, it must hold that `0 < threshold <= window` and,
in order to limit the amount of necessary book-keeping memory, we also impose
that `window` cannot be longer than 10 seconds (Linux puts the same constraint).

## Implementation

The Zircon changes will be split in several CLs, roughly in this order:

* Add infrastructure to detect stalls at per-CPU level (`StallAccumulator`).
* Make time spent blocked in `AnonymousPageRequest` count as a memory stall.
* Periodically aggregate all per-CPU values into global metrics
  (`StallAggregator`) in a low-priority kernel thread.
* Serve the new stall resource as a component manager built-in capability
  (`fuchsia.kernel.StallResource`).
* Implement the new `ZX_INFO_MEMORY_STALL` topic.
* Add infrastructure to detect when a stall threshold is crossed.
* Expose it through the `zx_system_watch_memory_stall` syscall.

Once the Zircon changes are made, Starnix will be modified to implement
`/proc/pressure/memory` on top of them as follows:

* the total `some` and `full` values will be directly read from
  `ZX_INFO_MEMORY_STALL`.
* the averages will be computed within Starnix by polling
  `ZX_INFO_MEMORY_STALL` (approximately) every second, keeping the last 300
  samples in a circular queue and computing the rolling averages from it.
* PSI notifications will be implemented on top of
  `zx_system_watch_memory_stall`, with the addition of a rate-limiter
  (implemented within Starnix itself) to match Linux's PSI behavior of only
  delivering up to one notification per window.

## Performance

As described in the previous sections, the proposed implementation will maintain
stall measurements in per-CPU data structures and periodically aggregate them
from a dedicated kernel thread. Performance testing has not found any
appreciable regression due to stall time book-keeping.

A possible performance risk might derive from having too many subscribed
`zx_system_watch_memory_stall` observers to be maintained and notified by the
Zircon kernel when the stall thresholds are crossed. If needed, it will be
possible to mitigate this by proxying access through a dedicated user-space
component that will aggregate and serve multiple clients through a single
Zircon subscription. Similarly to Zircon (see
[The new `zx_system_watch_memory_stall` syscall](#the_new_zx_system_watch_memory_stall_syscall)
above), such a proxy would be allowed to drop precision from the requested
values in order to maximize aggregation opportunities.

## Ergonomics

The proposed Zircon API makes implementing PSI in Starnix almost entirely
straightforward, with the exception of Linux's rate-limiting of notifications.
We could, in theory, implement rate-limiting in the Zircon kernel too and end up
with a strobe signaling scheme similar to the one described in [RFC-0237].
However, in order to make `zx_system_watch_memory_stall`'s behavior easier to
describe, as well as for consistency with the existing
[`zx_system_get_event`][zx_system_get_event] signals, it has been chosen to just
expose a simple level-triggered signal, and delegate the complexity of
rate-limiting notifications to user space.

## Backwards Compatibility

This RFC only introduces new interfaces (a new resource type, the
`ZX_INFO_MEMORY_STALL` topic and the `zx_system_watch_memory_stall` syscall)
that do not bring any breaking API/ABI change.

## Security considerations

Giving user space access to new performance measurements opens the possibility
for creating unwanted side channels. However, in this case, the risk is
mitigated by the fact that the capability to access the new measurements will
only be routed to either trusted Fuchsia components, or to trusted Linux
processes within Starnix (according to the Starnix container's security model).

The risk of unbounded kernel memory allocations is eliminated by giving the
kernel the freedom to drop precision from the requested threshold and window
values (enabling internal quantization of stats), and by limiting the range of
permissible values.

## Privacy considerations

This change should have no impact on privacy.

## Testing

Two kinds of Zircon tests will be added:

* Userspace tests (`core-tests`) that will watch memory stall events,
  generate memory pressure, query the info topic and verify that the events are
  actually triggered.
* In-kernel unit tests that will simulate the effects of `ScopedMemoryStall`
  guards and verify that the measured timings match the expectations.

## Documentation

Documentation will be added next to the relevant syscall surface (the new
`ZX_INFO_MEMORY_STALL` topic and the new `zx_system_watch_memory_stall` syscall)
as well as the `fuchsia.kernel.StallResource` FIDL protocol.

The list of conditions that are considered to be a stall will be regarded as an
implementation detail and will not be included in the documentation, to leave
the possibility of further expansion and tuning of such conditions without
breaking the API.

## Drawbacks, alternatives, and unknowns

### Syscall surface

The design in this RFC is closely related to implementing global PSI in Starnix,
and the syscall API is modeled after its requirements.

The option to expose the new features through a new "stall gauge" kernel object
type (instead of a new resource type) was considered, as a way to ease the
future implementation of cgroups' `memory.pressure` hierarchical stats. However,
given that at this time there is no need for such an extension, it was decided
to keep the API changes simple and avoid overplanning.

Instead of making `zx_system_watch_memory_stall` syscall return an event
instance, it was also considered to return a new specialized "stall monitor"
object type with dedicated signals (e.g. `ZX_STALL_MONITOR_ABOVE_THRESHOLD`).
While more explicit in the intent and in the correspondence to the internal
kernel implementation, this approach would have required a new kernel object
type for no other reason than this. It was rejected because the API of the
existing event object type has been deemed expressive enough.

Lastly, many new API names refer to "stalls" instead of "memory stalls" to leave
room for future expansion to other types of stall measurements too (such as CPU
and I/O stalls).

### Stall measurement and aggregation

Within the proposed measurement scheme, the option to transitively consider
threads waiting on resources held by other stalling threads as stalling
themeselves ("stall inheritance") was discussed but rejected, at least for an
initial implementation, due to the extra complexity and the lack of baseline
data to evaluate its benefits.

Time spent while decompressing [ZRAM][RFC-0219] was initially planned to be
regarded as a memory stall too, as a sign of thrashing (due to trying to
re-access an anonymous page that had been compressed in the past). However, due
to factors such as 1) Zircon proactively compressing inactive pages in the
background even if there is plenty of free memory and 2) decompression only
happening when pages are needed again, this kind of event only signalled a
delayed indication, at best, or a spurious notification, in the worst case.
Furthermore, it was experimentally found that the stall time contributions
originating from ZRAM decompression are currently several order of magnitude
lower than those deriving from `AnonymousPageRequester::WaitOnRequest`. For the
above reasons, it was decided to not consider ZRAM decompressions as memory
stalls for now and leave them out of the initial implementation.

A different aggregation scheme was also considered, in which the kernel exposes
raw stall time counters for each thread through shared memory in a lockless data
structure. It would then be up to user space to periodically perform the
aggregation with product-specific logic, decoupling the aggregation policy from
the kernel API. The drawbacks of this approach would be worse precision, due to
its sampling nature, a more complex design relying on shared memory, where it's
easy to get subtle details wrong, and the need to activate user space much more
frequently.

A simpler way of measuring stalls was considered too: just measuring time spent
with the evictor running. However, this approach would lend itself neither to
distinguishing between `some` and `full` stall times nor to identifying the
process that was affected by the stall.

### Relation to existing memory pressure events

Zircon already exposes information on the current memory pressure level through
five mutually-exclusive events (Normal, Warning, Critical, Imminent Out Of
Memory and Out Of Memory), that are asserted by the kernel and whose handles can
be obtained via the [`zx_system_get_event`][zx_system_get_event] syscall. While
the exact conditions that trigger them are not stated explicitly in Zircon's API
contract, in practice the trigger condition implemented in Zircon consists in
matching the current amount of free memory to one of the five corresponding
ranges.

As an alternative to this RFC's proposal of timing the duration of stalls, the
option to synthesize fake delays from the existing pressure events has been
evaluated too. However, it has been rejected on the basis that the expected
dynamics of Linux's PSI signal are very different: while the transitions between
Zircon's existing pressure events are slow (and, in some cases, subject to
artificial delays to avoid too frequent changes), Linux's PSI timers are
expected to update in near real-time. The growth rate of the PSI timers, in
particular, needs to change fast enough to immediately generate an impulse to
trigger registered watchers when the respective threshold is exceeded, and then
immediately stop triggering them as soon as it falls back below.

In addition, with Linux's PSI timers, the growth rate of an idle system should
be zero. With the current Zircon events, instead, there is no guarantee that the
actions that userspace is asked to take in response will eventually free enough
memory to go back to the Normal state.

### Conclusion

Preliminary tests on an Android image showed that measuring stalls as described
in this RFC generated notification events comparable to those generated by
corresponding workloads on Linux. However, by virtue of Zircon and Linux being
two different kernels with very different VMM subsystems, this is not granted
for all the edge cases, and refinements might be needed in the future, e.g. by
expanding/modifying the definition of what Zircon considers to be a stall
condition.

## Prior art and references

* [Android's LMKD][android-lmkd]
* [systemd-oomd]
* [Linux PSI documentation][linux-psi-doc]
* [Linux Kernel's Pressure Stall Info (PSI) model][linux-psi-model]

<!-- Links -->

[RFC-0082]: 0082_starnix.md
[RFC-0237]: 0237_signalling_clock_updates_with_zx_clock_updated.md
[RFC-0219]: 0219_zircon_page_compression.md
[android-lmkd]: https://source.android.com/docs/core/perf/lmkd
[systemd-oomd]: https://www.freedesktop.org/software/systemd/man/253/systemd-oomd.service.html
[linux-psi-doc]: https://docs.kernel.org/accounting/psi.html
[linux-psi-model]: https://gitlab.com/gitlab-com/gl-infra/scalability/-/issues/1825#kernels-pressure-stall-info-psi-model
[zx_system_get_event]: https://fuchsia.dev/reference/syscalls/system_get_event
[zx_object_get_info]: https://fuchsia.dev/reference/syscalls/object_get_info
