<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0281" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Problem Statement

Various developer tooling requires access to exception channels provided by
Zircon in order to monitor jobs, processes, and threads (collectively "tasks")
for exceptions that may arise during execution. Such tooling today may claim any
of the Zircon Task object's exception channels in order to achieve this. This is
not always practical, however. For instance, when monitoring many sibling
processes that reside within the same parent job, the tool must claim the
exception channel of each individual process. This operation becomes
prohibitively expensive at large scales such as automated testing infrastructure
or monitoring a fully featured Starnix container. Instead, the tooling would
prefer to be able to use the Job's exception channel to monitor all processes
under that job. This is impossible today due to the exclusivity of the Job
exception channel, and the lack of exception delivery to the Job Debugger
exception channel.

## Summary

This RFC proposes changes to the exception delivery mechanism of Zircon so that
the Job Debugger exception channel is included in the delivery path, while
maintaining the ability for multiple userspace entities to be bound to the same
Job. This brings the Job Debugger exception channel into line with the design of
the generalized exception delivery framework within the Zircon task hierarchy
and allows tooling to take advantage of the ability to guarantee the ability of
monitoring many processes simultaneously and efficiently.

### Motivation

#### Exception Propagation {#exception-propagation}

Exceptions that originate in a Zircon thread follow a precise walk order of
delivery to userspace for handling. Handlers are registered via
`zx_task_create_exception_channel` on a Task handle, and come in two flavors:

1. Normal - referred to as "exception channels".
2. Debugger - referred to as "**debugger** exception channels".

When registering themselves with Zircon, a handler must specify whether or not
they are a "Normal" or "Debugger" exception handler, via options to
`zx_task_create_exception_channel`. There is no semantic difference between the
two types of channels. When receiving an exception, handlers are expected to set
the `ZX_PROP_EXCEPTION_STATE` property on the `zx::exception` object to an
appropriate value before releasing their handle to the exception to mark their
handling as "complete". Handlers may mark an exception as
`ZX_EXCEPTION_STATE_HANDLED`, which will terminate the walk and continue the
thread, `ZX_EXCEPTION_STATE_TRY_NEXT` to send the exception to the next handler,
or `ZX_EXCEPTION_STATE_THREAD_EXIT` to immediately terminate the thread (and
therefore the walk).

There are two primary difference between the types of exception channels. The
first difference comes from the **walk order** defined by Zircon. The walk order
determines which exception channels get to see the exception in what order. For
Architectural and Policy exceptions, the walk order is defined to be:

| Step | Channel | Delivery Type |
| :---- | :---- | :---- |
| 1 | Process Debugger | First-chance |
| 2 | Thread | First-chance |
| 3 | Process | First-chance |
| 4 | Process Debugger | Second-chance |
| 5 | Job | First-chance |
| 6 | Parent Job | First-chance |
| 7 | Grandparent Job | First-chance |
| ... | Up the job tree, until the root job is reached | |

When an exception is generated, Zircon sends the exception first the Process
Debugger exception channel, then waits until the handler either closes
their exception channel, or closes their handle to the exception. If the
exception remains unhandled, it will be passed up to the next handler in the
walk order, again with Zircon waiting until the handle is closed before moving
on to the next. Each of these exception channels is permitted exactly one
handler. Another handler attempting to call `zx_task_create_exception_channel`
on the same task will be returned `ZX_ERR_ALREADY_BOUND` and will not receive
any exceptions.

The second difference is which exception types are sent to which channels. In
general, there are two types of exceptions defined by Zircon:

1. Architectural - e.g. segmentation faults, page faults, or undefined
   instructions.
2. Synthetic - e.g. policy violations or various starting and stopping
   notifications.

For the purposes of this document, the grouping is slightly different:

1. Fatal exceptions - All Architectural exceptions and policy violations. Left
   unhandled by an exception handler, these exceptions guarantee that the
   process is terminated.
2. Non-fatal exceptions - All Synthetic exceptions except policy violations.
   These are only sent to the "Debugger" flavor of exception channels and will
   not terminate a process if left unhandled.

This means that the exception propagation walk order for non-fatal exceptions is
significantly different:

| Step | Channel | Delivery Type |
| :---- | :---- | :---- |
| 1 | Process Debugger | First-chance |
| 2 | Job Debugger | First-chance |
| 3 | Process Debugger | Second-chance |
| 4 | Job Debugger | Second-chance |

After being sent to the Job Debugger exception channel, the walk is terminated
and no other exception channels are considered. Technically, second-chance
exceptions are supported for these synthetic, non-fatal exceptions but in
practice this is unused.

##### Exception Propagation from Restricted Mode {#restricted-mode-exception-propagation}

For threads operating within restricted mode (see [RFC-0261][rfc-0261]),
exception propagation is different. The caller of `zx_restricted_enter` acts as
an in-thread handler for exceptions that occur while executing in restricted
mode. This handler is logically injected into the above table as so:

| Step | Channel | Delivery Type |
| :---- | :---- | :---- |
| 1 | Process Debugger | First-chance |
| 2 | In-thread | First-chance |
| 3 | Thread | First-chance |
| 4 | Process | First-chance |
| 5 | Process Debugger | Second-chance |
| 6 | Job | First-chance |
| 7 | Parent Job | First-chance |
| 8 | Grandparent Job | First-chance |
| ... | Up the job tree, until the root job is reached | |

However, one caveat of this "in-thread" handling is that the exception can no
longer be propagated further via typical Zircon exception channels. In other
words, any exception sent to the in-thread handler is always considered handled
by Zircon's perspective.

The result of this is the exception delivery table now looks like this in
reality when an architectural or policy exception originated from restricted
mode:

| Step | Channel | Delivery Type |
| :---- | :---- | :---- |
| 1 | Process Debugger | First-chance |
| 2 | In-thread | First-chance |
| 3 | N/A | N/A |

In other words, when an exception occurs in restricted mode, the in-thread
handler can be thought of as a catch-all exception handler that will always mark
the exception as handled. This means that no other entities in the typical
exception propagation list will get to see the exception. This also means that,
while the Process Debugger handler still gets to receive the exception before
the in-thread handler, it loses the ability to register for "second-chance"
exception handling later. This is an acceptable tradeoff since the process
debugger will still get the first chance to examine and possible handle the
exception before the in-thread handler will see and handle the exception.

**Note**: The above is purely when the thread is executing in restricted mode.
If that same thread is operating in normal mode for any reason (e.g. handling a
syscall) and induces an exception while in normal mode, it is treated the same
as any other Zircon thread that doesn't have any restricted state at all.

Non-fatal exception types are not sent to the in-thread exception handler, and
therefore retain the same delivery order as described in
[Exception Propagation](#exception-propagation).

##### Second-Chance Exception Handling

While handling an exception, an exception handler that is registered via the
Process Debugger exception channel may set the `ZX_PROP_EXCEPTION_STRATEGY`
property to `ZX_EXCEPTION_STRATEGY_SECOND_CHANCE` in order to have another
chance to handle the same exception later, if it remains unhandled by the other
handlers that are invoked after the Process Debugger has had first-chance. As
discussed above, the handler should be aware of the fact that this might not
happen for exceptions that have come from a thread in restricted mode. Zircon
will not return an error when setting this property on an exception that
originated from restricted mode.

#### Job Exception Channel Contention

The current Zircon exception mechanism dictates that the Job Exception channel
can only be claimed by a single entity. This is problematic when multiple system
components have a legitimate need to observe job-level exceptions. For instance,
a debugger may need to intercept software breakpoint exceptions for a group of
processes within a job, while a crash diagnostics service needs to observe
unhandled exceptions for diagnostics and reporting, and some child process of
the job requires the ability to open the Job exception channel to handle faults
from other processes within its parent job. All three of these are legitimate
use cases for creating an exception channel on the parent job of a particular
process, yet only one of these entities may claim it.

Today, components may be constructed using the
[`job_with_available_exception_channel`][job-with-available-exception-channel]
flag in the component manifest, but that is only a viable strategy for processes
within that component's job, debugging and crash reporting tools cannot assume
that there will be a job hierarchy such that there is an available exception
channel on some job above the process's parent.

#### Job Debugger Exception Channel

In [RFC-0178][rfc-0178] the Job Debugger exception channel's purpose was
expanded to allow for multiple (`ZX_EXCEPTION_CHANNEL_JOB_MAX_COUNT`) exception
channels to be registered with a single job under the pretense that this was a
"notification only" channel. In other words, it does not receive architectural
exceptions, only Process Starting events.

Today, the Job Debugger exception channel serves an odd role in the exception
delivery pipeline. This is the second difference between the flavors of
exception channels. The analogue to the Job Debugger exception channel, the
Process Debugger exception channel, differs in which types of exceptions are
sent to it:


| Exception Type | Job exception channel | Job Debugger exception channel | Process exception channel | Process Debugger exception channel |
| :---- | :---- | :---- | :---- | :---- |
| Architectural & Policy exceptions | ✅ | ❌ | ✅ | ✅ |
| "Child" starting events | ❌ | ✅ (Processes) | ❌ | ✅ (Threads) |
| "Child" exiting events | ❌ | ⚠️ (Signal only) | ❌ | ✅ (Threads) |

The Job Debugger exception channel today **only** receives Process Starting and
Process Exiting events, the first of which is of type `zx::exception` and the
second is of type `zx::signal`. Architectural and Policy exceptions are **not**
sent to the Job Debugger exception channel.

#### Delivery of "Child" Starting and Exiting Events

The job tree as defined by Zircon is well defined: Jobs have children, which may
comprise zero or more jobs and zero or more processes. Processes also have
children, consisting of precisely one or more threads. Other entities in a
running system which have the ability to distinguish one job from another may
claim the job's debugger exception channel to receive Process Starting events,
and may claim any process' debugger exception channel to receive Thread Starting
and Thread Exiting events. Notifications of equivalent notions for Zircon Jobs
have thus far not been provided by Zircon, and are out of scope for this RFC.

All of these events are delivered in the same way: a `zx::exception` handle is
delivered to the debugger exception channel of the parent. This allows clients
bound to the debugger exception channel to perform arbitrary actions while it is
guaranteed that the "child" entity is suspended, for example, a debugger to set
e.g. `ZX_PROP_PROCESS_BREAK_ON_LOAD` on the process' object handle or do
necessary accounting for a thread's destruction before the thread is destroyed.

Process exiting events today are special: they are sent as a signal
`ZX_PROCESS_TERMINATED`, rather than as a `zx::exception` to the Job Debugger
exception channel. This difference goes beyond simple semantics: because process
terminated is sent as a Zircon signal, rather than an exception, it is
completely asynchronous. Entities listening for this signal are guaranteed
nothing at the time of signal delivery, the process object may have already been
destroyed by Zircon. Compare this to ThreadExiting events sent to the Process
Debugger exception channel, which provides additional guarantees from Zircon
that the thread's state is still reachable. Thus, for a watching entity to
correctly halt a process during program exit for examination of the final
program state (primarily the handle table and memory) along with the thread
state, it must correctly account for all thread starting and exiting events and
notice explicitly when the final thread is exiting, which will send a
zx::exception notification for the entity to hold on to as long as necessary,
rather than just a signal.

Note that while Zircon guarantees protections against the typical thread
teardown machinery, it does not provide guarantees about what other processes
might do to the thread or its parent process in the meantime, for example issue
a `zx_task_kill` syscall, which will immediately terminate the process and all
of its threads regardless of the state of the Process Debugger exception channel
state. In other words, handling the exception for a given thread does not
protect its process from immediate termination via `zx_task_kill`.

## Stakeholders

_Facilitator:_

- abarth@google.com

_Reviewers:_

- mcgrathr@google.com
- maniscalco@google.com
- jamesr@google.com

_Consulted:_

- abarth@google.com
- cpu@google.com
- lindkvist@google.com

_Socialization:_

Early versions of this RFC were circulated among the fuchsia-zircon-discuss
mailing list and discussed among the Debug and Testing Architecture teams.

## Requirements

The design must ensure that architectural and policy exceptions as described in
[Zircon Exception Types][exception-types] are delivered to the Job Debugger
exception channel, and that the Job Debugger exception channel continues to
allow multiple registrants as described in [RFC-0178][rfc-0178].

## Design

### Send Architectural & Policy Exceptions to the Job Debugger Channel

We propose enhancing the existing Job Debugger Exception channel to receive
architectural and policy exceptions, in addition to the
`ZX_EXCP_PROCESS_STARTING` events it currently receives.

#### Exception Channel Walk Order

This change requires modifying the order in which Zircon propagates exceptions.
Based on the [exception channel Types documentation][exception-types], the new
delivery order for architectural and policy exceptions will be:

| Step | Channel | Delivery Type |
| :---- | :---- | :---- |
| 1 | Process Debugger | First-chance |
| 2 | Job Debugger | First-chance, N times |
| 3 | Thread | First-chance |
| 4 | Process | First-chance |
| 5 | Process Debugger | Second-chance |
| 6 | Job Debugger | Second-chance, N times |
| 7 | Job | First-chance |
| 8 | Ancestor Job Debugger | First-chance, N times |
| 9 | Ancestor Job | First-chance |
| ... | Up the job tree, continuing with Job Debugger<br>and Job Exception channels until the root job is reached | |

The key change is that the Job Debugger exception channel will now receive these
exceptions, up to N times where N is equal to
`ZX_EXCEPTION_CHANNEL_JOB_MAX_COUNT`. The Job Debugger exception channel of the
parent job of the process immediately follows the Process Debugger exception
channel in the walk, meaning that clients that are attached to the nearest
parent job of an excepting thread will get both a first chance and second chance
to handle the exception, just like with the Process Debugger exception channel.

The walk then continues up the job tree, going from Job Debugger to the Job
exception channels up to the root job. This allows debugger implementations
freedom to choose where in the job hierarchy to attach themselves for various
use cases.

The opportunity to receive second chance exceptions while attached to the Job
Debugger exception channel only apply to the parent job - the ancestor jobs
above the parent will only have first-chance opportunities to inspect the
exception.

##### Delivery of Architectural & Policy Exceptions

According to [Zircon Exception Types][exception-types] the Job Debugger
exception channel is the *only* exception channel that does *not* receive
architectural exceptions, making it unnecessarily unique. The reasons for this
originate before [RFC-0178][rfc-0178] but the motivation is briefly mentioned:

> However, "debug job" is distinctive here because it's a notification-only
> channel: the only exception type it can receive is `ZX_EXCP_PROCESS_STARTING`
> where the `ZX_PROP_EXCEPTION_STATE` is ignored. Thus it's possible to allow
> multiple debug exception channels on one job without worrying about
> inconsistencies.

The "inconsistencies" here refers to the order in which architectural exceptions
are delivered to such an exception channel that may have multiple registered
clients. Because the exclusivity principle that applies to all other exception
channels does not hold for the Job Debugger exception channel, there is not a
well defined order of which exception channel will get to see an exception
event before another at the same level.

This RFC proposes that this is a non-issue. The order that exceptions are
delivered to Job Debugger handlers for a particular job is implementation
defined by Zircon, and it is the responsibility of the handlers to be aware that
other handlers may come before it at the same level and mark an exception as
handled. Similarly, handlers must also be aware that they have received an
exception for another entity that comes after them, but is attached to the same
job's Job Debugger exception channel.

An implication of this is that this mechanism ineffective for handlers that
expect to have exclusive access to an exception at a particular layer in the job
tree. Such handlers should continue to use the Job's exception channel, and
handle the cases where that channel is already claimed by another handler, e.g.
when `zx_task_create_exception_channel` returns `ZX_ERR_ALREADY_BOUND`.

##### On Restricted Mode

Threads operating in [restricted mode][restricted-mode] need to be handled
especially carefully. Threads that trip an exception while executing in
restricted mode stay in restricted mode while the exception is delivered to the
Process Debugger exception channel as specified in [RFC-0261][rfc-0261]. Only
after the Process Debugger channel finishes its business with the exception,
and leaves the exception unhandled, will the thread be kicked out of restricted
mode and into normal mode for handling via a special in-thread exception handler
for restricted mode. No further exception channels will witness the exception,
and there are no second-chance exceptions as described in [Exception Propagation
from Restricted Mode](#restricted-mode-exception-propagation).

This RFC proposes no changes to restricted mode exception handling. It is left
to later RFCs to detail this process and how it will interact with the Job
Debugger exception channel.

#### Handling Logic

Job Debugger channels will be delivered exceptions on a first-come-first-serve
basis based on the registration order of the Job Debugger channels. The first
client in the Job Debugger channels that marks the exception as handled (i.e.
sets the `ZX_EXCEPTION_STATE_HANDLED` property on the exception handle) will
terminate the walk through the list of Job Debugger exception channels for that
job and prevent the exception from propagating further up the job tree.

Clients connecting to the Job Debugger channel must be aware that other
registered clients may handle the exception before them. This is encoded in the
contract of the Job Debugger channel, and makes this an inappropriate mechanism
for receiving exceptions for generic system crash handlers that expect to
operate in a production environment.

| Exception Type | Job exception channel | Job Debugger exception channel | Process exception channel | Process Debugger exception channel |
| :---- | :---- | :---- | :---- | :---- |
| Architectural & Policy exceptions | ✅ | ✅ (N times) | ✅ | ✅ |
| "Child" starting events | ❌ | ✅ (Processes) | ❌ | ✅ (Threads) |
| "Child" exiting events | ❌ | ⚠️ (Processes, Signal only) | ❌ | ✅ (Threads) |

### ProcessExiting Events

Creation and delivery of ProcessExiting exception events are left for a future
RFC. The delivery of the `ZX_PROCESS_TERMINATED` signal is unchanged.

## Implementation

The syscall API and ABI of `zx_task_create_exception_channel` will not be
altered by this proposal. In accordance with [RFC-0178][rfc-0178],
`zx_task_create_exception_channel` will continue to allow up to
`ZX_EXCEPTION_CHANNEL_JOB_MAX_COUNT` channels to be created instead of returning
`ZX_ERR_ALREADY_BOUND` after the first one.

Users of the `ZX_EXCEPTION_CHANNEL_DEBUGGER` option to
`zx_task_create_exception_channel` will need to be made aware that they may now
also receive architectural and policy exceptions from child processes of the
job. There are only a few users of this channel today, which can be easily
updated inline with the changes to Zircon. See below for more discussion of
these users.

## Performance

Exception delivery performance will be impeded in the case of multiple entities
claiming the Job Debugger exception channel, since the exception will have to be
delivered to (potentially several) additional clients before reaching the root
job where the thread and/or process will be terminated. Despite that, any single
client may still hold the exception for unbounded lengths of time, which is no
worse when there are multiple clients that the exception will be delivered to.

## Ergonomics

The ergonomics of using `zx_task_create_exception_channel` and exception
delivery from Zircon to userspace are unchanged.

## Backwards Compatibility

### System ABI Implications

The system ABI of the delivery of exceptions *is* modified by this change.
Previously, it was impossible to receive architectural or policy exceptions via
registering for the Job Debugger exception channel. After this change, not only
do registrants need to be aware of the delivery of these exceptions, they also
need to be aware of the fact that they might not be the first receiver of this
exception at this level of the job tree.

As of this writing, there are only two notable non-test users of the Job
Debugger exception channel:

1. [Debugger][debugger]
2. [Profiler][profiler]

Both of these entities exist within the fuchsia.git source tree and are
trivially updateable without introducing explicit versioning.

Depending on the configuration for the debugger for the particular use case,
information about the thread may be collected before otherwise forwarding
exceptions along the chain, or marking it as handled if so instructed by the
debugger user. Additional use cases may appear for the debugger in various
configurations and settings, which are left to the debugger implementation to
properly handle in the light of this ABI change.

In the case of the profiler, the only interest is in process starting
notifications, so any other `zx::exception` objects that it receives from this
channel can simply be closed immediately and ignored.

### Implications for [`elf_runner`][elf_runner]

The `elf_runner` today spawns and claims every ELF component's Job exception
channel in order to serve the [CrashIntrospect][crash-introspect] protocol to
[crashsvc][crashsvc], Fuchsia's crash service.

This could be improved so that the `elf_runner` would now only need to take a
single job debugger exception channel on the RootJob, which is guaranteed to
receive exceptions before they are sent to the RootJob's exception channel,
ensuring that crashsvc still has access to the component information of a
crashing component while requiring the `elf_runner` to claim far fewer
resources.

These changes are not immediately required for the `elf_runner` since it does
not use the Job Debugger exception channel today, and therefore will not be
changed in the initial implementation.

## Security considerations

The upper limit of allowable Job Debugger channels is addressed in
[RFC-0178][rfc-0178] and is not modified in this proposal, preventing any DOS
vectors against the kernel.

Exception information is generally available in both engineering and production
environments to code that claims the exception channel of a particular Zircon
task, so this proposal does not expose otherwise sensitive information. It does,
however, increase the allowable maximum of entities that may inspect exception
information.

## Privacy considerations

This proposal does not have any privacy implications.

## Testing

New test cases will be added to //zircon/system/utest/debugger to cover this
feature.

## Documentation

The [Exception Types][exception-types] documentation will be updated to reflect
the new ordering of exception channels that will receive architectural
exceptions as well as new notes about how exceptions from restricted mode
threads are handled.

## Drawbacks, alternatives, and unknowns

Two other primary approaches were considered as alternatives to the proposed
solution:

### FIDL Exception Server

This approach would centralize exception handling within a component, such as
`elf_runner`, which would then serve a new FIDL protocol to multiple interested
clients.

* **Pros:** Full control over exception ordering and policy (e.g.,
  distinguishing between a single "Handler" and multiple "Notify Only"
  listeners).
* **Cons:** Introduces complexity into the user-space component (`elf_runner`),
  requires iterative exception handle passing due to non-duplicability, and
  places the burden of filtering on the clients.

### Zircon Modification to Job exception channel

This approach would modify Zircon to allow multiple components to successfully
call `zx_task_create_exception_channel` on a job, but would require expressing
the "Handler" vs. "Notify Only" interest via new options passed to the Zircon
syscall.

* **Pros:** Leverage Zircon's existing exception handle rights and flow. Allows
  Zircon to concurrently send exceptions to all "Notify Only" channels.
  Filtering is done automatically by requiring the client to target the specific
  job.
* **Cons:** Requires a more significant change to the Zircon kernel API by
  introducing new options to distinguish between "handlers" and "notify only"
  clients.

The proposed solution (Use the Job Debugger Channel) is preferred because it
extends the existing multi-listener mechanism (the Job Debugger channel) to
handle new exception types, minimizing differences between the Job Debugger
exception channel and the Process Debugger exception channel and minimizing new
API surface area in Zircon.

[crashsvc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/bringup/bin/critical-services/crashsvc/
[crash-introspect]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.sys2/crash_introspect.fidl
[debugger]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/developer/debug/shared/message_loop_fuchsia.cc;l=274-275;drc=19cca2dc1a25ec5fd85c1178248a1bc9cedcc8c3
[elf_runner]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/lib/elf_runner/src/crash_handler.rs
[exception-types]: /docs/concepts/kernel/exceptions.md
[job-with-available-exception-channel]: /docs/concepts/components/v2/elf_runner.md
[profiler]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/performance/experimental/profiler/job_watcher.cc;l=24;drc=19cca2dc1a25ec5fd85c1178248a1bc9cedcc8c3
[restricted-mode]: /docs/concepts/starnix/syscalls.md
[rfc-0178]: /docs/contribute/governance/rfcs/0178_multiple_debug_job_exception_channel.md
[rfc-0261]: /docs/contribute/governance/rfcs/0261_fast_and_efficient_user_space_kernel_emulation.md
